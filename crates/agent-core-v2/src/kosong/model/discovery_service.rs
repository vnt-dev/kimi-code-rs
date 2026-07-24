//! Provider-model discovery service.
//!
//! Original: `packages/agent-core-v2/src/kosong/model/discoveryService.ts`.
//!
//! The OAuth crate owns the protocol-specific refresh orchestration. This
//! service keeps the original application's boundary responsibilities: reload
//! before every refresh, serialize refreshes, protect static model sources,
//! persist the resulting user configuration, and publish catalog changes.

use std::{error::Error, sync::Arc};

use async_trait::async_trait;
use indexmap::IndexMap;
use kimi_code_oauth::{
    ManagedKimiOAuthRef, OAuthStorageBackend, RefreshHostError, RefreshProviderHost,
    RefreshProviderOptions, RefreshProviderScope, RefreshResult, refresh_provider_models,
};
use serde_json::{Map, Value};
use tokio::sync::Mutex;

use crate::{
    _base::{
        di::{
            descriptors::SyncDescriptor,
            instantiation::ServicesAccessorExt,
            scope::{InstantiationType, LifecycleScope, register_scoped_service},
        },
        errors::errors::Error2,
    },
    app::{
        auth::{OAUTH_SERVICE_ID, OAuthServiceHandle},
        config::{CONFIG_SERVICE_ID, ConfigServiceHandle, ConfigTarget},
        event::{EVENT_SERVICE_ID, EventServiceHandle, GlobalDomainEvent},
    },
    kosong::provider::{
        ModelSource, OAuthRef, OAuthStorage, PROVIDER_SERVICE_ID, PROVIDERS_SECTION,
        ProviderConfig, ProviderServiceHandle, provider_definition::get_provider_definition,
    },
};

use super::discovery::{
    PROVIDER_DISCOVERY_SERVICE_ID, ProviderDiscoveryResult, ProviderDiscoveryServiceContract,
    ProviderDiscoveryServiceHandle, ProviderRefreshChange, ProviderRefreshFailure,
    RefreshProviderModelsOptions, RefreshProviderModelsResponse, RefreshProviderModelsScope,
};
use super::{
    contract::{
        DEFAULT_MODEL_SECTION, MODEL_SERVICE_ID, MODELS_SECTION, ModelRecord, ModelServiceHandle,
    },
    errors::{PROVIDER_NOT_FOUND, ensure_model_catalog_errors_registered},
    host_request_headers::{HOST_REQUEST_HEADERS_ID, HostRequestHeaders},
    thinking::THINKING_SECTION,
};

// Original: discoveryService.ts, withoutKeys().
// IndexMap preserves the source object's configured insertion order.
pub fn without_keys<T: Clone, U>(
    record: &IndexMap<String, T>,
    excluded: &IndexMap<String, U>,
) -> IndexMap<String, T> {
    record
        .iter()
        .filter(|(key, _)| !excluded.contains_key(*key))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

#[derive(Clone, Default)]
struct StaticExclusion {
    providers: Map<String, Value>,
    models: Map<String, Value>,
    default_model: Option<Value>,
    thinking: Option<Value>,
}

pub struct ProviderDiscoveryService {
    // The original constructor receives IModelService. It is retained in the
    // Rust construction boundary for direct method-level traceability, even
    // though this source method currently reads model records from config.
    _models: ModelServiceHandle,
    providers: ProviderServiceHandle,
    config: ConfigServiceHandle,
    oauth: OAuthServiceHandle,
    events: EventServiceHandle,
    host_request_headers: HostRequestHeaders,
    refresh_gate: Mutex<()>,
}

impl ProviderDiscoveryService {
    // Original: ProviderDiscoveryService.constructor().
    pub fn new(
        models: ModelServiceHandle,
        providers: ProviderServiceHandle,
        config: ConfigServiceHandle,
        oauth: OAuthServiceHandle,
        events: EventServiceHandle,
        host_request_headers: HostRequestHeaders,
    ) -> Self {
        ensure_model_catalog_errors_registered();
        Self {
            _models: models,
            providers,
            config,
            oauth,
            events,
            host_request_headers,
            refresh_gate: Mutex::new(()),
        }
    }

    // Original: ProviderDiscoveryService.doRefreshProviderModels().
    async fn do_refresh_provider_models(
        &self,
        options: RefreshProviderModelsOptions,
    ) -> ProviderDiscoveryResult<RefreshProviderModelsResponse> {
        self.config.reload().await?;
        if let Some(provider_id) = options.provider_id.as_deref() {
            let provider = self.providers.get(provider_id).ok_or_else(|| {
                Box::new(Error2::new(
                    PROVIDER_NOT_FOUND,
                    format!("provider {provider_id} does not exist"),
                )) as Box<dyn Error + Send + Sync>
            })?;
            if effective_model_source(&provider)? == Some(ModelSource::Static) {
                return Ok(RefreshProviderModelsResponse {
                    changed: Vec::new(),
                    unchanged: vec![provider_id.to_owned()],
                    failed: Vec::new(),
                });
            }
        }

        let exclusion = self.compute_static_exclusion()?;
        let host = RefreshHostAdapter {
            config: self.config.clone(),
            oauth: self.oauth.clone(),
            exclusion,
            user_agent: self.host_request_headers.headers.get("User-Agent").cloned(),
        };
        let result = refresh_provider_models(
            &host,
            &RefreshProviderOptions {
                scope: match options.scope.unwrap_or(RefreshProviderModelsScope::All) {
                    RefreshProviderModelsScope::All => RefreshProviderScope::All,
                    RefreshProviderModelsScope::OAuth => RefreshProviderScope::OAuth,
                },
                provider_id: options.provider_id,
            },
        )
        .await
        .map_err(boxed_error)?;
        let response = map_refresh_result(result);
        if !response.changed.is_empty() {
            self.events.publish(GlobalDomainEvent {
                event_type: "event.model_catalog.changed".into(),
                payload: serde_json::to_value(&response)?,
            });
        }
        Ok(response)
    }

    // Original: ProviderDiscoveryService.computeStaticExclusion().
    fn compute_static_exclusion(&self) -> ProviderDiscoveryResult<StaticExclusion> {
        let providers = user_object(&self.config, PROVIDERS_SECTION);
        let mut static_ids = Vec::new();
        for (id, value) in &providers {
            let provider = serde_json::from_value::<ProviderConfig>(value.clone())?;
            if effective_model_source(&provider)? == Some(ModelSource::Static) {
                static_ids.push(id.clone());
            }
        }
        if static_ids.is_empty() {
            return Ok(StaticExclusion::default());
        }

        let excluded_providers = static_ids
            .iter()
            .filter_map(|id| providers.get(id).map(|value| (id.clone(), value.clone())))
            .collect::<Map<_, _>>();
        let models = user_object(&self.config, MODELS_SECTION);
        let mut excluded_models = Map::new();
        for (id, value) in &models {
            let record = serde_json::from_value::<ModelRecord>(value.clone())?;
            if record
                .provider
                .is_some_and(|provider| excluded_providers.contains_key(&provider))
            {
                excluded_models.insert(id.clone(), value.clone());
            }
        }
        let default_model = self.config.inspect(DEFAULT_MODEL_SECTION).user_value;
        let preserve_selection = default_model
            .as_ref()
            .and_then(Value::as_str)
            .is_some_and(|model| excluded_models.contains_key(model));
        Ok(StaticExclusion {
            providers: excluded_providers,
            models: excluded_models,
            default_model: preserve_selection.then_some(default_model).flatten(),
            thinking: preserve_selection
                .then(|| self.config.inspect(THINKING_SECTION).user_value)
                .flatten(),
        })
    }
}

#[async_trait]
impl ProviderDiscoveryServiceContract for ProviderDiscoveryService {
    // Original: ProviderDiscoveryService.refreshProviderModels(). The async
    // mutex is the Rust equivalent of the source's refreshChain promise.
    async fn refresh_provider_models(
        &self,
        options: Option<RefreshProviderModelsOptions>,
    ) -> ProviderDiscoveryResult<RefreshProviderModelsResponse> {
        let _refresh = self.refresh_gate.lock().await;
        self.do_refresh_provider_models(options.unwrap_or_default())
            .await
    }
}

struct RefreshHostAdapter {
    config: ConfigServiceHandle,
    oauth: OAuthServiceHandle,
    exclusion: StaticExclusion,
    user_agent: Option<String>,
}

impl RefreshHostAdapter {
    // Original: ProviderDiscoveryService.readUserConfigShape().
    fn read_user_config_shape(&self, exclusion: &StaticExclusion) -> Map<String, Value> {
        let mut shape = Map::new();
        shape.insert(
            "providers".into(),
            Value::Object(without_json_keys(
                &user_object(&self.config, PROVIDERS_SECTION),
                &exclusion.providers,
            )),
        );
        shape.insert(
            "models".into(),
            Value::Object(without_json_keys(
                &user_object(&self.config, MODELS_SECTION),
                &exclusion.models,
            )),
        );
        if let Some(value) = self.config.inspect(DEFAULT_MODEL_SECTION).user_value {
            shape.insert("defaultModel".into(), value);
        }
        if let Some(value) = self.config.inspect(THINKING_SECTION).user_value {
            shape.insert("thinking".into(), value);
        }
        shape
    }

    // Original: ProviderDiscoveryService.removeProviderForRefresh().
    async fn remove_provider_for_refresh(
        &self,
        provider_id: &str,
    ) -> Result<Map<String, Value>, RefreshHostError> {
        let current = self.read_user_config_shape(&StaticExclusion::default());
        let mut providers = object_field(&current, "providers");
        providers.remove(provider_id);
        let models: Map<String, Value> = object_field(&current, "models")
            .into_iter()
            .filter(|(_, value)| {
                value
                    .get("provider")
                    .and_then(Value::as_str)
                    .is_none_or(|provider| provider != provider_id)
            })
            .collect();
        self.config
            .replace(
                PROVIDERS_SECTION,
                Some(Value::Object(providers.clone())),
                ConfigTarget::User,
            )
            .await
            .map_err(RefreshHostError::new)?;
        self.config
            .replace(
                MODELS_SECTION,
                Some(Value::Object(models.clone())),
                ConfigTarget::User,
            )
            .await
            .map_err(RefreshHostError::new)?;
        let mut result = current;
        result.insert("providers".into(), Value::Object(providers));
        result.insert("models".into(), Value::Object(models));
        Ok(result)
    }

    // Original: ProviderDiscoveryService.applyRefreshPatch().
    async fn apply_refresh_patch(
        &self,
        patch: Map<String, Value>,
    ) -> Result<Map<String, Value>, RefreshHostError> {
        if let Some(providers) = patch.get("providers") {
            let merged = merge_excluded(&self.exclusion.providers, providers);
            self.config
                .replace(
                    PROVIDERS_SECTION,
                    Some(Value::Object(merged)),
                    ConfigTarget::User,
                )
                .await
                .map_err(RefreshHostError::new)?;
        }
        if let Some(models) = patch.get("models") {
            let merged = merge_excluded(&self.exclusion.models, models);
            self.config
                .replace(
                    MODELS_SECTION,
                    Some(Value::Object(merged)),
                    ConfigTarget::User,
                )
                .await
                .map_err(RefreshHostError::new)?;
        }
        let restore_default = self.exclusion.default_model.is_some();
        // The TypeScript orchestrator always sends these keys, including
        // explicit `undefined` to clear them. Rust's JSON map cannot encode
        // `undefined`, so the OAuth port omits a cleared key; both forms mean
        // the same write intent at this host boundary.
        self.config
            .replace(
                DEFAULT_MODEL_SECTION,
                if restore_default {
                    self.exclusion.default_model.clone()
                } else {
                    patch.get("defaultModel").cloned()
                },
                ConfigTarget::User,
            )
            .await
            .map_err(RefreshHostError::new)?;
        self.config
            .replace(
                THINKING_SECTION,
                if restore_default {
                    self.exclusion.thinking.clone()
                } else {
                    patch.get("thinking").cloned()
                },
                ConfigTarget::User,
            )
            .await
            .map_err(RefreshHostError::new)?;
        Ok(self.read_user_config_shape(&StaticExclusion::default()))
    }
}

#[async_trait]
impl RefreshProviderHost for RefreshHostAdapter {
    async fn get_config(&self) -> Result<Map<String, Value>, RefreshHostError> {
        Ok(self.read_user_config_shape(&self.exclusion))
    }

    async fn remove_provider(
        &self,
        provider_id: &str,
    ) -> Result<Map<String, Value>, RefreshHostError> {
        self.remove_provider_for_refresh(provider_id).await
    }

    async fn set_config(
        &self,
        patch: Map<String, Value>,
    ) -> Result<Map<String, Value>, RefreshHostError> {
        self.apply_refresh_patch(patch).await
    }

    async fn resolve_oauth_token(
        &self,
        provider_name: &str,
        oauth_ref: Option<&ManagedKimiOAuthRef>,
    ) -> Result<String, RefreshHostError> {
        let oauth_ref = oauth_ref.map(as_provider_oauth_ref);
        let token_provider = self
            .oauth
            .resolve_token_provider(provider_name, oauth_ref.as_ref())
            .ok_or_else(|| {
                RefreshHostError::new(std::io::Error::other(
                    "OAuth token provider is not configured.",
                ))
            })?;
        token_provider
            .get_access_token(false)
            .await
            .map_err(RefreshHostError::new)
    }

    fn user_agent(&self) -> Option<&str> {
        self.user_agent.as_deref()
    }
}

fn effective_model_source(
    provider: &ProviderConfig,
) -> ProviderDiscoveryResult<Option<ModelSource>> {
    match provider.model_source {
        Some(source) => Ok(Some(source)),
        None => Ok(match provider.provider_type.as_ref() {
            Some(provider_type) => get_provider_definition(provider_type.as_str(), None)?
                .and_then(|definition| definition.model_source),
            None => None,
        }),
    }
}

fn user_object(config: &ConfigServiceHandle, section: &str) -> Map<String, Value> {
    config
        .inspect(section)
        .user_value
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default()
}

fn without_json_keys(
    record: &Map<String, Value>,
    excluded: &Map<String, Value>,
) -> Map<String, Value> {
    record
        .iter()
        .filter(|(key, _)| !excluded.contains_key(*key))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

fn object_field(shape: &Map<String, Value>, key: &str) -> Map<String, Value> {
    shape
        .get(key)
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default()
}

fn merge_excluded(excluded: &Map<String, Value>, patch: &Value) -> Map<String, Value> {
    let mut merged = excluded.clone();
    if let Some(patch) = patch.as_object() {
        merged.extend(patch.clone());
    }
    merged
}

fn as_provider_oauth_ref(reference: &ManagedKimiOAuthRef) -> OAuthRef {
    OAuthRef {
        storage: match reference.storage {
            OAuthStorageBackend::File => OAuthStorage::File,
            OAuthStorageBackend::Keyring => OAuthStorage::Keyring,
        },
        key: reference.key.clone(),
        oauth_host: reference.oauth_host.clone(),
    }
}

fn boxed_error(error: RefreshHostError) -> Box<dyn Error + Send + Sync> {
    Box::new(error)
}

// Original: discoveryService.ts module registration.
pub fn register_provider_discovery_service() {
    register_scoped_service(
        LifecycleScope::App,
        PROVIDER_DISCOVERY_SERVICE_ID,
        SyncDescriptor::new(|accessor| {
            let service: Arc<dyn ProviderDiscoveryServiceContract> =
                Arc::new(ProviderDiscoveryService::new(
                    (*accessor.get(MODEL_SERVICE_ID)?).clone(),
                    (*accessor.get(PROVIDER_SERVICE_ID)?).clone(),
                    (*accessor.get(CONFIG_SERVICE_ID)?).clone(),
                    (*accessor.get(OAUTH_SERVICE_ID)?).clone(),
                    (*accessor.get(EVENT_SERVICE_ID)?).clone(),
                    (*accessor.get(HOST_REQUEST_HEADERS_ID)?).clone(),
                ));
            Ok(ProviderDiscoveryServiceHandle(service))
        }),
        InstantiationType::Eager,
        "modelCatalog",
    );
}

// Original: discoveryService.ts, mapRefreshResult().
pub fn map_refresh_result(result: RefreshResult) -> RefreshProviderModelsResponse {
    RefreshProviderModelsResponse {
        changed: result
            .changed
            .into_iter()
            .map(|change| ProviderRefreshChange {
                provider_id: change.provider_id,
                provider_name: change.provider_name,
                added: change.added as u64,
                removed: change.removed as u64,
            })
            .collect(),
        unchanged: result.unchanged,
        failed: result
            .failed
            .into_iter()
            .map(|failure| ProviderRefreshFailure {
                provider: failure.provider,
                reason: failure.reason,
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use kimi_code_oauth::{ProviderChange, ProviderRefreshFailure};

    use super::*;

    #[test]
    fn removes_only_excluded_records_and_maps_refresh_wire_fields() {
        let retained = without_keys(
            &IndexMap::from([("static".into(), 1), ("refreshable".into(), 2)]),
            &IndexMap::from([("static".into(), ())]),
        );
        assert_eq!(retained, IndexMap::from([("refreshable".into(), 2)]));

        let response = map_refresh_result(RefreshResult {
            changed: vec![ProviderChange {
                provider_id: "kimi".into(),
                provider_name: "Kimi".into(),
                added: 2,
                removed: 1,
            }],
            unchanged: vec!["static".into()],
            failed: vec![ProviderRefreshFailure {
                provider: "openai".into(),
                reason: "offline".into(),
            }],
        });
        assert_eq!(response.changed[0].provider_id, "kimi");
        assert_eq!(response.changed[0].added, 2);
        assert_eq!(response.unchanged, ["static"]);
        assert_eq!(response.failed[0].reason, "offline");
    }

    #[test]
    fn static_entries_are_merged_back_before_a_refresh_write() {
        let merged = merge_excluded(
            serde_json::json!({"static": {"type": "fixed"}})
                .as_object()
                .unwrap(),
            &serde_json::json!({"discovered": {"type": "remote"}}),
        );
        assert_eq!(
            merged,
            serde_json::json!({
                "static": {"type": "fixed"},
                "discovered": {"type": "remote"}
            })
            .as_object()
            .unwrap()
            .clone()
        );

        let replaced = merge_excluded(
            serde_json::json!({"same": {"source": "static"}})
                .as_object()
                .unwrap(),
            &serde_json::json!({"same": {"source": "refresh"}}),
        );
        assert_eq!(replaced["same"]["source"], "refresh");
    }

    #[test]
    fn managed_oauth_reference_uses_the_provider_storage_wire_values() {
        let provider = as_provider_oauth_ref(&ManagedKimiOAuthRef {
            storage: OAuthStorageBackend::Keyring,
            key: "oauth/kimi-code".into(),
            oauth_host: Some("https://auth.example.test".into()),
        });
        assert_eq!(provider.storage, OAuthStorage::Keyring);
        assert_eq!(provider.key, "oauth/kimi-code");
        assert_eq!(
            provider.oauth_host.as_deref(),
            Some("https://auth.example.test")
        );
    }
}
