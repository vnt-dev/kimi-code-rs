//! App-scoped adapter over the migrated `kimi-code-oauth` toolkit crate.
//!
//! Original: `packages/agent-core-v2/src/app/auth/authService.ts`,
//! `OAuthToolkitService`.

use std::sync::Arc;

use async_trait::async_trait;
use kimi_code_oauth::{
    AuthManagedUsageResult, AuthManagedUserInfoResult, AuthenticatedServiceOptions,
    BearerTokenProvider, KimiOAuthLoginOptions, KimiOAuthLoginResult, KimiOAuthLogoutResult,
    KimiOAuthTokenRef, KimiOAuthToolkit, KimiOAuthToolkitOptions,
};

use crate::{
    _base::di::{
        descriptors::SyncDescriptor,
        errors::DiError,
        instantiation::ServicesAccessorExt,
        scope::{InstantiationType, LifecycleScope, register_scoped_service},
    },
    app::{
        auth::contract::{
            AuthOperationError, OAUTH_TOOLKIT_ID, OAuthToolkitContract, OAuthToolkitHandle,
        },
        bootstrap::BOOTSTRAP_SERVICE_ID,
    },
};

pub struct OAuthToolkitService {
    inner: KimiOAuthToolkit,
}

impl OAuthToolkitService {
    // Original: OAuthToolkitService.constructor().
    pub fn new(home_dir: impl Into<std::path::PathBuf>) -> Result<Self, AuthOperationError> {
        let inner = KimiOAuthToolkit::new(KimiOAuthToolkitOptions {
            home_dir: Some(home_dir.into()),
            ..KimiOAuthToolkitOptions::default()
        })
        .map_err(toolkit_error)?;
        Ok(Self { inner })
    }

    pub async fn get_managed_usage(
        &self,
        provider_name: Option<&str>,
        options: AuthenticatedServiceOptions<'_>,
    ) -> AuthManagedUsageResult {
        self.inner.get_managed_usage(provider_name, options).await
    }

    pub async fn get_managed_user_info(
        &self,
        provider_name: Option<&str>,
        options: AuthenticatedServiceOptions<'_>,
    ) -> AuthManagedUserInfoResult {
        self.inner
            .get_managed_user_info(provider_name, options)
            .await
    }
}

#[async_trait]
impl OAuthToolkitContract for OAuthToolkitService {
    fn token_provider(
        &self,
        provider_name: Option<&str>,
        oauth_ref: Option<&KimiOAuthTokenRef>,
    ) -> Result<BearerTokenProvider, AuthOperationError> {
        self.inner
            .token_provider(provider_name, oauth_ref)
            .map_err(toolkit_error)
    }

    async fn login(
        &self,
        provider_name: Option<&str>,
        options: KimiOAuthLoginOptions<'_>,
    ) -> Result<KimiOAuthLoginResult, AuthOperationError> {
        self.inner
            .login(provider_name, options)
            .await
            .map_err(toolkit_error)
    }

    async fn logout(
        &self,
        provider_name: Option<&str>,
        oauth_ref: Option<&KimiOAuthTokenRef>,
    ) -> Result<KimiOAuthLogoutResult, AuthOperationError> {
        self.inner
            .logout(provider_name, oauth_ref)
            .await
            .map_err(toolkit_error)
    }

    async fn get_cached_access_token(
        &self,
        provider_name: Option<&str>,
        oauth_ref: Option<&KimiOAuthTokenRef>,
    ) -> Result<Option<String>, AuthOperationError> {
        self.inner
            .get_cached_access_token(provider_name, oauth_ref)
            .await
            .map_err(toolkit_error)
    }
}

fn toolkit_error(error: impl std::fmt::Display) -> AuthOperationError {
    AuthOperationError::new(error.to_string())
}

// Original: registerScopedService(... OAuthToolkitService ...).
pub fn register_oauth_toolkit_service() {
    register_scoped_service(
        LifecycleScope::App,
        OAUTH_TOOLKIT_ID,
        SyncDescriptor::new(|accessor| {
            let bootstrap = accessor.get(BOOTSTRAP_SERVICE_ID)?;
            let service = OAuthToolkitService::new(bootstrap.home_dir())
                .map_err(|error| DiError::Factory(error.to_string()))?;
            let service: Arc<dyn OAuthToolkitContract> = Arc::new(service);
            Ok(OAuthToolkitHandle(service))
        }),
        InstantiationType::Eager,
        "auth",
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn delegates_cached_token_and_provider_resolution_to_oauth_crate() {
        let home =
            std::env::temp_dir().join(format!("kimi-oauth-toolkit-{}", uuid::Uuid::new_v4()));
        let service = OAuthToolkitService::new(&home).unwrap();
        assert_eq!(
            service
                .get_cached_access_token(Some("kimi-code"), None)
                .await
                .unwrap(),
            None
        );
        assert!(service.token_provider(Some("kimi-code"), None).is_ok());
        let _ = std::fs::remove_dir_all(home);
    }
}
