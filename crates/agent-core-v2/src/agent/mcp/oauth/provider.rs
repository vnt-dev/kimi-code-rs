//! Persisted MCP OAuth provider state.
//!
//! Original: `agent/mcp/oauth/provider.ts`, `McpOAuthClientProvider`.

use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::OnceCell;
use url::Url;

use super::{
    McpOAuthStore, McpOAuthStoreKeyError, canonical_mcp_oauth_resource, mcp_oauth_store_key,
};
use crate::{_base::utils::hash::encode_hex, persistence::interface::storage::StorageError};

pub const MCP_OAUTH_TOKENS_SUFFIX: &str = "-tokens.json";
pub const MCP_OAUTH_CLIENT_SUFFIX: &str = "-client.json";
pub const MCP_OAUTH_DISCOVERY_SUFFIX: &str = "-discovery.json";
pub const PASSIVE_OAUTH_REDIRECT_URI: &str = "http://127.0.0.1:3118/callback";

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct McpOAuthClientMetadata {
    pub redirect_uris: Vec<String>,
    pub token_endpoint_auth_method: String,
    pub grant_types: Vec<String>,
    pub response_types: Vec<String>,
    pub client_name: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum McpOAuthInvalidationScope {
    All,
    Client,
    Tokens,
    Verifier,
    Discovery,
}

#[derive(Debug, thiserror::Error)]
pub enum McpOAuthProviderError {
    #[error(transparent)]
    InvalidStoreKey(#[from] McpOAuthStoreKeyError),

    #[error(transparent)]
    InvalidServerUrl(#[from] url::ParseError),

    #[error("McpOAuthClientProvider: PKCE code verifier not initialized")]
    MissingCodeVerifier,

    #[error("failed to generate OAuth state: {0}")]
    Random(#[from] getrandom::Error),

    #[error(transparent)]
    Storage(#[from] StorageError),
}

#[derive(Default)]
struct ProviderCache {
    client: Option<Value>,
    tokens: Option<Value>,
    discovery: Option<Value>,
    redirect_url: Option<String>,
    code_verifier: Option<String>,
    state: Option<String>,
    last_authorization_url: Option<String>,
}

/// One provider instance is scoped to one MCP server/resource identity.
///
/// The source starts loading its three persisted records in the constructor.
/// Rust initializes that same cache before each asynchronous data operation;
/// the synchronous redirect and metadata accessors only inspect the current
/// in-memory cache, matching the source's non-blocking getter contract.
pub struct McpOAuthClientProvider {
    pub store_key: String,
    pub server_url: String,
    store: Arc<dyn McpOAuthStore>,
    client_label: String,
    cache: Mutex<ProviderCache>,
    loaded: OnceCell<()>,
}

impl McpOAuthClientProvider {
    pub fn new(
        server_name: &str,
        server_url: &str,
        store: Arc<dyn McpOAuthStore>,
        client_label: Option<String>,
    ) -> Result<Self, McpOAuthProviderError> {
        let server_url = canonical_mcp_oauth_resource(server_url)?;
        let store_key = mcp_oauth_store_key(server_name, &server_url)?;
        Ok(Self {
            store_key,
            server_url,
            store,
            client_label: client_label.unwrap_or_else(|| format!("kimi-code ({server_name})")),
            cache: Mutex::new(ProviderCache::default()),
            loaded: OnceCell::new(),
        })
    }

    // Original: ready / load().
    pub async fn ready(&self) {
        self.loaded
            .get_or_init(|| async {
                let client_key = self.client_key();
                let tokens_key = self.tokens_key();
                let discovery_key = self.discovery_key();
                let (client, tokens, discovery) = tokio::join!(
                    self.store.read_value(&client_key),
                    self.store.read_value(&tokens_key),
                    self.store.read_value(&discovery_key),
                );
                let mut cache = self.cache.lock().unwrap();
                cache.client = client;
                cache.tokens = tokens;
                cache.discovery = discovery;
            })
            .await;
    }

    // Original: setRedirectUrl().
    pub fn set_redirect_url(&self, url: &Url) {
        self.cache.lock().unwrap().redirect_url = Some(url.to_string());
    }

    // Original: takeAuthorizationUrl().
    pub fn take_authorization_url(&self) -> Option<Url> {
        self.cache
            .lock()
            .unwrap()
            .last_authorization_url
            .take()
            .and_then(|url| Url::parse(&url).ok())
    }

    // Original: expectedState().
    pub fn expected_state(&self) -> Option<String> {
        self.cache.lock().unwrap().state.clone()
    }

    /// Rust OAuth transport adaptation: the RMCP authorization manager owns
    /// CSRF generation, so retain its generated state for the callback check
    /// performed by `McpOAuthService.complete`.
    pub fn set_expected_state(&self, state: String) {
        self.cache.lock().unwrap().state = Some(state);
    }

    // Original: resetFlow().
    pub fn reset_flow(&self) {
        let mut cache = self.cache.lock().unwrap();
        cache.redirect_url = None;
        cache.code_verifier = None;
        cache.state = None;
        cache.last_authorization_url = None;
    }

    // Original: redirectUrl getter.
    pub fn redirect_url(&self) -> String {
        self.effective_redirect_uri()
    }

    // Original: clientMetadata getter.
    pub fn client_metadata(&self) -> McpOAuthClientMetadata {
        McpOAuthClientMetadata {
            redirect_uris: vec![self.effective_redirect_uri()],
            token_endpoint_auth_method: "none".into(),
            grant_types: vec!["authorization_code".into(), "refresh_token".into()],
            response_types: vec!["code".into()],
            client_name: self.client_label.clone(),
        }
    }

    // Original: state().
    pub fn state(&self) -> Result<String, McpOAuthProviderError> {
        let mut cache = self.cache.lock().unwrap();
        if let Some(state) = &cache.state {
            return Ok(state.clone());
        }
        let mut bytes = [0_u8; 16];
        getrandom::fill(&mut bytes)?;
        let state = encode_hex(bytes);
        cache.state = Some(state.clone());
        Ok(state)
    }

    // Original: clientInformation().
    pub async fn client_information(&self) -> Option<Value> {
        self.ready().await;
        self.cache.lock().unwrap().client.clone()
    }

    // Original: saveClientInformation().
    pub async fn save_client_information(&self, info: Value) -> Result<(), McpOAuthProviderError> {
        self.cache.lock().unwrap().client = Some(info.clone());
        self.store.write_value(&self.client_key(), info).await?;
        Ok(())
    }

    // Original: tokens().
    pub async fn tokens(&self) -> Option<Value> {
        self.ready().await;
        self.cache.lock().unwrap().tokens.clone()
    }

    // Original: saveTokens().
    pub async fn save_tokens(&self, tokens: Value) -> Result<(), McpOAuthProviderError> {
        self.cache.lock().unwrap().tokens = Some(tokens.clone());
        self.store.write_value(&self.tokens_key(), tokens).await?;
        Ok(())
    }

    // Original: redirectToAuthorization().
    pub fn redirect_to_authorization(&self, url: &Url) {
        self.cache.lock().unwrap().last_authorization_url = Some(url.to_string());
    }

    // Original: saveCodeVerifier().
    pub fn save_code_verifier(&self, code_verifier: String) {
        self.cache.lock().unwrap().code_verifier = Some(code_verifier);
    }

    // Original: codeVerifier().
    pub fn code_verifier(&self) -> Result<String, McpOAuthProviderError> {
        self.cache
            .lock()
            .unwrap()
            .code_verifier
            .clone()
            .ok_or(McpOAuthProviderError::MissingCodeVerifier)
    }

    // Original: saveDiscoveryState().
    pub async fn save_discovery_state(
        &self,
        discovery: Value,
    ) -> Result<(), McpOAuthProviderError> {
        self.cache.lock().unwrap().discovery = Some(discovery.clone());
        self.store
            .write_value(&self.discovery_key(), discovery)
            .await?;
        Ok(())
    }

    // Original: discoveryState().
    pub async fn discovery_state(&self) -> Option<Value> {
        self.ready().await;
        self.cache.lock().unwrap().discovery.clone()
    }

    // Original: invalidateCredentials().
    pub async fn invalidate_credentials(
        &self,
        scope: McpOAuthInvalidationScope,
    ) -> Result<(), McpOAuthProviderError> {
        if scope == McpOAuthInvalidationScope::Verifier {
            self.cache.lock().unwrap().code_verifier = None;
            return Ok(());
        }
        if matches!(
            scope,
            McpOAuthInvalidationScope::Tokens | McpOAuthInvalidationScope::All
        ) {
            self.cache.lock().unwrap().tokens = None;
            self.store.remove(&self.tokens_key()).await?;
        }
        if matches!(
            scope,
            McpOAuthInvalidationScope::Client | McpOAuthInvalidationScope::All
        ) {
            self.cache.lock().unwrap().client = None;
            self.store.remove(&self.client_key()).await?;
        }
        if matches!(
            scope,
            McpOAuthInvalidationScope::Discovery | McpOAuthInvalidationScope::All
        ) {
            self.cache.lock().unwrap().discovery = None;
            self.store.remove(&self.discovery_key()).await?;
        }
        if scope == McpOAuthInvalidationScope::All {
            self.cache.lock().unwrap().code_verifier = None;
        }
        Ok(())
    }

    fn effective_redirect_uri(&self) -> String {
        let cache = self.cache.lock().unwrap();
        if let Some(url) = &cache.redirect_url {
            return url.clone();
        }
        registered_redirect_uri(cache.client.as_ref())
            .unwrap_or_else(|| PASSIVE_OAUTH_REDIRECT_URI.into())
    }

    fn tokens_key(&self) -> String {
        format!("{}{}", self.store_key, MCP_OAUTH_TOKENS_SUFFIX)
    }

    fn client_key(&self) -> String {
        format!("{}{}", self.store_key, MCP_OAUTH_CLIENT_SUFFIX)
    }

    fn discovery_key(&self) -> String {
        format!("{}{}", self.store_key, MCP_OAUTH_DISCOVERY_SUFFIX)
    }
}

fn registered_redirect_uri(info: Option<&Value>) -> Option<String> {
    info?
        .get("redirect_uris")?
        .as_array()?
        .first()?
        .as_str()
        .map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use std::{collections::HashMap, sync::Mutex};

    use async_trait::async_trait;

    use super::*;

    #[derive(Default)]
    struct MemoryStore(Mutex<HashMap<String, Value>>);

    #[async_trait]
    impl McpOAuthStore for MemoryStore {
        async fn read_value(&self, key: &str) -> Option<Value> {
            self.0.lock().unwrap().get(key).cloned()
        }

        async fn write_value(&self, key: &str, data: Value) -> Result<(), StorageError> {
            self.0.lock().unwrap().insert(key.into(), data);
            Ok(())
        }

        async fn remove(&self, key: &str) -> Result<(), StorageError> {
            self.0.lock().unwrap().remove(key);
            Ok(())
        }
    }

    fn provider(store: Arc<MemoryStore>) -> McpOAuthClientProvider {
        McpOAuthClientProvider::new("server", "https://example.test/mcp#fragment", store, None)
            .unwrap()
    }

    #[tokio::test]
    async fn loads_and_persists_cache_records_and_redirect_selection() {
        let store = Arc::new(MemoryStore::default());
        let provider = provider(Arc::clone(&store));
        assert_eq!(provider.server_url, "https://example.test/mcp");
        assert_eq!(provider.redirect_url(), PASSIVE_OAUTH_REDIRECT_URI);
        provider
            .save_client_information(
                serde_json::json!({"redirect_uris": ["https://stored.test/callback"]}),
            )
            .await
            .unwrap();
        assert_eq!(provider.redirect_url(), "https://stored.test/callback");
        provider
            .save_tokens(serde_json::json!({"access_token": "secret"}))
            .await
            .unwrap();
        assert_eq!(
            provider.tokens().await,
            Some(serde_json::json!({"access_token": "secret"}))
        );
        provider
            .save_discovery_state(serde_json::json!({"resource": "https://example.test"}))
            .await
            .unwrap();
        assert_eq!(
            provider.discovery_state().await,
            Some(serde_json::json!({"resource": "https://example.test"}))
        );

        provider.set_redirect_url(&Url::parse("http://127.0.0.1:9999/callback").unwrap());
        assert_eq!(provider.redirect_url(), "http://127.0.0.1:9999/callback");
    }

    #[tokio::test]
    async fn retains_and_resets_authorization_flow_state() {
        let provider = provider(Arc::new(MemoryStore::default()));
        assert_eq!(
            provider.code_verifier().unwrap_err().to_string(),
            "McpOAuthClientProvider: PKCE code verifier not initialized"
        );
        let state = provider.state().unwrap();
        assert_eq!(state.len(), 32);
        assert_eq!(provider.state().unwrap(), state);
        provider.save_code_verifier("verifier".into());
        provider.redirect_to_authorization(&Url::parse("https://auth.test/authorize").unwrap());
        assert_eq!(provider.code_verifier().unwrap(), "verifier");
        assert_eq!(
            provider.take_authorization_url().unwrap().as_str(),
            "https://auth.test/authorize"
        );
        assert!(provider.take_authorization_url().is_none());
        provider.reset_flow();
        assert!(provider.expected_state().is_none());
        assert!(provider.code_verifier().is_err());
    }

    #[tokio::test]
    async fn invalidates_exactly_the_requested_credential_scopes() {
        let provider = provider(Arc::new(MemoryStore::default()));
        provider
            .save_client_information(serde_json::json!({"client_id": "client"}))
            .await
            .unwrap();
        provider
            .save_tokens(serde_json::json!({"access_token": "token"}))
            .await
            .unwrap();
        provider
            .save_discovery_state(serde_json::json!({"issuer": "issuer"}))
            .await
            .unwrap();
        provider.save_code_verifier("verifier".into());

        provider
            .invalidate_credentials(McpOAuthInvalidationScope::Tokens)
            .await
            .unwrap();
        assert!(provider.tokens().await.is_none());
        assert!(provider.client_information().await.is_some());
        assert!(provider.discovery_state().await.is_some());
        assert_eq!(provider.code_verifier().unwrap(), "verifier");

        provider
            .invalidate_credentials(McpOAuthInvalidationScope::All)
            .await
            .unwrap();
        assert!(provider.client_information().await.is_none());
        assert!(provider.discovery_state().await.is_none());
        assert!(provider.code_verifier().is_err());
    }
}
