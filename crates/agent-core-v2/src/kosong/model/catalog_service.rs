//! Config-backed model resolver, requester cache, and catalog operations.
//!
//! Original: `packages/agent-core-v2/src/kosong/model/catalogService.ts`,
//! `ModelCatalog`.
//!
//! The cache is invalidated only by the model/provider change events. All
//! filesystem, network, and token work remains in the injected services and
//! requester; model resolution itself is synchronous.

use std::{
    error::Error,
    ops::Deref,
    sync::{Arc, Mutex},
    time::Instant,
};

use async_trait::async_trait;
use futures_util::StreamExt;
use indexmap::IndexMap;

use crate::{
    _base::{
        di::{
            descriptors::SyncDescriptor,
            instantiation::{ServiceIdentifier, ServicesAccessorExt},
            lifecycle::{Disposable, DisposableStore, DisposeResult},
            scope::{InstantiationType, LifecycleScope, register_scoped_service},
        },
        errors::errors::Error2,
    },
    app::{
        auth::{OAUTH_SERVICE_ID, OAuthServiceHandle},
        config::{CONFIG_INVALID, CONFIG_SERVICE_ID, ConfigServiceHandle, ConfigTarget},
    },
    kosong::{
        contract::{
            inspection::{InspectionSource, InspectionSourceKind, ResolutionTrace},
            message::{ContentPart, Message, Role, StreamedMessagePart},
            provider::{FinishReason, ProviderRequestAuth},
            usage::TokenUsage,
        },
        protocol::identity::{
            PROTOCOL_ADAPTER_REGISTRY_SERVICE_ID, Protocol, ProtocolAdapterRegistryHandle,
        },
        provider::{
            DEFAULT_PROVIDER_SECTION, PROVIDER_SERVICE_ID, ProviderConfig, ProviderServiceHandle,
            provider_definition::{explain_provider_endpoint, get_provider_definition},
        },
    },
};

use super::{
    catalog::{
        AuthProvider, AuthRequestOptions, Model, ModelCatalogItem, ModelPingResult,
        ProviderCatalogItem, ProviderCredentialState, SetDefaultModelResponse, StaticAuthProvider,
        build_protocol_provider_options, has_configured_api_key, profile_for_attribution,
        resolve_model_capabilities, resolve_outbound_headers, strip_trailing_v1, to_protocol_model,
        to_protocol_model_fallback, to_protocol_provider,
    },
    contract::{
        DEFAULT_MODEL_SECTION, MODEL_SERVICE_ID, ModelRecord, ModelServiceHandle, ModelsSection,
    },
    errors::{MODEL_NOT_FOUND, PROVIDER_NOT_FOUND, ensure_model_catalog_errors_registered},
    host_request_headers::{HOST_REQUEST_HEADERS_ID, HostRequestHeaders},
    inspection::{
        ModelInspection, ResolutionTraceCollector, TRACE, assemble_model_inspection,
        attribute_effective_fields, attribute_provider_options,
    },
    model_auth::{
        ResolveModelAuthArgs, derive_provider_id, effective_model_config, non_empty,
        resolve_model_auth_material,
    },
    model_requester::{ModelRequestEvent, ModelRequestInput, ModelRequestParams, ModelRequester},
    model_requester_impl::ModelRequesterImpl,
};

pub type ModelCatalogError = Box<dyn Error + Send + Sync>;
pub type ModelCatalogResult<T> = Result<T, ModelCatalogError>;

#[async_trait]
pub trait ModelCatalogContract: Disposable + Send + Sync {
    // Original: IModelCatalog.get().
    fn get(&self, id: &str) -> ModelCatalogResult<Arc<Model>>;
    // Original: IModelCatalog.getRequester().
    fn get_requester(&self, id: &str) -> ModelCatalogResult<Arc<dyn ModelRequester>>;
    // Original: IModelCatalog.inspect().
    fn inspect(&self, id: &str) -> ModelCatalogResult<ModelInspection>;
    // Original: IModelCatalog.ping().
    async fn ping(&self, id: &str) -> ModelPingResult;
    // Original: IModelCatalog.findByName().
    fn find_by_name(&self, name: &str) -> Vec<String>;
    // Original: IModelCatalog.listModels().
    async fn list_models(&self) -> Vec<ModelCatalogItem>;
    // Original: IModelCatalog.listProviders().
    async fn list_providers(&self) -> ModelCatalogResult<Vec<ProviderCatalogItem>>;
    // Original: IModelCatalog.getProvider().
    async fn get_provider(&self, provider_id: &str) -> ModelCatalogResult<ProviderCatalogItem>;
    // Original: IModelCatalog.setDefaultModel().
    async fn set_default_model(
        &self,
        model_id: &str,
    ) -> ModelCatalogResult<SetDefaultModelResponse>;
}

#[derive(Clone)]
pub struct ModelCatalogHandle(pub Arc<dyn ModelCatalogContract>);

impl Deref for ModelCatalogHandle {
    type Target = dyn ModelCatalogContract;

    fn deref(&self) -> &Self::Target {
        self.0.as_ref()
    }
}

impl Disposable for ModelCatalogHandle {
    fn dispose(&self) -> DisposeResult {
        self.0.dispose()
    }
}

// Original: catalog.ts, IModelCatalog. The source deliberately retains the
// deleted IModelResolver identity for compatibility with existing consumers.
pub const MODEL_CATALOG_SERVICE_ID: ServiceIdentifier<ModelCatalogHandle> =
    ServiceIdentifier::new("modelResolver");

struct CatalogEntry {
    model: Arc<Model>,
    requester: Arc<dyn ModelRequester>,
    trace: Arc<ResolutionTraceCollector>,
}

pub struct ModelCatalog {
    config: ConfigServiceHandle,
    providers: ProviderServiceHandle,
    models: ModelServiceHandle,
    oauth: OAuthServiceHandle,
    protocol_registry: ProtocolAdapterRegistryHandle,
    host_request_headers: HostRequestHeaders,
    cache: Arc<Mutex<IndexMap<String, Arc<CatalogEntry>>>>,
    disposables: DisposableStore,
}

impl ModelCatalog {
    // Original: ModelCatalog.constructor().
    pub fn new(
        config: ConfigServiceHandle,
        providers: ProviderServiceHandle,
        models: ModelServiceHandle,
        oauth: OAuthServiceHandle,
        protocol_registry: ProtocolAdapterRegistryHandle,
        host_request_headers: HostRequestHeaders,
    ) -> Self {
        ensure_model_catalog_errors_registered();
        let cache = Arc::new(Mutex::new(IndexMap::new()));
        let disposables = DisposableStore::new();
        for event in [
            models.on_did_change_models().map(|_| ()),
            providers.on_did_change_providers().map(|_| ()),
        ] {
            let cache = Arc::clone(&cache);
            disposables.add(event.subscribe(move |_| {
                cache
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .clear();
            }));
        }
        Self {
            config,
            providers,
            models,
            oauth,
            protocol_registry,
            host_request_headers,
            cache,
            disposables,
        }
    }

    // Original: ModelCatalog.notifyConfigChanged().
    pub fn notify_config_changed(&self) {
        self.cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
    }

    fn entry(&self, id: &str) -> ModelCatalogResult<Arc<CatalogEntry>> {
        if let Some(entry) = self
            .cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(id)
            .cloned()
        {
            return Ok(entry);
        }
        let mut trace = ResolutionTraceCollector::default();
        let model = Arc::new(self.build_model(id, &mut trace)?);
        let requester: Arc<dyn ModelRequester> = Arc::new(ModelRequesterImpl::new(
            Arc::clone(&model),
            Arc::clone(&self.protocol_registry.0),
        ));
        let entry = Arc::new(CatalogEntry {
            model,
            requester,
            trace: Arc::new(trace),
        });
        let mut cache = self
            .cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Ok(cache
            .entry(id.to_owned())
            .or_insert_with(|| Arc::clone(&entry))
            .clone())
    }

    // Original: ModelCatalog.toCatalogProvider().
    async fn to_catalog_provider(
        &self,
        provider_id: &str,
        provider: &ProviderConfig,
        models: &ModelsSection,
        global_default_model: Option<&str>,
    ) -> ModelCatalogResult<ProviderCatalogItem> {
        let credential = self.resolve_credential(provider_id, provider).await?;
        Ok(to_protocol_provider(
            provider_id,
            provider,
            models,
            global_default_model,
            credential,
        ))
    }

    // Original: ModelCatalog.resolveCredential().
    async fn resolve_credential(
        &self,
        provider_id: &str,
        provider: &ProviderConfig,
    ) -> ModelCatalogResult<ProviderCredentialState> {
        Ok(ProviderCredentialState {
            has_api_key: has_configured_api_key(provider)?,
            has_oauth_token: self.has_cached_token(provider_id, provider).await,
        })
    }

    // Original: ModelCatalog.hasCachedToken(). Failure is deliberately
    // converted to false instead of failing provider enumeration.
    async fn has_cached_token(&self, provider_id: &str, provider: &ProviderConfig) -> bool {
        let Some(oauth) = provider.oauth.as_ref() else {
            return false;
        };
        self.oauth
            .get_cached_access_token(provider_id, Some(oauth))
            .await
            .ok()
            .and_then(|token| token)
            .is_some_and(|token| non_empty(Some(&token)).is_some())
    }

    // Original: ModelCatalog.providerTypeOf().
    fn provider_type_of(&self, record: &ModelRecord) -> Option<String> {
        let configured_default = self
            .config
            .get(DEFAULT_PROVIDER_SECTION)
            .and_then(|value| value.as_str().map(str::to_owned));
        let provider_id = record
            .provider_id
            .as_deref()
            .or(record.provider.as_deref())
            .or(configured_default.as_deref());
        self.providers
            .get(provider_id.unwrap_or_default())
            .and_then(|provider| provider.provider_type.map(|kind| kind.to_string()))
            .or_else(|| record.protocol.map(|protocol| protocol.to_string()))
    }

    // Original: ModelCatalog.buildModel().
    fn build_model(
        &self,
        id: &str,
        trace: &mut ResolutionTraceCollector,
    ) -> ModelCatalogResult<Model> {
        let configured_model = self.models.get(id).ok_or_else(|| {
            coded_error(
                CONFIG_INVALID,
                format!("Model \"{id}\" is not configured in config.toml."),
            )
        })?;
        trace.capture_value(TRACE.configured_model, configured_model.clone());
        trace.record(
            "model.record",
            source(InspectionSourceKind::Config, "[models.*] section"),
        );

        let routing_model = effective_model_config(&configured_model, None)?;
        let context = self.resolve_provider_context(id, &routing_model, trace)?;
        trace.capture_value(TRACE.provider_config, context.provider_config.clone());
        trace.capture_value(TRACE.provider_name, context.provider_name.clone());
        trace.capture_value(TRACE.raw_base_url, context.resolved_base_url.clone());

        let protocol =
            self.resolve_protocol(id, &routing_model, context.provider_config.as_ref(), trace)?;
        let profile_provider_type = context
            .provider_config
            .as_ref()
            .and_then(|provider| provider.provider_type.as_ref())
            .map(|kind| kind.as_str())
            .or_else(|| configured_model.protocol.map(Protocol::as_str));
        let model = effective_model_config(&configured_model, profile_provider_type)?;
        trace.capture_value(TRACE.effective_model, model.clone());
        let wire_name = model.name.as_deref().or(model.model.as_deref());
        let (profile, inferred) = profile_for_attribution(
            &configured_model,
            context.provider_config.as_ref(),
            wire_name,
        )?;
        attribute_effective_fields(trace, &configured_model, &model, profile, inferred);

        let auth = resolve_model_auth_material(
            ResolveModelAuthArgs {
                model_id: id,
                model: &model,
                provider: context.provider_config.as_ref(),
                provider_name: &context.provider_name,
            },
            Some(trace),
        )?;
        trace.capture_value(TRACE.auth_material, auth.clone());
        let auth_provider = self.build_auth_provider(&context.provider_name, auth);
        let provider_type = context
            .provider_config
            .as_ref()
            .and_then(|provider| provider.provider_type.clone())
            .unwrap_or_else(|| {
                super::super::provider::config::ProviderType::from(protocol.as_str())
            });
        let resolved_base_url = if protocol == Protocol::Anthropic {
            context.resolved_base_url.as_deref().map(strip_trailing_v1)
        } else {
            context.resolved_base_url.clone()
        };
        let wire_name = wire_name.ok_or_else(|| {
            coded_error(
                CONFIG_INVALID,
                format!("Model \"{id}\" must define a wire-facing name in config.toml."),
            )
        })?;
        let max_context_size = model
            .max_context_size
            .ok_or_else(|| {
                coded_error(
                    CONFIG_INVALID,
                    format!(
                        "Model \"{id}\" must define a positive max_context_size in config.toml."
                    ),
                )
            })?
            .get();
        let explained = self.protocol_registry.explain_capability(
            protocol,
            wire_name,
            Some(provider_type.as_str()),
        );
        trace.capture_value(TRACE.detected_capability, explained.capability.clone());
        trace.capture_value(TRACE.capability_source, explained.source.clone());
        let capabilities = resolve_model_capabilities(
            model.capabilities.as_deref(),
            &explained.capability,
            max_context_size,
        );
        let provider_options = build_protocol_provider_options(
            &model,
            protocol,
            context.provider_config.as_ref(),
            resolved_base_url.as_deref(),
        );
        if let Some(options) = &provider_options {
            attribute_provider_options(
                trace,
                options,
                context
                    .provider_config
                    .as_ref()
                    .and_then(|provider| provider.env.as_ref()),
            );
        }
        let always_thinking = model
            .capabilities
            .as_deref()
            .unwrap_or_default()
            .iter()
            .any(|capability| capability.trim().eq_ignore_ascii_case("always_thinking"));
        trace.capture_value(
            TRACE.host_headers,
            self.host_request_headers.headers.clone(),
        );
        Ok(Model {
            id: id.to_owned(),
            name: wire_name.to_owned(),
            aliases: model.aliases.unwrap_or_default(),
            protocol,
            base_url: resolved_base_url,
            headers: resolve_outbound_headers(
                Some(provider_type.as_str()),
                context
                    .provider_config
                    .as_ref()
                    .and_then(|provider| provider.custom_headers.as_ref()),
                &self.host_request_headers.headers,
            )?,
            capabilities,
            max_context_size,
            max_output_size: model.max_output_size.map(|size| size.get()),
            display_name: model.display_name,
            reasoning_key: model.reasoning_key,
            support_efforts: model.support_efforts,
            default_effort: model.default_effort,
            always_thinking,
            provider_type: Some(provider_type),
            provider_name: context.provider_name,
            auth_provider,
            provider_options,
        })
    }

    // Original: ModelCatalog.resolveProviderContext().
    fn resolve_provider_context(
        &self,
        id: &str,
        model: &ModelRecord,
        trace: &mut ResolutionTraceCollector,
    ) -> ModelCatalogResult<ProviderContext> {
        let default_provider = self
            .config
            .get(DEFAULT_PROVIDER_SECTION)
            .and_then(|value| value.as_str().map(str::to_owned));
        let provider_id = model
            .provider_id
            .as_deref()
            .or(model.provider.as_deref())
            .or(default_provider.as_deref());
        if let Some(provider_id) = provider_id {
            let detail = if model.provider_id.is_some() {
                format!("model.providerId '{provider_id}'")
            } else if model.provider.is_some() {
                format!("model.provider '{provider_id}'")
            } else {
                format!("[defaultProvider] '{provider_id}'")
            };
            trace.record("provider", source(InspectionSourceKind::Config, &detail));
            trace.capture_value(TRACE.provider_synthesized, false);
            let provider_config = self.providers.get(provider_id).ok_or_else(|| {
                coded_error(
                    CONFIG_INVALID,
                    format!(
                        "Provider \"{provider_id}\" referenced by model \"{id}\" is not configured."
                    ),
                )
            })?;
            let resolved_base_url = if let Some(base_url) = non_empty(model.base_url.as_deref()) {
                trace.record(
                    "resolved.baseUrl",
                    source(InspectionSourceKind::Config, "model.baseUrl"),
                );
                Some(base_url.to_owned())
            } else if let Some(base_url) = non_empty(provider_config.base_url.as_deref()) {
                trace.record(
                    "resolved.baseUrl",
                    source(
                        InspectionSourceKind::Config,
                        &format!("provider '{provider_id}' baseUrl"),
                    ),
                );
                Some(base_url.to_owned())
            } else {
                let endpoint_type = provider_config
                    .provider_type
                    .as_ref()
                    .map(|kind| kind.as_str())
                    .or_else(|| model.protocol.map(Protocol::as_str));
                let endpoint = endpoint_type
                    .map(|kind| {
                        explain_provider_endpoint(
                            kind,
                            provider_config.env.as_ref().unwrap_or(&IndexMap::new()),
                        )
                    })
                    .transpose()?;
                if let Some(endpoint) = endpoint {
                    if let Some(name) = endpoint.base_url_env_name {
                        trace.record(
                            "resolved.baseUrl",
                            source(
                                InspectionSourceKind::Env,
                                &format!("{name} (provider '{provider_id}' env bag)"),
                            ),
                        );
                    } else if endpoint.base_url_is_default == Some(true) {
                        trace.record(
                            "resolved.baseUrl",
                            source(
                                InspectionSourceKind::Builtin,
                                &format!(
                                    "provider definition '{}' defaultBaseUrl",
                                    endpoint_type.unwrap_or_default()
                                ),
                            ),
                        );
                    }
                    non_empty(endpoint.base_url.as_deref()).map(str::to_owned)
                } else {
                    None
                }
            };
            return Ok(ProviderContext {
                provider_config: Some(provider_config),
                provider_name: provider_id.to_owned(),
                resolved_base_url,
            });
        }
        let base_url = non_empty(model.base_url.as_deref()).ok_or_else(|| {
            coded_error(
                CONFIG_INVALID,
                format!("Model \"{id}\" must set either providerId or baseUrl in config.toml."),
            )
        })?;
        trace.record(
            "provider",
            source(
                InspectionSourceKind::Synthesized,
                "flat model — provider synthesized from the baseUrl host",
            ),
        );
        trace.capture_value(TRACE.provider_synthesized, true);
        trace.record(
            "resolved.baseUrl",
            source(InspectionSourceKind::Config, "model.baseUrl (flat)"),
        );
        Ok(ProviderContext {
            provider_config: None,
            provider_name: derive_provider_id(base_url),
            resolved_base_url: Some(base_url.to_owned()),
        })
    }

    // Original: ModelCatalog.resolveProtocol().
    fn resolve_protocol(
        &self,
        id: &str,
        model: &ModelRecord,
        provider: Option<&ProviderConfig>,
        trace: &mut ResolutionTraceCollector,
    ) -> ModelCatalogResult<Protocol> {
        if let Some(protocol) = model.protocol {
            trace.record(
                "resolved.protocol",
                source(InspectionSourceKind::Config, "model.protocol"),
            );
            return Ok(protocol);
        }
        if let Some(provider_type) = provider.and_then(|provider| provider.provider_type.as_ref()) {
            if let Ok(protocol) = provider_type.as_str().parse() {
                trace.record(
                    "resolved.protocol",
                    source(
                        InspectionSourceKind::Config,
                        &format!(
                            "provider type '{}' is itself a wire protocol",
                            provider_type
                        ),
                    ),
                );
                return Ok(protocol);
            }
            if let Some(definition) = get_provider_definition(provider_type.as_str(), None)? {
                trace.record(
                    "resolved.protocol",
                    source(
                        InspectionSourceKind::Builtin,
                        &format!("vendor '{}' declared baseProtocol", provider_type),
                    ),
                );
                return Ok(definition.base_protocol);
            }
        }
        Err(coded_error(
            CONFIG_INVALID,
            format!("Model \"{id}\" must declare a wire protocol (config: models.<id>.protocol)."),
        ))
    }

    // Original: ModelCatalog.buildAuthProvider().
    fn build_auth_provider(
        &self,
        provider_name: &str,
        auth: super::types::ResolvedModelAuthMaterial,
    ) -> Arc<dyn AuthProvider> {
        if let Some(api_key) = auth.api_key {
            return Arc::new(StaticAuthProvider::new(Some(api_key)));
        }
        if let Some(oauth_ref) = auth.oauth {
            return Arc::new(OAuthAuthProvider {
                oauth: self.oauth.clone(),
                provider_key: auth
                    .oauth_provider_key
                    .unwrap_or_else(|| provider_name.to_owned()),
                oauth_ref,
            });
        }
        Arc::new(StaticAuthProvider::default())
    }
}

#[async_trait]
impl ModelCatalogContract for ModelCatalog {
    fn get(&self, id: &str) -> ModelCatalogResult<Arc<Model>> {
        Ok(Arc::clone(&self.entry(id)?.model))
    }

    fn get_requester(&self, id: &str) -> ModelCatalogResult<Arc<dyn ModelRequester>> {
        Ok(Arc::clone(&self.entry(id)?.requester))
    }

    fn inspect(&self, id: &str) -> ModelCatalogResult<ModelInspection> {
        let entry = self.entry(id)?;
        Ok(assemble_model_inspection(id, &entry.model, &entry.trace)?)
    }

    async fn ping(&self, id: &str) -> ModelPingResult {
        let started_at = Instant::now();
        let result = async {
            let requester = self.get_requester(id)?;
            let mut stream =
                requester.request(
                    ModelRequestInput {
                        system_prompt:
                            "You are a connectivity probe. Answer with the single word \"pong\"."
                                .into(),
                        tools: Vec::new(),
                        messages: vec![Message::new(
                            Role::User,
                            vec![ContentPart::Text {
                                text: "ping".into(),
                            }],
                            Vec::new(),
                        )],
                        response_format: None,
                    },
                    None,
                    Some(ModelRequestParams {
                        max_completion_tokens: Some(512),
                        ..ModelRequestParams::default()
                    }),
                );
            let mut text = String::new();
            let mut usage: Option<TokenUsage> = None;
            let mut finish_reason: Option<String> = None;
            while let Some(event) = stream.next().await {
                match event.map_err(|error| -> ModelCatalogError { Box::new(error) })? {
                    ModelRequestEvent::Part(StreamedMessagePart::Content(ContentPart::Text {
                        text: part,
                    })) => text.push_str(&part),
                    ModelRequestEvent::Usage {
                        usage: event_usage, ..
                    } => usage = Some(event_usage),
                    ModelRequestEvent::Finish {
                        provider_finish_reason,
                        raw_finish_reason,
                        ..
                    } => {
                        finish_reason = provider_finish_reason
                            .map(finish_reason_name)
                            .or(raw_finish_reason);
                    }
                    _ => {}
                }
            }
            Ok::<_, ModelCatalogError>((text, usage, finish_reason))
        }
        .await;
        match result {
            Ok((text, usage, finish_reason)) => ModelPingResult {
                ok: true,
                duration_ms: started_at.elapsed().as_secs_f64() * 1000.0,
                text: Some(text.trim().to_owned()),
                finish_reason,
                usage,
                error: None,
            },
            Err(error) => ModelPingResult {
                ok: false,
                duration_ms: started_at.elapsed().as_secs_f64() * 1000.0,
                text: None,
                finish_reason: None,
                usage: None,
                error: Some(error.to_string()),
            },
        }
    }

    fn find_by_name(&self, name: &str) -> Vec<String> {
        self.models
            .list()
            .into_iter()
            .filter_map(|(id, record)| {
                (record.name.as_deref() == Some(name)
                    || record.model.as_deref() == Some(name)
                    || record
                        .aliases
                        .as_deref()
                        .is_some_and(|aliases| aliases.iter().any(|alias| alias == name)))
                .then_some(id)
            })
            .collect()
    }

    async fn list_models(&self) -> Vec<ModelCatalogItem> {
        self.models
            .list()
            .into_iter()
            .filter_map(|(id, record)| {
                let provider_type = self.provider_type_of(&record);
                self.get(&id)
                    .ok()
                    .and_then(|model| {
                        to_protocol_model(&model, &record, provider_type.as_deref()).ok()
                    })
                    .or_else(|| {
                        to_protocol_model_fallback(&id, &record, provider_type.as_deref()).ok()
                    })
            })
            .collect()
    }

    async fn list_providers(&self) -> ModelCatalogResult<Vec<ProviderCatalogItem>> {
        let providers = self.providers.list();
        let models = self.models.list();
        let default_model = self
            .config
            .get(DEFAULT_MODEL_SECTION)
            .and_then(|value| value.as_str().map(str::to_owned));
        let mut result = Vec::with_capacity(providers.len());
        for (id, provider) in providers {
            result.push(
                self.to_catalog_provider(&id, &provider, &models, default_model.as_deref())
                    .await?,
            );
        }
        Ok(result)
    }

    async fn get_provider(&self, provider_id: &str) -> ModelCatalogResult<ProviderCatalogItem> {
        let provider = self.providers.get(provider_id).ok_or_else(|| {
            coded_error(
                PROVIDER_NOT_FOUND,
                format!("provider {provider_id} does not exist"),
            )
        })?;
        let models = self.models.list();
        let default_model = self
            .config
            .get(DEFAULT_MODEL_SECTION)
            .and_then(|value| value.as_str().map(str::to_owned));
        self.to_catalog_provider(provider_id, &provider, &models, default_model.as_deref())
            .await
    }

    async fn set_default_model(
        &self,
        model_id: &str,
    ) -> ModelCatalogResult<SetDefaultModelResponse> {
        let record = self.models.get(model_id).ok_or_else(|| {
            coded_error(MODEL_NOT_FOUND, format!("model {model_id} does not exist"))
        })?;
        let model = self.get(model_id)?;
        self.config
            .set(
                DEFAULT_MODEL_SECTION,
                Some(serde_json::Value::String(model_id.to_owned())),
                ConfigTarget::User,
            )
            .await?;
        Ok(SetDefaultModelResponse {
            default_model: model_id.to_owned(),
            model: to_protocol_model(&model, &record, self.provider_type_of(&record).as_deref())?,
        })
    }
}

impl Disposable for ModelCatalog {
    fn dispose(&self) -> DisposeResult {
        self.disposables.dispose()
    }
}

struct ProviderContext {
    provider_config: Option<ProviderConfig>,
    provider_name: String,
    resolved_base_url: Option<String>,
}

struct OAuthAuthProvider {
    oauth: OAuthServiceHandle,
    provider_key: String,
    oauth_ref: crate::kosong::provider::OAuthRef,
}

#[async_trait]
impl AuthProvider for OAuthAuthProvider {
    fn can_refresh(&self) -> bool {
        true
    }

    async fn get_auth(
        &self,
        options: Option<AuthRequestOptions>,
    ) -> Result<Option<ProviderRequestAuth>, ModelCatalogError> {
        let token_provider = self
            .oauth
            .resolve_token_provider(&self.provider_key, Some(&self.oauth_ref))
            .ok_or_else(|| {
                coded_error(
                    crate::app::auth::AUTH_LOGIN_REQUIRED,
                    format!(
                        "OAuth provider \"{}\" requires login before it can be used.",
                        self.provider_key
                    ),
                )
            })?;
        let api_key = token_provider
            .get_access_token(options.is_some_and(|options| options.force))
            .await
            .map_err(|_error| {
                coded_error(
                    crate::app::auth::AUTH_LOGIN_REQUIRED,
                    format!(
                        "OAuth provider \"{}\" requires login before it can be used.",
                        self.provider_key
                    ),
                )
            })?;
        if api_key.trim().is_empty() {
            return Err(coded_error(
                crate::app::auth::AUTH_LOGIN_REQUIRED,
                format!(
                    "OAuth provider \"{}\" requires login before it can be used.",
                    self.provider_key
                ),
            ));
        }
        Ok(Some(ProviderRequestAuth {
            api_key: Some(api_key),
            headers: None,
        }))
    }
}

fn source(kind: InspectionSourceKind, detail: &str) -> InspectionSource {
    InspectionSource {
        kind,
        detail: Some(detail.to_owned()),
    }
}
fn coded_error(code: impl Into<String>, message: impl Into<String>) -> ModelCatalogError {
    Box::new(Error2::new(code, message))
}
fn finish_reason_name(reason: FinishReason) -> String {
    match reason {
        FinishReason::Completed => "completed",
        FinishReason::ToolCalls => "tool_calls",
        FinishReason::Truncated => "truncated",
        FinishReason::Filtered => "filtered",
        FinishReason::Paused => "paused",
        FinishReason::Other => "other",
    }
    .into()
}

// Original: catalogService.ts module setup. Rust explicitly composes services
// instead of relying on TypeScript import side effects.
pub fn register_model_catalog() {
    register_scoped_service(
        LifecycleScope::App,
        MODEL_CATALOG_SERVICE_ID,
        SyncDescriptor::new(|accessor| {
            let service: Arc<dyn ModelCatalogContract> = Arc::new(ModelCatalog::new(
                (*accessor.get(CONFIG_SERVICE_ID)?).clone(),
                (*accessor.get(PROVIDER_SERVICE_ID)?).clone(),
                (*accessor.get(MODEL_SERVICE_ID)?).clone(),
                (*accessor.get(OAUTH_SERVICE_ID)?).clone(),
                (*accessor.get(PROTOCOL_ADAPTER_REGISTRY_SERVICE_ID)?).clone(),
                (*accessor.get(HOST_REQUEST_HEADERS_ID)?).clone(),
            ));
            Ok(ModelCatalogHandle(service))
        })
        .disposable(),
        InstantiationType::Eager,
        "modelCatalog",
    );
}

#[cfg(test)]
mod tests {
    use std::{num::NonZeroU64, sync::Arc};

    use async_trait::async_trait;
    use serde_json::{Map, Value};

    use crate::{
        _base::{di::lifecycle::Disposable, event::Emitter},
        app::{
            auth::{
                AuthOperationError, AuthStatus, OAuthFlowSnapshot, OAuthFlowStart,
                OAuthLoginCancelResponse, OAuthLogoutResponse, RefreshOAuthProviderModelsResponse,
            },
            config::{
                ConfigChangeSource, ConfigChangedEvent, ConfigDiagnostic, ConfigInspectValue,
                ConfigServiceContract, ConfigServiceError, ResolvedConfig,
            },
        },
        kosong::{
            protocol::identity::ProtocolAdapterRegistry,
            provider::{
                ProviderServiceContract, ProviderServiceResult, ProvidersChangedEvent,
                ProvidersSection,
            },
        },
    };

    use super::*;

    struct StubConfig {
        values: Mutex<ResolvedConfig>,
        events: Emitter<ConfigChangedEvent>,
    }

    impl StubConfig {
        fn new(values: ResolvedConfig) -> Self {
            Self {
                values: Mutex::new(values),
                events: Emitter::new(),
            }
        }
    }

    #[async_trait]
    impl ConfigServiceContract for StubConfig {
        async fn ready(&self) -> Result<(), ConfigServiceError> {
            Ok(())
        }
        fn on_did_change_configuration(&self) -> crate::_base::event::Event<ConfigChangedEvent> {
            self.events.event()
        }
        fn on_did_section_change(&self) -> crate::_base::event::Event<ConfigChangedEvent> {
            self.events.event()
        }
        fn get(&self, domain: &str) -> Option<Value> {
            self.values.lock().unwrap().get(domain).cloned()
        }
        fn inspect(&self, domain: &str) -> ConfigInspectValue {
            ConfigInspectValue {
                value: self.get(domain),
                ..ConfigInspectValue::default()
            }
        }
        fn get_all(&self) -> ResolvedConfig {
            self.values.lock().unwrap().clone()
        }
        async fn set(
            &self,
            domain: &str,
            value: Option<Value>,
            _target: ConfigTarget,
        ) -> Result<(), ConfigServiceError> {
            let previous_value = self
                .values
                .lock()
                .unwrap()
                .insert(domain.into(), value.clone().unwrap_or(Value::Null));
            self.events.fire(&ConfigChangedEvent {
                domain: domain.into(),
                source: ConfigChangeSource::Set,
                value,
                previous_value,
            });
            Ok(())
        }
        async fn replace(
            &self,
            domain: &str,
            value: Option<Value>,
            target: ConfigTarget,
        ) -> Result<(), ConfigServiceError> {
            self.set(domain, value, target).await
        }
        async fn reload(&self) -> Result<(), ConfigServiceError> {
            Ok(())
        }
        fn diagnostics(&self) -> Vec<ConfigDiagnostic> {
            Vec::new()
        }
    }

    impl Disposable for StubConfig {
        fn dispose(&self) -> DisposeResult {
            self.events.dispose()
        }
    }

    struct StubProviders {
        values: ProvidersSection,
        events: Emitter<ProvidersChangedEvent>,
    }

    #[async_trait]
    impl ProviderServiceContract for StubProviders {
        async fn ready(&self) -> ProviderServiceResult<()> {
            Ok(())
        }
        fn on_did_change_providers(&self) -> crate::_base::event::Event<ProvidersChangedEvent> {
            self.events.event()
        }
        fn get(&self, name: &str) -> Option<ProviderConfig> {
            self.values.get(name).cloned()
        }
        fn list(&self) -> ProvidersSection {
            self.values.clone()
        }
        async fn set(&self, _name: &str, _config: ProviderConfig) -> ProviderServiceResult<()> {
            Ok(())
        }
        async fn delete(&self, _name: &str) -> ProviderServiceResult<()> {
            Ok(())
        }
    }

    impl Disposable for StubProviders {
        fn dispose(&self) -> DisposeResult {
            self.events.dispose()
        }
    }

    struct StubModels {
        values: ModelsSection,
        events: Emitter<super::super::contract::ModelsChangedEvent>,
    }

    #[async_trait]
    impl crate::kosong::model::contract::ModelServiceContract for StubModels {
        fn on_did_change_models(
            &self,
        ) -> crate::_base::event::Event<super::super::contract::ModelsChangedEvent> {
            self.events.event()
        }
        fn get(&self, id: &str) -> Option<ModelRecord> {
            self.values.get(id).cloned()
        }
        fn list(&self) -> ModelsSection {
            self.values.clone()
        }
        async fn set(
            &self,
            _id: &str,
            _model: ModelRecord,
        ) -> crate::kosong::model::contract::ModelServiceResult<()> {
            Ok(())
        }
        async fn delete(
            &self,
            _id: &str,
        ) -> crate::kosong::model::contract::ModelServiceResult<()> {
            Ok(())
        }
    }

    impl Disposable for StubModels {
        fn dispose(&self) -> DisposeResult {
            self.events.dispose()
        }
    }

    struct StubOAuth;

    #[async_trait]
    impl crate::app::auth::OAuthServiceContract for StubOAuth {
        async fn start_login(
            &self,
            _provider: Option<&str>,
        ) -> Result<OAuthFlowStart, AuthOperationError> {
            Err(AuthOperationError::new("unused"))
        }
        fn get_flow(&self, _provider: Option<&str>) -> Option<OAuthFlowSnapshot> {
            None
        }
        async fn cancel_login(
            &self,
            _provider: Option<&str>,
        ) -> Result<OAuthLoginCancelResponse, AuthOperationError> {
            Err(AuthOperationError::new("unused"))
        }
        async fn logout(
            &self,
            _provider: Option<&str>,
        ) -> Result<OAuthLogoutResponse, AuthOperationError> {
            Err(AuthOperationError::new("unused"))
        }
        async fn status(&self, _provider: Option<&str>) -> Result<AuthStatus, AuthOperationError> {
            Err(AuthOperationError::new("unused"))
        }
        async fn refresh_oauth_provider_models(
            &self,
        ) -> Result<RefreshOAuthProviderModelsResponse, AuthOperationError> {
            Err(AuthOperationError::new("unused"))
        }
        fn resolve_token_provider(
            &self,
            _provider: &str,
            _oauth_ref: Option<&crate::kosong::provider::OAuthRef>,
        ) -> Option<kimi_code_oauth::BearerTokenProvider> {
            None
        }
        async fn get_cached_access_token(
            &self,
            _provider: &str,
            _oauth_ref: Option<&crate::kosong::provider::OAuthRef>,
        ) -> Result<Option<String>, AuthOperationError> {
            Ok(None)
        }
    }

    fn catalog_with_flat_model() -> (ModelCatalog, Arc<StubConfig>) {
        let config = Arc::new(StubConfig::new(Map::new()));
        let providers: Arc<dyn ProviderServiceContract> = Arc::new(StubProviders {
            values: ProvidersSection::new(),
            events: Emitter::new(),
        });
        let models: Arc<dyn crate::kosong::model::contract::ModelServiceContract> =
            Arc::new(StubModels {
                values: IndexMap::from([(
                    "flat".into(),
                    ModelRecord {
                        base_url: Some("https://api.example.test/v1".into()),
                        protocol: Some(Protocol::OpenAi),
                        name: Some("wire-name".into()),
                        aliases: Some(vec!["alias".into()]),
                        max_context_size: NonZeroU64::new(32_000),
                        ..ModelRecord::default()
                    },
                )]),
                events: Emitter::new(),
            });
        let oauth: Arc<dyn crate::app::auth::OAuthServiceContract> = Arc::new(StubOAuth);
        let registry: Arc<dyn ProtocolAdapterRegistry> = Arc::new(
            crate::kosong::provider::protocol_adapter_registry::ProtocolAdapterRegistry::new(),
        );
        (
            ModelCatalog::new(
                ConfigServiceHandle(config.clone()),
                ProviderServiceHandle(providers),
                ModelServiceHandle(models),
                OAuthServiceHandle(oauth),
                ProtocolAdapterRegistryHandle(registry),
                HostRequestHeaders::default(),
            ),
            config,
        )
    }

    #[tokio::test]
    async fn caches_one_resolution_then_invalidates_and_persists_default_after_materialization() {
        let (catalog, config) = catalog_with_flat_model();
        let first = catalog.get("flat").unwrap();
        let second = catalog.get("flat").unwrap();
        assert!(Arc::ptr_eq(&first, &second));
        assert_eq!(first.provider_name, "api.example.test");
        assert_eq!(catalog.find_by_name("wire-name"), ["flat"]);
        assert_eq!(catalog.find_by_name("alias"), ["flat"]);
        assert_eq!(
            catalog.inspect("flat").unwrap().resolved.wire_name,
            "wire-name"
        );

        catalog.notify_config_changed();
        assert!(!Arc::ptr_eq(&first, &catalog.get("flat").unwrap()));
        let default = catalog.set_default_model("flat").await.unwrap();
        assert_eq!(default.default_model, "flat");
        assert_eq!(
            config.get(DEFAULT_MODEL_SECTION),
            Some(Value::String("flat".into()))
        );
    }
}
