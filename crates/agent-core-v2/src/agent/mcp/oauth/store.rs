//! MCP OAuth credential-store keys and atomic-document adapter.
//!
//! Original: `agent/mcp/oauth/store.ts`.

use async_trait::async_trait;
use serde_json::Value;
use sha2::{Digest, Sha256};
use url::Url;

use crate::{
    _base::utils::hash::encode_hex,
    persistence::interface::atomic_document_store::AtomicDocumentStoreHandle,
};

pub const MCP_OAUTH_CREDENTIALS_SCOPE: &str = "credentials/mcp";

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
#[error("Invalid MCP OAuth store key: \"{name}\"")]
pub struct McpOAuthStoreKeyError {
    pub name: String,
}

// Original: sanitizeStoreKey(). `pathe.basename()` accepts both separator
// styles, so the Rust adaptation does likewise on every target platform.
pub fn sanitize_mcp_oauth_store_key(name: &str) -> Result<String, McpOAuthStoreKeyError> {
    let basename = name.rsplit(['/', '\\']).next().unwrap_or_default();
    let mut safe = String::with_capacity(basename.len());
    let mut previous_underscore = false;
    for character in basename.chars() {
        let is_safe = character.is_ascii_alphanumeric() || matches!(character, '_' | '-');
        if is_safe {
            safe.push(character);
            previous_underscore = false;
        } else if !previous_underscore {
            safe.push('_');
            previous_underscore = true;
        }
    }
    if safe.is_empty() || safe.starts_with('.') {
        return Err(McpOAuthStoreKeyError { name: name.into() });
    }
    Ok(safe)
}

// Original: canonicalMcpOAuthResource().
pub fn canonical_mcp_oauth_resource(server_url: &str) -> Result<String, url::ParseError> {
    let mut url = Url::parse(server_url)?;
    url.set_fragment(None);
    Ok(url.into())
}

// Original: mcpOAuthStoreKey().
pub fn mcp_oauth_store_key(
    server_name: &str,
    server_url: &str,
) -> Result<String, McpOAuthStoreKeyError> {
    let safe_name = sanitize_mcp_oauth_store_key(server_name)?;
    let resource = canonical_mcp_oauth_resource(server_url).map_err(|_| McpOAuthStoreKeyError {
        name: server_name.into(),
    })?;
    let mut hasher = Sha256::new();
    hasher.update(server_name.as_bytes());
    hasher.update([0]);
    hasher.update(resource.as_bytes());
    let digest = encode_hex(hasher.finalize());
    Ok(format!("{safe_name}-{}", &digest[..24]))
}

#[async_trait]
pub trait McpOAuthStore: Send + Sync {
    async fn read_value(&self, key: &str) -> Option<Value>;

    async fn write_value(
        &self,
        key: &str,
        data: Value,
    ) -> Result<(), crate::persistence::interface::storage::StorageError>;

    async fn remove(
        &self,
        key: &str,
    ) -> Result<(), crate::persistence::interface::storage::StorageError>;
}

#[derive(Clone)]
pub struct AtomicMcpOAuthStore {
    documents: AtomicDocumentStoreHandle,
}

impl AtomicMcpOAuthStore {
    pub fn new(documents: AtomicDocumentStoreHandle) -> Self {
        Self { documents }
    }
}

#[async_trait]
impl McpOAuthStore for AtomicMcpOAuthStore {
    // Original: createMcpOAuthStore().read(). Backend read failures
    // intentionally mean absent; typed callers validate their own shape.
    async fn read_value(&self, key: &str) -> Option<Value> {
        self.documents
            .0
            .get_value(MCP_OAUTH_CREDENTIALS_SCOPE, key)
            .await
            .ok()
            .flatten()
    }

    async fn write_value(
        &self,
        key: &str,
        data: Value,
    ) -> Result<(), crate::persistence::interface::storage::StorageError> {
        self.documents
            .0
            .set_value(MCP_OAUTH_CREDENTIALS_SCOPE, key, data)
            .await
    }

    async fn remove(
        &self,
        key: &str,
    ) -> Result<(), crate::persistence::interface::storage::StorageError> {
        self.documents
            .0
            .delete(MCP_OAUTH_CREDENTIALS_SCOPE, key)
            .await
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::{Arc};
    use parking_lot::Mutex;

    use async_trait::async_trait;

    use super::*;
    use crate::{
        _base::{
            di::lifecycle::{DisposableHandle, disposable_none},
            event::Event,
        },
        persistence::interface::{
            atomic_document_store::AtomicDocumentStoreService, storage::StorageError,
        },
    };

    #[derive(Default)]
    struct Documents(Mutex<HashMap<String, Value>>);

    #[async_trait]
    impl AtomicDocumentStoreService for Documents {
        async fn get_value(&self, _scope: &str, key: &str) -> Result<Option<Value>, StorageError> {
            Ok(self.0.lock().get(key).cloned())
        }

        async fn set_value(
            &self,
            _scope: &str,
            key: &str,
            value: Value,
        ) -> Result<(), StorageError> {
            self.0.lock().insert(key.into(), value);
            Ok(())
        }

        async fn delete(&self, _scope: &str, key: &str) -> Result<(), StorageError> {
            self.0.lock().remove(key);
            Ok(())
        }

        async fn list(
            &self,
            _scope: &str,
            _prefix: Option<&str>,
        ) -> Result<Vec<String>, StorageError> {
            Ok(vec![])
        }

        fn watch(&self, _scope: &str, _key: &str) -> Event<()> {
            Event::none()
        }

        fn acquire(&self, _scope: &str, _key: &str) -> DisposableHandle {
            disposable_none()
        }
    }

    #[test]
    fn sanitizes_keys_and_hashes_canonical_resources() {
        assert_eq!(
            sanitize_mcp_oauth_store_key("../a b///server").unwrap(),
            "server"
        );
        assert!(sanitize_mcp_oauth_store_key("").is_err());
        assert_eq!(
            canonical_mcp_oauth_resource("https://example.test/mcp#fragment").unwrap(),
            "https://example.test/mcp"
        );
        assert_eq!(
            mcp_oauth_store_key("a b", "https://example.test/mcp#fragment").unwrap(),
            "a_b-57e91cbfa067bc02d78ef57d"
        );
    }

    #[tokio::test]
    async fn persists_in_the_mcp_credentials_scope_and_treats_read_errors_as_absent() {
        let documents = Arc::new(Documents::default());
        let store = AtomicMcpOAuthStore::new(AtomicDocumentStoreHandle(documents));
        store
            .write_value("record", serde_json::json!({"token": "value"}))
            .await
            .unwrap();
        assert_eq!(
            store.read_value("record").await,
            Some(serde_json::json!({"token": "value"}))
        );
        store.remove("record").await.unwrap();
        assert_eq!(store.read_value("record").await, None);
    }
}
