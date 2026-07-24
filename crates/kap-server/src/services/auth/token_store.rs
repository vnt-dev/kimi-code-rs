use std::fs::Metadata;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

use subtle::ConstantTimeEq;

use super::persistent_token::{load_or_create_server_token, server_token_path};
use super::private_files::PrivateFileError;

#[derive(Debug)]
struct TokenCache {
    token: String,
    modified: Option<SystemTime>,
    file_identity: u64,
}

/// Persistent token store over `<homeDir>/server.token`.
#[derive(Debug)]
pub struct TokenStore {
    token_path: PathBuf,
    cache: Mutex<TokenCache>,
}

impl TokenStore {
    pub async fn create(home_dir: impl AsRef<Path>) -> Result<Self, PrivateFileError> {
        let token_path = server_token_path(home_dir);
        let token =
            load_or_create_server_token(token_path.parent().unwrap_or_else(|| Path::new(".")))
                .await?;
        let metadata = std::fs::metadata(&token_path)?;
        Ok(Self {
            token_path,
            cache: Mutex::new(TokenCache {
                token,
                modified: metadata.modified().ok(),
                file_identity: file_identity(&metadata),
            }),
        })
    }

    pub fn token_path(&self) -> &Path {
        &self.token_path
    }

    // Original: tokenStore.ts, currentToken()/getToken().
    pub fn get_token(&self) -> String {
        let mut cache = self.cache.lock().unwrap_or_else(|error| error.into_inner());
        let Ok(metadata) = std::fs::metadata(&self.token_path) else {
            return cache.token.clone();
        };
        let modified = metadata.modified().ok();
        let identity = file_identity(&metadata);
        if modified == cache.modified && identity == cache.file_identity {
            return cache.token.clone();
        }
        if is_too_permissive(&metadata) {
            return cache.token.clone();
        }
        if let Ok(contents) = std::fs::read_to_string(&self.token_path) {
            let token = contents.trim();
            if !token.is_empty() {
                cache.token = token.to_owned();
                cache.modified = modified;
                cache.file_identity = identity;
            }
        }
        cache.token.clone()
    }

    pub fn is_valid(&self, candidate: &str) -> bool {
        let token = self.get_token();
        candidate.len() == token.len() && bool::from(candidate.as_bytes().ct_eq(token.as_bytes()))
    }

    /// Persistent token files deliberately survive disposal.
    pub async fn dispose(&self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(unix)]
fn file_identity(metadata: &Metadata) -> u64 {
    use std::os::unix::fs::MetadataExt;
    metadata.ino()
}

#[cfg(not(unix))]
fn file_identity(_metadata: &Metadata) -> u64 {
    0
}

#[cfg(unix)]
fn is_too_permissive(metadata: &Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    metadata.permissions().mode() & 0o077 != 0
}

#[cfg(not(unix))]
fn is_too_permissive(_metadata: &Metadata) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::auth::private_files::write_private_file;

    #[tokio::test]
    async fn persists_and_validates_tokens() {
        let directory = tempfile::tempdir().unwrap();
        let store = TokenStore::create(directory.path()).await.unwrap();
        let token = store.get_token();
        assert_eq!(store.get_token(), token);
        assert!(store.is_valid(&token));
        assert!(!store.is_valid(""));
        assert!(!store.is_valid(&"x".repeat(token.len())));

        let second = TokenStore::create(directory.path()).await.unwrap();
        assert_eq!(second.get_token(), token);
        store.dispose().await.unwrap();
        assert!(store.token_path().exists());
    }

    #[tokio::test]
    async fn reloads_after_live_rotation() {
        let directory = tempfile::tempdir().unwrap();
        let store = TokenStore::create(directory.path()).await.unwrap();
        let original = store.get_token();
        let rotated = "r".repeat(original.len());
        write_private_file(store.token_path(), &rotated)
            .await
            .unwrap();

        assert_eq!(store.get_token(), rotated);
        assert!(store.is_valid(&rotated));
        assert!(!store.is_valid(&original));
    }
}
