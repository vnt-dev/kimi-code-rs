//! Session-scoped MCP OAuth authorization orchestration.
//!
//! Original: `agent/mcp/oauth/service.ts`, `McpOAuthService`.

use std::{collections::HashMap, sync::Arc};

use async_trait::async_trait;
use rmcp::transport::auth::{
    AuthError, AuthorizationManager, AuthorizationRequest, AuthorizationSession, CredentialStore,
    StoredCredentials,
};
use serde_json::Value;
use tokio::sync::Mutex;
use url::Url;

use crate::_base::utils::abort::AbortSignal;

use super::{CallbackServer, McpOAuthClientProvider, McpOAuthInvalidationScope, McpOAuthStore};

#[derive(Clone)]
pub struct McpOAuthServiceOptions {
    pub store: Arc<dyn McpOAuthStore>,
    pub client_label: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum McpOAuthServiceError {
    #[error(transparent)]
    AlreadyAuthorized(#[from] AlreadyAuthorizedError),
    #[error("failed to start OAuth callback listener: {0}")]
    CallbackStart(String),
    #[error("failed to start OAuth flow for {server_name:?}: {message}")]
    Start {
        server_name: String,
        message: String,
    },
    #[error("OAuth flow for {server_name:?} failed: {message}")]
    Complete {
        server_name: String,
        message: String,
    },
    #[error("OAuth flow already completed or cancelled")]
    Settled,
    #[error("OAuth state mismatch — possible CSRF; refusing token exchange")]
    StateMismatch,
    #[error(transparent)]
    Provider(#[from] super::McpOAuthProviderError),
}

#[derive(Debug, thiserror::Error)]
#[error("{server_name:?} is already authorized; no browser flow needed")]
pub struct AlreadyAuthorizedError {
    pub server_name: String,
}

struct ProviderCredentialStore {
    provider: Arc<McpOAuthClientProvider>,
}

#[async_trait]
impl CredentialStore for ProviderCredentialStore {
    async fn load(&self) -> Result<Option<StoredCredentials>, AuthError> {
        let Some(client) = self.provider.client_information().await else {
            return Ok(None);
        };
        let Some(client_id) = client.get("client_id").and_then(Value::as_str) else {
            return Ok(None);
        };
        let token_response = self
            .provider
            .tokens()
            .await
            .map(serde_json::from_value)
            .transpose()
            .map_err(|error| AuthError::InternalError(error.to_string()))?;
        Ok(Some(StoredCredentials::new(
            client_id.into(),
            token_response,
            Vec::new(),
            None,
        )))
    }

    async fn save(&self, credentials: StoredCredentials) -> Result<(), AuthError> {
        self.provider
            .save_client_information(serde_json::json!({"client_id": credentials.client_id}))
            .await
            .map_err(|error| AuthError::InternalError(error.to_string()))?;
        if let Some(tokens) = credentials.token_response {
            self.provider
                .save_tokens(
                    serde_json::to_value(tokens)
                        .map_err(|error| AuthError::InternalError(error.to_string()))?,
                )
                .await
                .map_err(|error| AuthError::InternalError(error.to_string()))?;
        }
        Ok(())
    }

    async fn clear(&self) -> Result<(), AuthError> {
        self.provider
            .invalidate_credentials(McpOAuthInvalidationScope::All)
            .await
            .map_err(|error| AuthError::InternalError(error.to_string()))
    }
}

struct PendingAuthorization {
    callback: Arc<CallbackServer>,
    session: AuthorizationSession,
}

pub struct BeginAuthorizationResult {
    pub authorization_url: Url,
    server_name: String,
    provider: Arc<McpOAuthClientProvider>,
    pending: Mutex<Option<PendingAuthorization>>,
}

impl BeginAuthorizationResult {
    // Original: BeginAuthorizationResult.cancel().
    pub async fn cancel(&self) {
        let pending = self.pending.lock().await.take();
        if let Some(pending) = pending {
            pending.callback.close().await;
        }
        self.provider.reset_flow();
    }

    // Original: BeginAuthorizationResult.complete().
    pub async fn complete(
        &self,
        signal: Option<AbortSignal>,
        timeout_ms: Option<u64>,
    ) -> Result<(), McpOAuthServiceError> {
        let pending = self
            .pending
            .lock()
            .await
            .take()
            .ok_or(McpOAuthServiceError::Settled)?;
        let callback = Arc::clone(&pending.callback);
        let result = async {
            let response = callback
                .wait_for_code(signal, timeout_ms)
                .await
                .map_err(|error| McpOAuthServiceError::Complete {
                    server_name: self.server_name.clone(),
                    message: error.to_string(),
                })?;
            if let Some(expected) = self.provider.expected_state()
                && response.state.as_deref() != Some(expected.as_str())
            {
                return Err(McpOAuthServiceError::StateMismatch);
            }
            pending
                .session
                .handle_callback(
                    &response.code,
                    response.state.as_deref().unwrap_or_default(),
                )
                .await
                .map_err(|error| McpOAuthServiceError::Complete {
                    server_name: self.server_name.clone(),
                    message: error.to_string(),
                })?;
            Ok(())
        }
        .await;
        if let Err(error) = result {
            callback.close().await;
            self.provider.reset_flow();
            return Err(match error {
                McpOAuthServiceError::StateMismatch | McpOAuthServiceError::Complete { .. } => {
                    error
                }
                _ => McpOAuthServiceError::Complete {
                    server_name: self.server_name.clone(),
                    message: error.to_string(),
                },
            });
        }
        callback.close().await;
        self.provider.reset_flow();
        Ok(())
    }
}

pub struct McpOAuthService {
    store: Arc<dyn McpOAuthStore>,
    client_label: Option<String>,
    providers: Mutex<HashMap<String, Arc<McpOAuthClientProvider>>>,
}

impl McpOAuthService {
    // Original: McpOAuthService.constructor().
    pub fn new(options: McpOAuthServiceOptions) -> Self {
        Self {
            store: options.store,
            client_label: options.client_label,
            providers: Mutex::new(HashMap::new()),
        }
    }

    // Original: getProvider().
    pub async fn get_provider(
        &self,
        server_name: &str,
        server_url: &str,
    ) -> Result<Arc<McpOAuthClientProvider>, McpOAuthServiceError> {
        let candidate = McpOAuthClientProvider::new(
            server_name,
            server_url,
            Arc::clone(&self.store),
            self.client_label.clone(),
        )?;
        let mut providers = self.providers.lock().await;
        Ok(providers
            .entry(candidate.store_key.clone())
            .or_insert_with(|| Arc::new(candidate))
            .clone())
    }

    // Original: hasTokens().
    pub async fn has_tokens(
        &self,
        server_name: &str,
        server_url: &str,
    ) -> Result<bool, McpOAuthServiceError> {
        Ok(self
            .get_provider(server_name, server_url)
            .await?
            .tokens()
            .await
            .is_some())
    }

    // Original: beginAuthorization().
    pub async fn begin_authorization(
        &self,
        server_name: &str,
        server_url: &str,
        client_label: Option<String>,
    ) -> Result<BeginAuthorizationResult, McpOAuthServiceError> {
        let provider = if client_label.is_some() {
            let provider = Arc::new(McpOAuthClientProvider::new(
                server_name,
                server_url,
                Arc::clone(&self.store),
                client_label.clone(),
            )?);
            self.providers
                .lock()
                .await
                .insert(provider.store_key.clone(), Arc::clone(&provider));
            provider
        } else {
            self.get_provider(server_name, server_url).await?
        };
        provider.ready().await;
        if provider.tokens().await.is_some() {
            return Err(AlreadyAuthorizedError {
                server_name: server_name.into(),
            }
            .into());
        }
        provider.reset_flow();
        let callback = CallbackServer::start()
            .await
            .map_err(|error| McpOAuthServiceError::CallbackStart(error.to_string()))?;
        provider.set_redirect_url(&Url::parse(&callback.redirect_uri).map_err(|error| {
            McpOAuthServiceError::Start {
                server_name: server_name.into(),
                message: error.to_string(),
            }
        })?);
        let mut manager = AuthorizationManager::new(server_url)
            .await
            .map_err(|error| McpOAuthServiceError::Start {
                server_name: server_name.into(),
                message: error.to_string(),
            })?;
        manager.set_credential_store(ProviderCredentialStore {
            provider: Arc::clone(&provider),
        });
        let request = AuthorizationRequest::new(callback.redirect_uri.clone())
            .with_client_name(client_label.unwrap_or_else(|| format!("kimi-code ({server_name})")));
        let session = AuthorizationSession::new(manager, request)
            .await
            .map_err(|(_, error)| McpOAuthServiceError::Start {
                server_name: server_name.into(),
                message: error.to_string(),
            })?;
        let authorization_url =
            Url::parse(&session.auth_url).map_err(|error| McpOAuthServiceError::Start {
                server_name: server_name.into(),
                message: error.to_string(),
            })?;
        let state = authorization_url
            .query_pairs()
            .find_map(|(name, value)| (name == "state").then_some(value.into_owned()))
            .ok_or_else(|| McpOAuthServiceError::Start {
                server_name: server_name.into(),
                message: "OAuth authorization URL did not include state".into(),
            })?;
        provider.set_expected_state(state);
        Ok(BeginAuthorizationResult {
            authorization_url,
            server_name: server_name.into(),
            provider,
            pending: Mutex::new(Some(PendingAuthorization { callback, session })),
        })
    }

    // Original: invalidate().
    pub async fn invalidate(
        &self,
        server_name: &str,
        server_url: &str,
        scope: McpOAuthInvalidationScope,
    ) -> Result<(), McpOAuthServiceError> {
        self.get_provider(server_name, server_url)
            .await?
            .invalidate_credentials(scope)
            .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex as StdMutex;

    use super::*;
    use crate::persistence::interface::storage::StorageError;

    #[derive(Default)]
    struct MemoryStore(StdMutex<HashMap<String, Value>>);

    #[async_trait]
    impl McpOAuthStore for MemoryStore {
        async fn read_value(&self, key: &str) -> Option<Value> {
            self.0.lock().unwrap().get(key).cloned()
        }

        async fn write_value(&self, key: &str, value: Value) -> Result<(), StorageError> {
            self.0.lock().unwrap().insert(key.into(), value);
            Ok(())
        }

        async fn remove(&self, key: &str) -> Result<(), StorageError> {
            self.0.lock().unwrap().remove(key);
            Ok(())
        }
    }

    #[tokio::test]
    async fn caches_providers_and_observes_and_invalidates_persisted_tokens() {
        let service = McpOAuthService::new(McpOAuthServiceOptions {
            store: Arc::new(MemoryStore::default()),
            client_label: None,
        });
        let first = service
            .get_provider("remote", "https://example.test/mcp#fragment")
            .await
            .unwrap();
        let second = service
            .get_provider("remote", "https://example.test/mcp")
            .await
            .unwrap();
        assert!(Arc::ptr_eq(&first, &second));
        assert!(
            !service
                .has_tokens("remote", "https://example.test/mcp")
                .await
                .unwrap()
        );

        first
            .save_tokens(serde_json::json!({"access_token": "token"}))
            .await
            .unwrap();
        assert!(
            service
                .has_tokens("remote", "https://example.test/mcp")
                .await
                .unwrap()
        );
        service
            .invalidate(
                "remote",
                "https://example.test/mcp",
                McpOAuthInvalidationScope::Tokens,
            )
            .await
            .unwrap();
        assert!(
            !service
                .has_tokens("remote", "https://example.test/mcp")
                .await
                .unwrap()
        );
    }
}
