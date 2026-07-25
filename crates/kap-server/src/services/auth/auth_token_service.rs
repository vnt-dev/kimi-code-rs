use std::sync::Arc;

use super::password::{PasswordError, verify_password};
use super::token_store::TokenStore;

#[derive(Debug, Clone)]
pub struct AuthTokenService {
    token_store: Arc<TokenStore>,
    password_hash: Option<String>,
}

pub fn create_auth_token_service(
    token_store: Arc<TokenStore>,
    password_hash: Option<String>,
) -> AuthTokenService {
    AuthTokenService {
        token_store,
        password_hash,
    }
}

impl AuthTokenService {
    pub fn get_token(&self) -> String {
        self.token_store.get_token()
    }

    // Original: authTokenService.ts, createAuthTokenService().isValid().
    pub async fn is_valid(&self, candidate: &str) -> Result<bool, PasswordError> {
        if self.token_store.is_valid(candidate) {
            return Ok(true);
        }
        verify_password(candidate, self.password_hash.as_deref()).await
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::services::auth::password::resolve_password_hash;

    #[tokio::test]
    async fn accepts_token_or_password() {
        let directory = tempfile::tempdir().unwrap();
        let store = Arc::new(TokenStore::create(directory.path()).await.unwrap());
        let env = HashMap::from([("KIMI_CODE_PASSWORD".into(), "password".into())]);
        let password_hash = resolve_password_hash(&env).await.unwrap();
        let service = create_auth_token_service(Arc::clone(&store), password_hash);

        assert_eq!(service.get_token(), store.get_token());
        assert!(service.is_valid(&store.get_token()).await.unwrap());
        assert!(service.is_valid("password").await.unwrap());
        assert!(!service.is_valid("wrong").await.unwrap());
    }
}
