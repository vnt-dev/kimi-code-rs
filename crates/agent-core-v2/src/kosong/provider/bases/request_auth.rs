use indexmap::IndexMap;
use std::sync::Arc;

use crate::kosong::contract::errors::ChatProviderError;
use crate::kosong::contract::provider::ProviderRequestAuth;

// Original:
//   packages/agent-core-v2/src/kosong/provider/bases/request-auth.ts
//   requireProviderApiKey()
pub fn require_provider_api_key(
    provider_name: &str,
    auth: Option<&ProviderRequestAuth>,
    default_api_key: Option<&str>,
) -> Result<String, ChatProviderError> {
    // An explicitly present empty request key overrides the default and then
    // fails, matching JavaScript's nullish-coalescing rather than truthiness.
    let api_key = auth
        .and_then(|auth| auth.api_key.as_deref())
        .or(default_api_key);
    match api_key {
        Some(api_key) if !api_key.is_empty() => Ok(api_key.to_owned()),
        _ => Err(ChatProviderError::ChatProvider {
            message: format!(
                "{provider_name}: apiKey is required. Provide it via the constructor options, the provider's API-key environment variable, options.auth.apiKey on each request, or an OAuth login."
            ),
        }),
    }
}

// Original: request-auth.ts, mergeRequestHeaders()
pub fn merge_request_headers(
    default_headers: Option<&IndexMap<String, String>>,
    request_headers: Option<&IndexMap<String, String>>,
) -> Option<IndexMap<String, String>> {
    let mut merged = IndexMap::new();
    if let Some(default_headers) = default_headers {
        merged.extend(default_headers.clone());
    }
    if let Some(request_headers) = request_headers {
        merged.extend(request_headers.clone());
    }
    (!merged.is_empty()).then_some(merged)
}

pub type ClientFactory<TClient> = Arc<dyn Fn(ProviderRequestAuth) -> TClient + Send + Sync>;

pub struct AuthBackedClientState<TClient> {
    pub cached_client: Option<Arc<TClient>>,
    pub client_factory: Option<ClientFactory<TClient>>,
}

impl<TClient> Default for AuthBackedClientState<TClient> {
    fn default() -> Self {
        Self {
            cached_client: None,
            client_factory: None,
        }
    }
}

// Original: request-auth.ts, resolveAuthBackedClient()
//
// Rust adaptation:
//   Arc preserves cached-client object identity while allowing the selected
//   client to leave this helper without cloning the SDK client itself.
pub fn resolve_auth_backed_client<TClient>(
    state: &AuthBackedClientState<TClient>,
    auth: Option<&ProviderRequestAuth>,
    build: impl FnOnce(Option<&ProviderRequestAuth>) -> TClient,
) -> Arc<TClient> {
    if let Some(factory) = state.client_factory.as_ref() {
        return Arc::new(factory(auth.cloned().unwrap_or_default()));
    }
    if auth.is_none()
        && let Some(cached_client) = state.cached_client.as_ref()
    {
        return Arc::clone(cached_client);
    }
    Arc::new(build(auth))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn request_key_precedes_default_and_empty_request_key_still_overrides() {
        let request = ProviderRequestAuth {
            api_key: Some("request-key".to_owned()),
            headers: None,
        };
        assert_eq!(
            require_provider_api_key("OpenAI", Some(&request), Some("default-key")).unwrap(),
            "request-key"
        );
        assert_eq!(
            require_provider_api_key("OpenAI", None, Some("default-key")).unwrap(),
            "default-key"
        );

        let empty = ProviderRequestAuth {
            api_key: Some(String::new()),
            headers: None,
        };
        let error =
            require_provider_api_key("OpenAI", Some(&empty), Some("default-key")).unwrap_err();
        assert_eq!(
            error.message(),
            "OpenAI: apiKey is required. Provide it via the constructor options, the provider's API-key environment variable, options.auth.apiKey on each request, or an OAuth login."
        );
    }

    #[test]
    fn header_merge_preserves_default_order_and_request_precedence() {
        let defaults = IndexMap::from([
            ("User-Agent".to_owned(), "kimi/1".to_owned()),
            ("X-Shared".to_owned(), "default".to_owned()),
        ]);
        let request = IndexMap::from([
            ("X-Shared".to_owned(), "request".to_owned()),
            ("Authorization".to_owned(), "Bearer token".to_owned()),
        ]);
        let merged = merge_request_headers(Some(&defaults), Some(&request)).unwrap();
        assert_eq!(
            merged.keys().map(String::as_str).collect::<Vec<_>>(),
            ["User-Agent", "X-Shared", "Authorization"]
        );
        assert_eq!(merged["X-Shared"], "request");
        assert_eq!(merge_request_headers(None, None), None);
    }

    #[test]
    fn factory_always_wins_and_receives_empty_auth_when_absent() {
        let factory_calls = Arc::new(AtomicUsize::new(0));
        let calls = Arc::clone(&factory_calls);
        let state = AuthBackedClientState {
            cached_client: Some(Arc::new("cached".to_owned())),
            client_factory: Some(Arc::new(move |auth: ProviderRequestAuth| {
                calls.fetch_add(1, Ordering::SeqCst);
                auth.api_key.unwrap_or_else(|| "factory-empty".to_owned())
            }) as ClientFactory<String>),
        };
        let build_calls = AtomicUsize::new(0);
        let client = resolve_auth_backed_client(&state, None, |_| {
            build_calls.fetch_add(1, Ordering::SeqCst);
            "built".to_owned()
        });
        assert_eq!(client.as_str(), "factory-empty");
        assert_eq!(factory_calls.load(Ordering::SeqCst), 1);
        assert_eq!(build_calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn cached_client_is_reused_only_when_request_auth_is_absent() {
        let cached = Arc::new("cached".to_owned());
        let state = AuthBackedClientState {
            cached_client: Some(Arc::clone(&cached)),
            client_factory: None,
        };
        let reused = resolve_auth_backed_client(&state, None, |_| "built".to_owned());
        assert!(Arc::ptr_eq(&cached, &reused));

        let explicit_empty = ProviderRequestAuth::default();
        let rebuilt =
            resolve_auth_backed_client(&state, Some(&explicit_empty), |_| "rebuilt".to_owned());
        assert!(!Arc::ptr_eq(&cached, &rebuilt));
        assert_eq!(rebuilt.as_str(), "rebuilt");
    }

    #[test]
    fn build_is_used_when_no_factory_or_reusable_cache_exists() {
        let state = AuthBackedClientState::<String>::default();
        let auth = ProviderRequestAuth {
            api_key: Some("key".to_owned()),
            headers: None,
        };
        let built = resolve_auth_backed_client(&state, Some(&auth), |auth| {
            auth.unwrap().api_key.clone().unwrap()
        });
        assert_eq!(built.as_str(), "key");
    }
}
