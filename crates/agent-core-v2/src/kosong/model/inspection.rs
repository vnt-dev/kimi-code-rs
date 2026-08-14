//! Config-resolution provenance and the on-demand model inspection view.
//!
//! Original: `packages/agent-core-v2/src/kosong/model/inspection.ts`.
//!
//! This module is deliberately synchronous: it only captures references and
//! transforms in-memory configuration/model values. Network and token access
//! remain in the model catalog/requester layer.

use std::{
    collections::HashMap,
    sync::{Arc, LazyLock},
};

use indexmap::IndexMap;
use kimi_code_oauth::parse_kimi_code_custom_headers;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::kosong::{
    contract::{
        capability::ModelCapability,
        inspection::{
            CapturedResolutionValue, InspectionSource, InspectionSourceKind, ResolutionTrace,
        },
    },
    protocol::identity::{Protocol, ProtocolProviderOptions},
    provider::{
        config::{ModelSource, ProviderConfig},
        provider_definition::{
            HostHeaders, ProviderDefinitionRegistryError, get_provider_definition,
        },
    },
};

use super::{catalog::Model, contract::ModelRecord, types::ResolvedModelAuthMaterial};

/// Keys shared by model resolution and inspection assembly.
pub struct TraceKeys {
    pub configured_model: &'static str,
    pub effective_model: &'static str,
    pub provider_config: &'static str,
    pub provider_name: &'static str,
    pub provider_synthesized: &'static str,
    pub raw_base_url: &'static str,
    pub auth_material: &'static str,
    pub detected_capability: &'static str,
    pub capability_source: &'static str,
    pub host_headers: &'static str,
}

// Original: inspection.ts, TRACE.
pub const TRACE: TraceKeys = TraceKeys {
    configured_model: "configuredModel",
    effective_model: "effectiveModel",
    provider_config: "providerConfig",
    provider_name: "providerName",
    provider_synthesized: "providerSynthesized",
    raw_base_url: "rawBaseUrl",
    auth_material: "authMaterial",
    detected_capability: "detectedCapability",
    capability_source: "capabilitySource",
    host_headers: "hostHeaders",
};

/// Reference-only trace captured during one model-resolution pass.
#[derive(Default)]
pub struct ResolutionTraceCollector {
    source_map: IndexMap<String, InspectionSource>,
    capture_map: HashMap<String, CapturedResolutionValue>,
}

impl ResolutionTraceCollector {
    // Original: ResolutionTraceCollector.captured().
    pub fn captured<T: Send + Sync + 'static>(&self, key: &str) -> Option<&T> {
        self.capture_map.get(key)?.downcast_ref::<T>()
    }

    // Original: ResolutionTraceCollector.sources.
    pub fn sources(&self) -> &IndexMap<String, InspectionSource> {
        &self.source_map
    }

    pub fn capture_value<T: Send + Sync + 'static>(&mut self, key: &str, value: T) {
        self.capture(key, Arc::new(value));
    }
}

impl ResolutionTrace for ResolutionTraceCollector {
    fn record(&mut self, path: &str, source: InspectionSource) {
        self.source_map.insert(path.to_owned(), source);
    }

    fn capture(&mut self, key: &str, value: CapturedResolutionValue) {
        self.capture_map.insert(key.to_owned(), value);
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InspectedAuth {
    pub kind: InspectedAuthKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub oauth_provider_key: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum InspectedAuthKind {
    ApiKey,
    OAuth,
    None,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InspectedResolvedModel {
    pub protocol: Protocol,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_type: Option<String>,
    pub provider_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    pub wire_name: String,
    pub aliases: Vec<String>,
    pub auth: InspectedAuth,
    pub capabilities: ModelCapability,
    pub max_context_size: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_output_size: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub support_efforts: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_effort: Option<String>,
    pub always_thinking: bool,
    pub headers: IndexMap<String, String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_options: Option<ProtocolProviderOptions>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelInspection {
    pub id: String,
    pub model: InspectedModel,
    pub provider: InspectedProvider,
    pub resolved: InspectedResolvedModel,
    pub sources: IndexMap<String, InspectionSource>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct InspectedModel {
    pub id: String,
    /// JSON is intentional: passthrough model fields are preserved and deeply
    /// redacted exactly as the TypeScript `Record<string, unknown>` output.
    pub record: Value,
    pub effective: Value,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InspectedProvider {
    pub id: String,
    pub synthesized: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub config: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub definition: Option<InspectedProviderDefinition>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InspectedProviderDefinition {
    pub registered: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_protocol: Option<Protocol>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_source: Option<ModelSource>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host_headers: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<Value>,
}

#[derive(Debug, thiserror::Error)]
pub enum InspectionAssemblyError {
    #[error("model inspection is missing captured {0}")]
    MissingCapture(&'static str),
    #[error(transparent)]
    ProviderDefinition(#[from] ProviderDefinitionRegistryError),
    #[error(transparent)]
    Serialization(#[from] serde_json::Error),
}

const CAPABILITY_KEYS: [&str; 6] = [
    "image_in",
    "video_in",
    "audio_in",
    "thinking",
    "tool_use",
    "dynamically_loaded_tools",
];

static SECRET_KEY_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new("(?i)api[-_]?key|token|secret|password|authorization")
        .expect("static secret-key expression must compile")
});

// Original: inspection.ts, maskSecret().
pub fn mask_secret(value: &str) -> String {
    let mut reversed = value.chars().rev();
    let suffix = reversed.by_ref().take(4).collect::<Vec<_>>();
    if reversed.next().is_none() {
        "••••".to_owned()
    } else {
        format!("••••{}", suffix.into_iter().rev().collect::<String>())
    }
}

// Original: inspection.ts, redactSecrets().
pub fn redact_secrets(value: &Value) -> Value {
    redact_value(value, &SECRET_KEY_RE)
}

fn redact_value(value: &Value, secret_key: &Regex) -> Value {
    match value {
        Value::Array(items) => Value::Array(
            items
                .iter()
                .map(|item| redact_value(item, secret_key))
                .collect(),
        ),
        Value::Object(items) => Value::Object(
            items
                .iter()
                .map(|(key, item)| {
                    let redacted = if item.is_string() && secret_key.is_match(key) {
                        Value::String(mask_secret(item.as_str().unwrap_or_default()))
                    } else {
                        redact_value(item, secret_key)
                    };
                    (key.clone(), redacted)
                })
                .collect(),
        ),
        _ => value.clone(),
    }
}

// Original: inspection.ts, attributeEffectiveFields().
pub fn attribute_effective_fields(
    trace: &mut ResolutionTraceCollector,
    configured: &ModelRecord,
    effective: &ModelRecord,
    profile: Option<
        super::super::provider::bases::anthropic::anthropic_profile::AnthropicModelProfile,
    >,
    profile_inferred: bool,
) -> Result<(), serde_json::Error> {
    let mut configured = serde_json::to_value(configured)?;
    let effective = serde_json::to_value(effective)?;
    let overrides = configured
        .as_object_mut()
        .and_then(|record| record.remove("overrides"))
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default();
    let configured = configured.as_object().cloned().unwrap_or_default();
    let effective = effective.as_object().cloned().unwrap_or_default();
    let profile_detail = profile.map(|profile| {
        let mode = match profile.mode {
            super::super::provider::bases::anthropic::anthropic_profile::AnthropicThinkingMode::Budget => "budget",
            super::super::provider::bases::anthropic::anthropic_profile::AnthropicThinkingMode::Adaptive => "adaptive",
        };
        format!(
            "anthropic profile ({mode}, efforts: {}{})",
            profile.efforts.join("/"),
            if profile_inferred { ", inferred fallback" } else { "" }
        )
    });
    let mut keys = IndexMap::new();
    for key in configured.keys().chain(effective.keys()) {
        keys.insert(key.clone(), ());
    }
    for key in keys.keys() {
        let path = format!("model.effective.{key}");
        let before = configured.get(key);
        let after = effective.get(key);
        if before.is_none() && after.is_none() {
            continue;
        }
        if overrides.contains_key(key) {
            trace.record(
                &path,
                source(InspectionSourceKind::Override, "models.*.overrides"),
            );
        } else if after.is_none() {
            trace.record(
                &path,
                source(
                    InspectionSourceKind::Synthesized,
                    "removed by the effective pass (defaultEffort not in override supportEfforts)",
                ),
            );
        } else if matches!(
            key.as_str(),
            "capabilities" | "supportEfforts" | "defaultEffort"
        ) && profile_detail.is_some()
            && before != after
        {
            trace.record(
                &path,
                source(
                    InspectionSourceKind::Builtin,
                    profile_detail.as_deref().unwrap_or_default(),
                ),
            );
        } else {
            trace.record(
                &path,
                source(InspectionSourceKind::Config, "[models.*] section"),
            );
        }
    }
    Ok(())
}

// Original: inspection.ts, attributeProviderOptions().
pub fn attribute_provider_options(
    trace: &mut ResolutionTraceCollector,
    options: &ProtocolProviderOptions,
    provider_env: Option<&IndexMap<String, String>>,
) -> Result<(), serde_json::Error> {
    let values = serde_json::to_value(options)?;
    let Some(values) = values.as_object() else {
        return Ok(());
    };
    for key in values.keys() {
        let path = format!("resolved.providerOptions.{key}");
        let value = match key.as_str() {
            "vertexai" => source(
                InspectionSourceKind::Env,
                "provider env bag supplies both vertex coordinates",
            ),
            "project" => source(
                InspectionSourceKind::Env,
                "GOOGLE_CLOUD_PROJECT (provider env bag)",
            ),
            "location"
                if provider_env.is_some_and(|env| env.contains_key("GOOGLE_CLOUD_LOCATION")) =>
            {
                source(
                    InspectionSourceKind::Env,
                    "GOOGLE_CLOUD_LOCATION (provider env bag)",
                )
            }
            "location" => source(
                InspectionSourceKind::Synthesized,
                "parsed from the baseUrl host",
            ),
            "defaultMaxTokens" => trace
                .sources()
                .get("model.effective.maxOutputSize")
                .cloned()
                .unwrap_or_else(|| source(InspectionSourceKind::Config, "[models.*] section")),
            "supportEfforts" => trace
                .sources()
                .get("model.effective.supportEfforts")
                .cloned()
                .unwrap_or_else(|| source(InspectionSourceKind::Config, "[models.*] section")),
            "adaptiveThinking" => trace
                .sources()
                .get("model.effective.adaptiveThinking")
                .cloned()
                .unwrap_or_else(|| source(InspectionSourceKind::Config, "[models.*] section")),
            "betaApi" => trace
                .sources()
                .get("model.effective.betaApi")
                .cloned()
                .unwrap_or_else(|| source(InspectionSourceKind::Config, "[models.*] section")),
            "reasoningKey" => trace
                .sources()
                .get("model.effective.reasoningKey")
                .cloned()
                .unwrap_or_else(|| source(InspectionSourceKind::Config, "[models.*] section")),
            _ => source(InspectionSourceKind::Config, "[models.*] section"),
        };
        trace.record(&path, value);
    }
    Ok(())
}

// Original: inspection.ts, assembleModelInspection().
pub fn assemble_model_inspection(
    id: &str,
    model: &Model,
    trace: &ResolutionTraceCollector,
) -> Result<ModelInspection, InspectionAssemblyError> {
    let configured =
        required_capture::<ModelRecord>(trace, TRACE.configured_model, "configured model")?;
    let effective =
        required_capture::<ModelRecord>(trace, TRACE.effective_model, "effective model")?;
    let provider_config = trace
        .captured::<Option<ProviderConfig>>(TRACE.provider_config)
        .cloned()
        .flatten();
    let provider_name = trace
        .captured::<String>(TRACE.provider_name)
        .cloned()
        .unwrap_or_else(|| model.provider_name.clone());
    let provider_synthesized = trace
        .captured::<bool>(TRACE.provider_synthesized)
        .is_some_and(|value| *value);
    let raw_base_url = trace
        .captured::<Option<String>>(TRACE.raw_base_url)
        .cloned()
        .flatten();
    let auth_material = trace
        .captured::<ResolvedModelAuthMaterial>(TRACE.auth_material)
        .cloned()
        .unwrap_or_default();

    let mut sources = trace.sources().clone();
    sources.insert(
        "model.effective".into(),
        source(
            InspectionSourceKind::Synthesized,
            "overrides merged into the raw record, then the Anthropic profile pass fills gaps",
        ),
    );
    sources.insert(
        "resolved".into(),
        source(
            InspectionSourceKind::Synthesized,
            "the assembled runtime view (Model) of this same resolution pass",
        ),
    );
    for field in [
        "maxContextSize",
        "maxOutputSize",
        "displayName",
        "reasoningKey",
        "supportEfforts",
        "defaultEffort",
        "aliases",
    ] {
        if let Some(value) = sources.get(&format!("model.effective.{field}")).cloned() {
            sources.insert(format!("resolved.{field}"), value);
        }
    }
    let effective_value = serde_json::to_value(&effective)?;
    let wire_name_field = if effective_value
        .as_object()
        .is_some_and(|record| record.contains_key("name"))
    {
        "name"
    } else {
        "model"
    };
    sources.insert(
        "resolved.wireName".into(),
        sources
            .get(&format!("model.effective.{wire_name_field}"))
            .cloned()
            .unwrap_or_else(|| source(InspectionSourceKind::Config, "[models.*] section")),
    );
    sources.insert(
        "resolved.alwaysThinking".into(),
        source(
            InspectionSourceKind::Synthesized,
            "derived from the declared capabilities ('always_thinking' present)",
        ),
    );
    sources.insert(
        "resolved.providerType".into(),
        if provider_config.is_some() {
            source(
                InspectionSourceKind::Config,
                &format!("provider '{provider_name}' type"),
            )
        } else {
            source(
                InspectionSourceKind::Synthesized,
                "no provider — falls back to the resolved protocol",
            )
        },
    );
    sources.insert(
        "resolved.providerName".into(),
        sources.get("provider").cloned().unwrap_or_else(|| {
            source(
                InspectionSourceKind::Config,
                &format!("provider '{provider_name}'"),
            )
        }),
    );
    sources.insert(
        "model".into(),
        source(InspectionSourceKind::Config, "the [models.*] section entry"),
    );
    sources.insert(
        "model.id".into(),
        source(InspectionSourceKind::Config, "the [models.*] section key"),
    );
    sources.insert(
        "resolved.headers".into(),
        source(
            InspectionSourceKind::Synthesized,
            "env < host < provider customHeaders merge (later wins)",
        ),
    );
    if let Some(base_url_source) = sources.get("resolved.baseUrl").cloned()
        && model.protocol == Protocol::Anthropic
        && raw_base_url.as_deref() != model.base_url.as_deref()
        && raw_base_url.is_some()
    {
        sources.insert(
            "resolved.baseUrl".into(),
            source(
                InspectionSourceKind::Synthesized,
                &format!(
                    "{} · trailing /v1 stripped",
                    base_url_source
                        .detail
                        .unwrap_or_else(|| source_kind_name(base_url_source.kind).into())
                ),
            ),
        );
    }

    attribute_capabilities(&mut sources, &configured, &effective, trace);
    attribute_headers(&mut sources, model, provider_config.as_ref(), trace)?;

    let definition = if let Some(provider) = &provider_config {
        sources.insert(
            "provider.config".into(),
            source(InspectionSourceKind::Config, "[providers.*] section"),
        );
        let definition = provider
            .provider_type
            .as_ref()
            .map(|provider_type| get_provider_definition(provider_type.as_str(), None))
            .transpose()?
            .flatten();
        sources.insert(
            "provider.definition".into(),
            source(
                InspectionSourceKind::Builtin,
                &definition.as_ref().map_or_else(
                    || {
                        format!(
                            "vendor '{}' is not registered in the provider-definition registry",
                            provider
                                .provider_type
                                .as_ref()
                                .map_or("", |kind| kind.as_str())
                        )
                    },
                    |definition| format!("provider definition '{}'", definition.id),
                ),
            ),
        );
        Some(InspectedProviderDefinition {
            registered: definition.is_some(),
            base_protocol: definition
                .as_ref()
                .map(|definition| definition.base_protocol),
            model_source: definition
                .as_ref()
                .and_then(|definition| definition.model_source),
            host_headers: definition.as_ref().and_then(|definition| {
                match definition.host_headers {
                    Some(HostHeaders::Full) => Some("full".into()),
                    Some(HostHeaders::UserAgent) => Some("user-agent".into()),
                    None => None,
                }
            }),
            endpoint: definition
                .as_ref()
                .and_then(|definition| definition.endpoint.as_ref())
                .map(endpoint_json),
        })
    } else {
        None
    };

    let auth = if let Some(api_key) = auth_material.api_key {
        InspectedAuth {
            kind: InspectedAuthKind::ApiKey,
            api_key: Some(mask_secret(&api_key)),
            oauth_provider_key: None,
        }
    } else if auth_material.oauth.is_some() {
        InspectedAuth {
            kind: InspectedAuthKind::OAuth,
            api_key: None,
            oauth_provider_key: auth_material.oauth_provider_key,
        }
    } else {
        InspectedAuth {
            kind: InspectedAuthKind::None,
            api_key: None,
            oauth_provider_key: None,
        }
    };

    let provider_config_value = provider_config.map(serde_json::to_value).transpose()?;

    Ok(ModelInspection {
        id: id.to_owned(),
        model: InspectedModel {
            id: id.to_owned(),
            record: redact_secrets(&serde_json::to_value(configured)?),
            effective: redact_secrets(&serde_json::to_value(effective)?),
        },
        provider: InspectedProvider {
            id: provider_name,
            synthesized: provider_synthesized,
            config: provider_config_value.map(|value| redact_secrets(&value)),
            definition,
        },
        resolved: InspectedResolvedModel {
            protocol: model.protocol,
            provider_type: model.provider_type.as_ref().map(ToString::to_string),
            provider_name: model.provider_name.clone(),
            base_url: model.base_url.clone(),
            wire_name: model.name.clone(),
            aliases: model.aliases.clone(),
            auth,
            capabilities: model.capabilities.clone(),
            max_context_size: model.max_context_size,
            max_output_size: model.max_output_size,
            display_name: model.display_name.clone(),
            reasoning_key: model.reasoning_key.clone(),
            support_efforts: model.support_efforts.clone(),
            default_effort: model.default_effort.clone(),
            always_thinking: model.always_thinking,
            headers: model.headers.clone(),
            provider_options: model.provider_options.clone(),
        },
        sources,
    })
}

fn attribute_capabilities(
    sources: &mut IndexMap<String, InspectionSource>,
    configured: &ModelRecord,
    effective: &ModelRecord,
    trace: &ResolutionTraceCollector,
) {
    let raw = capability_names(configured.capabilities.as_deref());
    let added = capability_names(effective.capabilities.as_deref());
    let detected = trace.captured::<ModelCapability>(TRACE.detected_capability);
    let detected_source = trace
        .captured::<InspectionSource>(TRACE.capability_source)
        .cloned()
        .unwrap_or_else(|| source(InspectionSourceKind::None, ""));
    let profile_source = sources.get("model.effective.capabilities").cloned();
    for key in CAPABILITY_KEYS {
        let path = format!("resolved.capabilities.{key}");
        let value = if raw.contains(key) || (key == "thinking" && raw.contains("always_thinking")) {
            source(
                InspectionSourceKind::Config,
                "declared in model capabilities",
            )
        } else if added.contains(key) || (key == "thinking" && added.contains("always_thinking")) {
            profile_source.clone().unwrap_or_else(|| {
                source(
                    InspectionSourceKind::Builtin,
                    "added by the Anthropic profile pass",
                )
            })
        } else if detected.is_some_and(|capability| capability_value(capability, key)) {
            detected_source.clone()
        } else {
            source(InspectionSourceKind::None, "neither declared nor detected")
        };
        sources.insert(path, value);
    }
    sources.insert(
        "resolved.capabilities.max_context_tokens".into(),
        source(
            InspectionSourceKind::Synthesized,
            "forced to the resolved maxContextSize",
        ),
    );
}

fn attribute_headers(
    sources: &mut IndexMap<String, InspectionSource>,
    model: &Model,
    provider: Option<&ProviderConfig>,
    trace: &ResolutionTraceCollector,
) -> Result<(), ProviderDefinitionRegistryError> {
    let env_layer = parse_kimi_code_custom_headers(&std::env::vars().collect());
    let raw_host = trace
        .captured::<IndexMap<String, String>>(TRACE.host_headers)
        .cloned()
        .unwrap_or_default();
    let forwards_all = provider
        .and_then(|provider| provider.provider_type.as_ref())
        .map(|provider_type| get_provider_definition(provider_type.as_str(), None))
        .transpose()?
        .flatten()
        .is_some_and(|definition| definition.host_headers == Some(HostHeaders::Full));
    let host_layer = if forwards_all {
        raw_host
    } else {
        raw_host
            .get("User-Agent")
            .map(|value| IndexMap::from([("User-Agent".into(), value.clone())]))
            .unwrap_or_default()
    };
    let custom_layer = provider.and_then(|provider| provider.custom_headers.as_ref());
    for key in model.headers.keys() {
        let value = if custom_layer.is_some_and(|headers| headers.contains_key(key)) {
            source(InspectionSourceKind::Config, "provider's customHeaders")
        } else if host_layer.contains_key(key) {
            source(
                InspectionSourceKind::Builtin,
                if forwards_all {
                    "host request headers (hostHeaders: 'full')"
                } else {
                    "host User-Agent"
                },
            )
        } else if env_layer.contains_key(key) {
            source(InspectionSourceKind::Env, "KIMI_CODE_CUSTOM_HEADERS")
        } else {
            continue;
        };
        sources.insert(format!("resolved.headers.{key}"), value);
    }
    Ok(())
}

fn required_capture<T: Clone + Send + Sync + 'static>(
    trace: &ResolutionTraceCollector,
    key: &'static str,
    description: &'static str,
) -> Result<T, InspectionAssemblyError> {
    trace
        .captured(key)
        .cloned()
        .ok_or(InspectionAssemblyError::MissingCapture(description))
}

fn source(kind: InspectionSourceKind, detail: &str) -> InspectionSource {
    InspectionSource {
        kind,
        detail: (!detail.is_empty()).then(|| detail.to_owned()),
    }
}

fn source_kind_name(kind: InspectionSourceKind) -> &'static str {
    match kind {
        InspectionSourceKind::Config => "config",
        InspectionSourceKind::Override => "override",
        InspectionSourceKind::Builtin => "builtin",
        InspectionSourceKind::Env => "env",
        InspectionSourceKind::Synthesized => "synthesized",
        InspectionSourceKind::None => "none",
    }
}

fn capability_names(names: Option<&[String]>) -> std::collections::HashSet<String> {
    names
        .unwrap_or_default()
        .iter()
        .map(|name| name.trim().to_ascii_lowercase())
        .collect()
}

fn capability_value(capability: &ModelCapability, key: &str) -> bool {
    match key {
        "image_in" => capability.image_in,
        "video_in" => capability.video_in,
        "audio_in" => capability.audio_in,
        "thinking" => capability.thinking,
        "tool_use" => capability.tool_use,
        "dynamically_loaded_tools" => capability.dynamically_loaded_tools == Some(true),
        _ => false,
    }
}

fn endpoint_json(endpoint: &crate::kosong::protocol::protocol_trait::ProtocolEndpoint) -> Value {
    let mut value = serde_json::Map::new();
    if let Some(api_key_env) = &endpoint.api_key_env {
        value.insert("apiKeyEnv".into(), Value::String(api_key_env.clone()));
    }
    if let Some(base_url_env) = &endpoint.base_url_env {
        value.insert("baseUrlEnv".into(), Value::String(base_url_env.clone()));
    }
    if let Some(default_base_url) = &endpoint.default_base_url {
        value.insert(
            "defaultBaseUrl".into(),
            Value::String(default_base_url.clone()),
        );
    }
    Value::Object(value)
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU64;

    use super::*;
    use crate::kosong::{contract::capability::UNKNOWN_CAPABILITY, provider::config::ProviderType};

    #[test]
    fn masks_and_redacts_only_secret_named_string_values() {
        assert_eq!(mask_secret("abcd"), "••••");
        assert_eq!(mask_secret("abcdef"), "••••cdef");
        assert_eq!(mask_secret("秘密密钥值"), "••••密密钥值");
        assert_eq!(
            redact_secrets(&serde_json::json!({
                "apiKey": "secret-value", "nested": {"authorization": "Bearer token"},
                "tokenCount": 3, "ordinary": "unchanged"
            })),
            serde_json::json!({
                "apiKey": "••••alue", "nested": {"authorization": "••••oken"},
                "tokenCount": 3, "ordinary": "unchanged"
            })
        );
    }

    #[test]
    fn effective_field_and_option_sources_follow_override_profile_and_env_rules() {
        let configured = ModelRecord {
            max_context_size: NonZeroU64::new(10),
            support_efforts: Some(vec!["low".into(), "high".into()]),
            overrides: Some(crate::kosong::model::contract::ModelRecordOverride {
                support_efforts: Some(vec!["low".into()]),
                ..Default::default()
            }),
            ..Default::default()
        };
        let effective = ModelRecord {
            max_context_size: NonZeroU64::new(10),
            support_efforts: Some(vec!["low".into()]),
            ..Default::default()
        };
        let mut trace = ResolutionTraceCollector::default();
        attribute_effective_fields(&mut trace, &configured, &effective, None, false).unwrap();
        attribute_provider_options(
            &mut trace,
            &ProtocolProviderOptions {
                support_efforts: Some(vec!["low".into()]),
                location: Some("us-central1".into()),
                ..Default::default()
            },
            Some(&IndexMap::from([(
                "GOOGLE_CLOUD_LOCATION".into(),
                "us-central1".into(),
            )])),
        )
        .unwrap();
        assert_eq!(
            trace.sources()["model.effective.supportEfforts"].kind,
            InspectionSourceKind::Override
        );
        assert_eq!(
            trace.sources()["resolved.providerOptions.supportEfforts"].kind,
            InspectionSourceKind::Override
        );
        assert_eq!(
            trace.sources()["resolved.providerOptions.location"].kind,
            InspectionSourceKind::Env
        );
    }

    #[test]
    fn assembles_a_redacted_same_pass_inspection_with_capability_attribution() {
        let configured = ModelRecord {
            name: Some("model-wire".into()),
            api_key: Some("secret-value".into()),
            capabilities: Some(vec!["thinking".into()]),
            max_context_size: NonZeroU64::new(32),
            ..Default::default()
        };
        let mut trace = ResolutionTraceCollector::default();
        trace.capture_value(TRACE.configured_model, configured.clone());
        trace.capture_value(TRACE.effective_model, configured.clone());
        trace.capture_value(TRACE.provider_config, Option::<ProviderConfig>::None);
        trace.capture_value(TRACE.provider_name, "flat.example.test".to_owned());
        trace.capture_value(TRACE.provider_synthesized, true);
        trace.capture_value(
            TRACE.raw_base_url,
            Some("https://flat.example.test/v1".to_owned()),
        );
        trace.capture_value(
            TRACE.auth_material,
            ResolvedModelAuthMaterial {
                api_key: Some("secret-value".into()),
                ..Default::default()
            },
        );
        trace.capture_value(TRACE.detected_capability, UNKNOWN_CAPABILITY);
        trace.capture_value(
            TRACE.capability_source,
            source(InspectionSourceKind::None, "no catalog match"),
        );
        trace.capture_value(TRACE.host_headers, IndexMap::<String, String>::new());
        trace.record(
            "provider",
            source(InspectionSourceKind::Synthesized, "flat model"),
        );
        trace.record(
            "resolved.baseUrl",
            source(InspectionSourceKind::Config, "model.baseUrl (flat)"),
        );
        attribute_effective_fields(&mut trace, &configured, &configured, None, false).unwrap();
        let model = Model {
            id: "model-id".into(),
            name: "model-wire".into(),
            aliases: vec![],
            protocol: Protocol::OpenAi,
            base_url: Some("https://flat.example.test/v1".into()),
            headers: IndexMap::new(),
            capabilities: ModelCapability {
                thinking: true,
                max_context_tokens: 32,
                ..UNKNOWN_CAPABILITY
            },
            max_context_size: 32,
            max_output_size: None,
            display_name: None,
            reasoning_key: None,
            support_efforts: None,
            default_effort: None,
            always_thinking: false,
            provider_type: Some(ProviderType::from("openai")),
            provider_name: "flat.example.test".into(),
            auth_provider: Arc::new(super::super::catalog::StaticAuthProvider::default()),
            provider_options: None,
        };
        let inspection = assemble_model_inspection("model-id", &model, &trace).unwrap();
        assert_eq!(inspection.model.record["apiKey"], "••••alue");
        assert_eq!(inspection.resolved.auth.kind, InspectedAuthKind::ApiKey);
        assert_eq!(
            inspection.resolved.auth.api_key.as_deref(),
            Some("••••alue")
        );
        assert!(inspection.provider.synthesized);
        assert_eq!(
            inspection.sources["resolved.capabilities.thinking"].kind,
            InspectionSourceKind::Config
        );
        assert_eq!(
            inspection.sources["resolved.capabilities.video_in"].kind,
            InspectionSourceKind::None
        );
    }
}
