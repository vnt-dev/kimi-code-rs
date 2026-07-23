use url::{Position, Url};

use crate::{
    _base::errors::errors::Error2,
    app::config::{CONFIG_INVALID, ensure_config_errors_registered},
    kosong::{
        contract::inspection::{InspectionSource, InspectionSourceKind, ResolutionTrace},
        model::{
            contract::{ModelRecord, ModelRecordOverride},
            thinking::drives_thinking_through_traits,
            types::ResolvedModelAuthMaterial,
        },
        provider::{
            bases::anthropic::anthropic_profile::{
                BUDGET_THINKING_EFFORTS, infer_anthropic_model_profile,
                match_known_anthropic_model_profile,
            },
            config::ProviderConfig,
            provider_definition::{
                ExplainedProviderEndpoint, ProviderDefinitionRegistryError,
                explain_provider_endpoint,
            },
        },
    },
};

#[derive(Debug, thiserror::Error)]
pub enum ModelAuthError {
    #[error(transparent)]
    InvalidConfig(Box<Error2>),
    #[error(transparent)]
    ProviderDefinition(#[from] ProviderDefinitionRegistryError),
}

pub struct ResolveModelAuthArgs<'a> {
    pub model_id: &'a str,
    pub model: &'a ModelRecord,
    pub provider: Option<&'a ProviderConfig>,
    pub provider_name: &'a str,
}

// Original: modelAuth.ts, resolveModelAuthMaterial().
pub fn resolve_model_auth_material(
    args: ResolveModelAuthArgs<'_>,
    mut trace: Option<&mut dyn ResolutionTrace>,
) -> Result<ResolvedModelAuthMaterial, ModelAuthError> {
    let model_api_key = non_empty(args.model.api_key.as_deref()).map(str::to_owned);
    if model_api_key.is_some() && args.model.oauth.is_some() {
        return Err(ModelAuthError::InvalidConfig(Box::new(
            auth_conflict_error("Model", args.model_id),
        )));
    }
    if let Some(api_key) = model_api_key {
        record_auth_source(
            &mut trace,
            InspectionSourceKind::Config,
            "model.apiKey".into(),
        );
        return Ok(ResolvedModelAuthMaterial {
            api_key: Some(api_key),
            ..ResolvedModelAuthMaterial::default()
        });
    }
    if let Some(oauth) = &args.model.oauth {
        record_auth_source(
            &mut trace,
            InspectionSourceKind::Config,
            "model.oauth".into(),
        );
        return Ok(ResolvedModelAuthMaterial {
            oauth: Some(oauth.clone()),
            oauth_provider_key: model_provider_key(args.model),
            ..ResolvedModelAuthMaterial::default()
        });
    }

    let provider_auth_type = args
        .provider
        .and_then(|provider| provider.provider_type.as_ref())
        .map(|provider_type| provider_type.as_str())
        .or_else(|| args.model.protocol.map(|protocol| protocol.as_str()));
    let empty_env = indexmap::IndexMap::new();
    let provider_endpoint = match provider_auth_type {
        Some(provider_type) => explain_provider_endpoint(
            provider_type,
            args.provider
                .and_then(|provider| provider.env.as_ref())
                .unwrap_or(&empty_env),
        )?,
        None => ExplainedProviderEndpoint::default(),
    };
    let configured_provider_api_key = args
        .provider
        .and_then(|provider| non_empty(provider.api_key.as_deref()))
        .map(str::to_owned);
    let provider_api_key = configured_provider_api_key
        .clone()
        .or(provider_endpoint.api_key.clone());
    if provider_api_key.is_some()
        && args
            .provider
            .and_then(|provider| provider.oauth.as_ref())
            .is_some()
    {
        return Err(ModelAuthError::InvalidConfig(Box::new(
            auth_conflict_error("Provider", args.provider_name),
        )));
    }
    if let Some(api_key) = provider_api_key {
        let (kind, detail) = if configured_provider_api_key.is_some() {
            (
                InspectionSourceKind::Config,
                format!("provider '{}' apiKey", args.provider_name),
            )
        } else {
            (
                InspectionSourceKind::Env,
                format!(
                    "{} (provider '{}' env bag)",
                    provider_endpoint.api_key_env_name.as_deref().unwrap_or("?"),
                    args.provider_name
                ),
            )
        };
        record_auth_source(&mut trace, kind, detail);
        return Ok(ResolvedModelAuthMaterial {
            api_key: Some(api_key),
            ..ResolvedModelAuthMaterial::default()
        });
    }
    if let Some(oauth) = args.provider.and_then(|provider| provider.oauth.as_ref()) {
        record_auth_source(
            &mut trace,
            InspectionSourceKind::Config,
            format!("provider '{}' oauth", args.provider_name),
        );
        return Ok(ResolvedModelAuthMaterial {
            oauth: Some(oauth.clone()),
            oauth_provider_key: model_provider_key(args.model),
            ..ResolvedModelAuthMaterial::default()
        });
    }
    record_auth_source(
        &mut trace,
        InspectionSourceKind::None,
        "no credential resolved at any layer (adapter construction may still read process.env)"
            .into(),
    );
    Ok(ResolvedModelAuthMaterial::default())
}

// Original: modelAuth.ts, effectiveModelConfig().
pub fn effective_model_config(
    model: &ModelRecord,
    provider_type: Option<&str>,
) -> Result<ModelRecord, ProviderDefinitionRegistryError> {
    let mut effective = model.clone();
    if let Some(overrides) = &model.overrides {
        effective.overrides = None;
        apply_model_overrides(&mut effective, overrides);
        if overrides.support_efforts.is_some()
            && overrides.default_effort.is_none()
            && effective.default_effort.as_ref().is_some_and(|default| {
                !overrides
                    .support_efforts
                    .as_ref()
                    .is_some_and(|efforts| efforts.contains(default))
            })
        {
            effective.default_effort = None;
        }
    }
    with_anthropic_profile(effective, provider_type)
}

fn apply_model_overrides(model: &mut ModelRecord, overrides: &ModelRecordOverride) {
    if let Some(value) = overrides.max_context_size {
        model.max_context_size = Some(value);
    }
    if let Some(value) = overrides.max_output_size {
        model.max_output_size = Some(value);
    }
    if let Some(value) = &overrides.capabilities {
        model.capabilities = Some(value.clone());
    }
    if let Some(value) = &overrides.display_name {
        model.display_name = Some(value.clone());
    }
    if let Some(value) = &overrides.reasoning_key {
        model.reasoning_key = Some(value.clone());
    }
    if let Some(value) = overrides.adaptive_thinking {
        model.adaptive_thinking = Some(value);
    }
    if let Some(value) = &overrides.support_efforts {
        model.support_efforts = Some(value.clone());
    }
    if let Some(value) = &overrides.default_effort {
        model.default_effort = Some(value.clone());
    }
}

// Original: modelAuth.ts, withAnthropicProfile().
fn with_anthropic_profile(
    mut model: ModelRecord,
    provider_type: Option<&str>,
) -> Result<ModelRecord, ProviderDefinitionRegistryError> {
    let Some(wire_name) = model.name.as_deref().or(model.model.as_deref()) else {
        return Ok(model);
    };
    let protocol_is_anthropic = model.protocol
        == Some(crate::kosong::protocol::identity::Protocol::Anthropic)
        || (model.protocol.is_none() && provider_type == Some("anthropic"));
    let profile = if provider_type.is_some()
        && !drives_thinking_through_traits(provider_type)?
        && protocol_is_anthropic
    {
        Some(infer_anthropic_model_profile(wire_name))
    } else {
        match_known_anthropic_model_profile(wire_name)
    };
    let Some(profile) = profile else {
        return Ok(model);
    };

    let capability = if profile.can_disable_thinking {
        "thinking"
    } else {
        "always_thinking"
    };
    let capabilities = model.capabilities.get_or_insert_with(Vec::new);
    if !capabilities
        .iter()
        .any(|candidate| candidate.trim().eq_ignore_ascii_case(capability))
    {
        capabilities.push(capability.into());
    }
    let adaptive_thinking = model.adaptive_thinking;
    let support_efforts = model.support_efforts.get_or_insert_with(|| {
        let efforts = if adaptive_thinking == Some(false) {
            BUDGET_THINKING_EFFORTS
        } else {
            profile.efforts
        };
        efforts.iter().map(|effort| (*effort).to_owned()).collect()
    });
    if model.default_effort.is_none() && support_efforts.iter().any(|effort| effort == "high") {
        model.default_effort = Some("high".into());
    }
    Ok(model)
}

fn model_provider_key(model: &ModelRecord) -> Option<String> {
    model.provider_id.clone().or_else(|| model.provider.clone())
}

fn record_auth_source(
    trace: &mut Option<&mut dyn ResolutionTrace>,
    kind: InspectionSourceKind,
    detail: String,
) {
    if let Some(trace) = trace.as_deref_mut() {
        trace.record(
            "resolved.auth",
            InspectionSource {
                kind,
                detail: Some(detail),
            },
        );
    }
}

fn auth_conflict_error(kind: &str, name: &str) -> Error2 {
    ensure_config_errors_registered();
    Error2::new(
        CONFIG_INVALID,
        format!(
            "{kind} \"{name}\" has both apiKey and oauth set in config.toml - they are mutually exclusive. Remove one."
        ),
    )
}

// Original:
//   packages/agent-core-v2/src/kosong/model/modelAuth.ts
//   deriveProviderId()
//
// Rust adaptation:
//   The Position slice includes an explicit non-default port, matching the
//   JavaScript URL.host property. Url::host_str() alone would incorrectly
//   omit that port.
pub fn derive_provider_id(base_url: &str) -> String {
    match Url::parse(base_url) {
        Ok(url) => url[Position::BeforeHost..Position::AfterPort].to_owned(),
        Err(_) => base_url.to_owned(),
    }
}

// Original:
//   packages/agent-core-v2/src/kosong/model/modelAuth.ts
//   nonEmpty()
pub fn non_empty(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU64;

    use indexmap::IndexMap;

    use super::*;
    use crate::kosong::{
        contract::inspection::CapturedResolutionValue,
        protocol::{identity::Protocol, protocol_trait::ProtocolEndpoint},
        provider::{
            config::{OAuthRef, OAuthStorage, ProviderType},
            provider_definition::{ProviderDefinition, register_provider_definition},
        },
    };

    #[derive(Default)]
    struct Trace(Vec<(String, InspectionSource)>);

    impl ResolutionTrace for Trace {
        fn record(&mut self, path: &str, source: InspectionSource) {
            self.0.push((path.into(), source));
        }

        fn capture(&mut self, _: &str, _: CapturedResolutionValue) {}
    }

    fn oauth(key: &str) -> OAuthRef {
        OAuthRef::new(OAuthStorage::Keyring, key, None).unwrap()
    }

    fn args<'a>(
        model: &'a ModelRecord,
        provider: Option<&'a ProviderConfig>,
    ) -> ResolveModelAuthArgs<'a> {
        ResolveModelAuthArgs {
            model_id: "model-id",
            model,
            provider,
            provider_name: "provider-name",
        }
    }

    fn ensure_auth_test_provider() {
        static REGISTERED: std::sync::OnceLock<()> = std::sync::OnceLock::new();
        REGISTERED.get_or_init(|| {
            register_provider_definition(ProviderDefinition {
                id: "model-auth-test-provider".into(),
                base_protocol: Protocol::OpenAi,
                traits: vec![],
                endpoint: Some(ProtocolEndpoint {
                    api_key_env: Some("MODEL_AUTH_TEST_KEY".into()),
                    base_url_env: None,
                    default_base_url: None,
                }),
                host_headers: None,
                model_source: None,
                capability: None,
            })
            .unwrap();
        });
    }

    #[test]
    fn provider_id_is_the_parsed_url_host() {
        assert_eq!(
            derive_provider_id("https://api.example.test/v1"),
            "api.example.test"
        );
        assert_eq!(
            derive_provider_id("https://api.example.test:8443/v1"),
            "api.example.test:8443"
        );
        assert_eq!(
            derive_provider_id("https://user:secret@api.example.test:8443/v1"),
            "api.example.test:8443"
        );
    }

    #[test]
    fn provider_id_preserves_unparseable_input_verbatim() {
        assert_eq!(derive_provider_id("not-a-url"), "not-a-url");
        assert_eq!(derive_provider_id(""), "");
        assert_eq!(derive_provider_id(" relative/path "), " relative/path ");
    }

    #[test]
    fn provider_id_matches_url_normalization_edge_cases() {
        assert_eq!(
            derive_provider_id("HTTPS://EXAMPLE.COM:443/path"),
            "example.com"
        );
        assert_eq!(derive_provider_id("https://[::1]:8080/v1"), "[::1]:8080");
        assert_eq!(derive_provider_id("mailto:user@example.com"), "");
    }

    #[test]
    fn non_empty_trims_and_filters_empty_values() {
        assert_eq!(non_empty(None), None);
        assert_eq!(non_empty(Some("")), None);
        assert_eq!(non_empty(Some(" \t\n ")), None);
        assert_eq!(non_empty(Some("  key  ")), Some("key"));
    }

    #[test]
    fn model_credentials_win_and_conflicts_preserve_config_error() {
        let provider = ProviderConfig {
            api_key: Some("provider-key".into()),
            ..ProviderConfig::default()
        };
        let model = ModelRecord {
            api_key: Some("  model-key  ".into()),
            ..ModelRecord::default()
        };
        let mut trace = Trace::default();
        assert_eq!(
            resolve_model_auth_material(args(&model, Some(&provider)), Some(&mut trace)).unwrap(),
            ResolvedModelAuthMaterial {
                api_key: Some("model-key".into()),
                ..ResolvedModelAuthMaterial::default()
            }
        );
        assert_eq!(trace.0[0].0, "resolved.auth");
        assert_eq!(trace.0[0].1.kind, InspectionSourceKind::Config);
        assert_eq!(trace.0[0].1.detail.as_deref(), Some("model.apiKey"));

        let conflicting = ModelRecord {
            api_key: Some("key".into()),
            oauth: Some(oauth("model")),
            ..ModelRecord::default()
        };
        let error = resolve_model_auth_material(args(&conflicting, None), None).unwrap_err();
        let ModelAuthError::InvalidConfig(error) = error else {
            panic!("expected config error")
        };
        assert_eq!(error.code, CONFIG_INVALID);
        assert_eq!(
            error.message,
            "Model \"model-id\" has both apiKey and oauth set in config.toml - they are mutually exclusive. Remove one."
        );
    }

    #[test]
    fn oauth_uses_structured_or_legacy_model_provider_key() {
        let model = ModelRecord {
            provider_id: Some("structured".into()),
            provider: Some("legacy".into()),
            oauth: Some(oauth("model")),
            ..ModelRecord::default()
        };
        let resolved = resolve_model_auth_material(args(&model, None), None).unwrap();
        assert_eq!(resolved.oauth.unwrap().key, "model");
        assert_eq!(resolved.oauth_provider_key.as_deref(), Some("structured"));

        let model = ModelRecord {
            provider: Some("legacy".into()),
            ..ModelRecord::default()
        };
        let provider = ProviderConfig {
            oauth: Some(oauth("provider")),
            ..ProviderConfig::default()
        };
        let resolved = resolve_model_auth_material(args(&model, Some(&provider)), None).unwrap();
        assert_eq!(resolved.oauth.unwrap().key, "provider");
        assert_eq!(resolved.oauth_provider_key.as_deref(), Some("legacy"));
    }

    #[test]
    fn provider_config_precedes_env_bag_and_env_trace_names_the_variable() {
        ensure_auth_test_provider();
        let model = ModelRecord::default();
        let provider = ProviderConfig {
            provider_type: Some(ProviderType::new("model-auth-test-provider")),
            api_key: Some(" configured ".into()),
            env: Some(IndexMap::from([(
                "MODEL_AUTH_TEST_KEY".into(),
                "environment".into(),
            )])),
            ..ProviderConfig::default()
        };
        let resolved = resolve_model_auth_material(args(&model, Some(&provider)), None).unwrap();
        assert_eq!(resolved.api_key.as_deref(), Some("configured"));

        let provider = ProviderConfig {
            api_key: None,
            ..provider
        };
        let mut trace = Trace::default();
        let resolved =
            resolve_model_auth_material(args(&model, Some(&provider)), Some(&mut trace)).unwrap();
        assert_eq!(resolved.api_key.as_deref(), Some("environment"));
        assert_eq!(trace.0[0].1.kind, InspectionSourceKind::Env);
        assert_eq!(
            trace.0[0].1.detail.as_deref(),
            Some("MODEL_AUTH_TEST_KEY (provider 'provider-name' env bag)")
        );
    }

    #[test]
    fn provider_conflict_includes_env_key_and_no_credentials_records_none() {
        ensure_auth_test_provider();
        let model = ModelRecord::default();
        let provider = ProviderConfig {
            provider_type: Some(ProviderType::new("model-auth-test-provider")),
            oauth: Some(oauth("provider")),
            env: Some(IndexMap::from([(
                "MODEL_AUTH_TEST_KEY".into(),
                "environment".into(),
            )])),
            ..ProviderConfig::default()
        };
        let error = resolve_model_auth_material(args(&model, Some(&provider)), None).unwrap_err();
        assert!(matches!(error, ModelAuthError::InvalidConfig(_)));

        let mut trace = Trace::default();
        assert_eq!(
            resolve_model_auth_material(args(&model, None), Some(&mut trace)).unwrap(),
            ResolvedModelAuthMaterial::default()
        );
        assert_eq!(trace.0[0].1.kind, InspectionSourceKind::None);
    }

    #[test]
    fn effective_config_applies_only_override_fields_and_clears_stale_default() {
        let model = ModelRecord {
            provider_id: Some("provider-id".into()),
            name: Some("not-an-anthropic-model".into()),
            max_context_size: NonZeroU64::new(100),
            display_name: Some("base".into()),
            support_efforts: Some(vec!["low".into(), "high".into()]),
            default_effort: Some("high".into()),
            overrides: Some(ModelRecordOverride {
                max_context_size: NonZeroU64::new(200),
                display_name: Some("override".into()),
                support_efforts: Some(vec!["low".into()]),
                ..ModelRecordOverride::default()
            }),
            ..ModelRecord::default()
        };

        let effective = effective_model_config(&model, None).unwrap();
        assert_eq!(effective.provider_id.as_deref(), Some("provider-id"));
        assert_eq!(effective.max_context_size, NonZeroU64::new(200));
        assert_eq!(effective.display_name.as_deref(), Some("override"));
        assert_eq!(effective.support_efforts, Some(vec!["low".into()]));
        assert_eq!(effective.default_effort, None);
        assert_eq!(effective.overrides, None);
        assert!(model.overrides.is_some());
    }

    #[test]
    fn anthropic_profile_preserves_declared_values_and_adds_missing_defaults() {
        let model = ModelRecord {
            protocol: Some(Protocol::Anthropic),
            name: Some("claude-opus-4-6".into()),
            capabilities: Some(vec![" THINKING ".into(), "image_in".into()]),
            adaptive_thinking: Some(false),
            ..ModelRecord::default()
        };
        let effective = effective_model_config(&model, None).unwrap();
        assert_eq!(
            effective.capabilities,
            Some(vec![" THINKING ".into(), "image_in".into()])
        );
        assert_eq!(
            effective.support_efforts,
            Some(vec!["low".into(), "medium".into(), "high".into()])
        );
        assert_eq!(effective.default_effort.as_deref(), Some("high"));

        let declared = ModelRecord {
            protocol: Some(Protocol::Anthropic),
            name: Some("claude-opus-4-6".into()),
            support_efforts: Some(vec!["custom".into()]),
            default_effort: Some("custom".into()),
            ..ModelRecord::default()
        };
        let effective = effective_model_config(&declared, None).unwrap();
        assert_eq!(effective.support_efforts, Some(vec!["custom".into()]));
        assert_eq!(effective.default_effort.as_deref(), Some("custom"));
    }

    #[test]
    fn unknown_anthropic_models_are_inferred_only_for_non_trait_driven_providers() {
        static REGISTERED: std::sync::OnceLock<()> = std::sync::OnceLock::new();
        REGISTERED.get_or_init(|| {
            register_provider_definition(ProviderDefinition {
                id: "model-auth-trait-thinking".into(),
                base_protocol: Protocol::Anthropic,
                traits: vec![std::sync::Arc::new(
                    crate::kosong::protocol::protocol_trait::ProtocolTrait {
                        with_thinking: Some(std::sync::Arc::new(|_, _, _, _| None)),
                        ..Default::default()
                    },
                )],
                endpoint: None,
                host_headers: None,
                model_source: None,
                capability: None,
            })
            .unwrap();
        });
        let model = ModelRecord {
            protocol: Some(Protocol::Anthropic),
            name: Some("future-model-without-known-profile".into()),
            ..ModelRecord::default()
        };

        let inferred = effective_model_config(&model, Some("plain-provider")).unwrap();
        assert_eq!(inferred.capabilities, Some(vec!["thinking".to_owned()]));
        assert!(
            inferred
                .support_efforts
                .as_ref()
                .is_some_and(|efforts| efforts.iter().any(|effort| effort == "xhigh"))
        );

        let trait_driven =
            effective_model_config(&model, Some("model-auth-trait-thinking")).unwrap();
        assert_eq!(trait_driven.capabilities, None);
        assert_eq!(trait_driven.support_efforts, None);
        assert_eq!(trait_driven.default_effort, None);
    }
}
