use url::{Position, Url};

use crate::{
    _base::errors::errors::Error2,
    app::config::{CONFIG_INVALID, ensure_config_errors_registered},
    kosong::{
        contract::inspection::{InspectionSource, InspectionSourceKind, ResolutionTrace},
        model::{contract::ModelRecord, types::ResolvedModelAuthMaterial},
        provider::{
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

// MIGRATION-TODO:
// Original: packages/agent-core-v2/src/kosong/model/modelAuth.ts
// Missing unit: effectiveModelConfig() and its Anthropic profile fold.
// Completion condition: migrate the registry-driven thinking verdict and port
// override/profile composition without vendor string gates.

#[cfg(test)]
mod tests {
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
}
