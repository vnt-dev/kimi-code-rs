use subtle::ConstantTimeEq;

use super::auth_token_service::AuthTokenService;
use super::password::PasswordError;

#[derive(Debug, Clone)]
pub struct CredentialValidator {
    auth_token_service: AuthTokenService,
    rpc_token: Option<String>,
}

impl CredentialValidator {
    pub fn new(auth_token_service: AuthTokenService, rpc_token: Option<String>) -> Self {
        Self {
            auth_token_service,
            rpc_token,
        }
    }

    // Original: credentials.ts, createCredentialValidator() returned closure.
    pub async fn is_valid(&self, candidate: &str) -> Result<bool, PasswordError> {
        if self.auth_token_service.is_valid(candidate).await? {
            return Ok(true);
        }
        Ok(self.rpc_token.as_deref().is_some_and(|expected| {
            !candidate.is_empty()
                && candidate.len() == expected.len()
                && bool::from(candidate.as_bytes().ct_eq(expected.as_bytes()))
        }))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::services::auth::auth_token_service::create_auth_token_service;
    use crate::services::auth::token_store::TokenStore;

    #[tokio::test]
    async fn accepts_primary_or_additional_rpc_token() {
        let directory = tempfile::tempdir().unwrap();
        let store = Arc::new(TokenStore::create(directory.path()).await.unwrap());
        let validator = CredentialValidator::new(
            create_auth_token_service(Arc::clone(&store), None),
            Some("rpc-token".into()),
        );
        assert!(validator.is_valid(&store.get_token()).await.unwrap());
        assert!(validator.is_valid("rpc-token").await.unwrap());
        assert!(!validator.is_valid("").await.unwrap());
        assert!(!validator.is_valid("wrong").await.unwrap());
    }
}
