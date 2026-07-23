//! OAuth, authentication status, and readiness contracts.
//!
//! Original: `packages/agent-core-v2/src/app/auth/auth.ts`.

use std::{error::Error, fmt, ops::Deref, sync::Arc};

use async_trait::async_trait;
use kimi_code_oauth::{
    BearerTokenProvider, KimiOAuthLoginOptions, KimiOAuthLoginResult, KimiOAuthLogoutResult,
    KimiOAuthTokenRef,
};
use serde_json::{Map, Value};

use crate::{
    _base::{
        di::instantiation::ServiceIdentifier,
        errors::errors::{Error2, Error2Options},
    },
    kosong::provider::config::OAuthRef,
};

use super::{
    errors::{AUTH_MODEL_NOT_RESOLVED, AUTH_PROVISIONING_REQUIRED, AUTH_TOKEN_MISSING},
    oauth_protocol::{
        OAuthFlowSnapshot, OAuthFlowStart, OAuthLoginCancelResponse, OAuthLogoutResponse,
        RefreshOAuthProviderModelsResponse,
    },
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuthStatus {
    pub logged_in: bool,
    pub provider: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
#[error("{message}")]
pub struct AuthOperationError {
    pub message: String,
}

impl AuthOperationError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

#[async_trait]
pub trait OAuthServiceContract: Send + Sync {
    async fn start_login(
        &self,
        provider: Option<&str>,
    ) -> Result<OAuthFlowStart, AuthOperationError>;
    fn get_flow(&self, provider: Option<&str>) -> Option<OAuthFlowSnapshot>;
    async fn cancel_login(
        &self,
        provider: Option<&str>,
    ) -> Result<OAuthLoginCancelResponse, AuthOperationError>;
    async fn logout(
        &self,
        provider: Option<&str>,
    ) -> Result<OAuthLogoutResponse, AuthOperationError>;
    async fn status(&self, provider: Option<&str>) -> Result<AuthStatus, AuthOperationError>;
    async fn refresh_oauth_provider_models(
        &self,
    ) -> Result<RefreshOAuthProviderModelsResponse, AuthOperationError>;
    fn resolve_token_provider(
        &self,
        provider: &str,
        oauth_ref: Option<&OAuthRef>,
    ) -> Option<BearerTokenProvider>;
    async fn get_cached_access_token(
        &self,
        provider: &str,
        oauth_ref: Option<&OAuthRef>,
    ) -> Result<Option<String>, AuthOperationError>;
}

#[derive(Clone)]
pub struct OAuthServiceHandle(pub Arc<dyn OAuthServiceContract>);

impl Deref for OAuthServiceHandle {
    type Target = dyn OAuthServiceContract;

    fn deref(&self) -> &Self::Target {
        self.0.as_ref()
    }
}

pub const OAUTH_SERVICE_ID: ServiceIdentifier<OAuthServiceHandle> =
    ServiceIdentifier::new("oauthService");

#[async_trait]
pub trait OAuthToolkitContract: Send + Sync {
    async fn login(
        &self,
        provider_name: Option<&str>,
        options: KimiOAuthLoginOptions<'_>,
    ) -> Result<KimiOAuthLoginResult, AuthOperationError>;
    async fn logout(
        &self,
        provider_name: Option<&str>,
        oauth_ref: Option<&KimiOAuthTokenRef>,
    ) -> Result<KimiOAuthLogoutResult, AuthOperationError>;
    async fn get_cached_access_token(
        &self,
        provider_name: Option<&str>,
        oauth_ref: Option<&KimiOAuthTokenRef>,
    ) -> Result<Option<String>, AuthOperationError>;
    fn token_provider(
        &self,
        provider_name: Option<&str>,
        oauth_ref: Option<&KimiOAuthTokenRef>,
    ) -> Result<BearerTokenProvider, AuthOperationError>;
}

#[derive(Clone)]
pub struct OAuthToolkitHandle(pub Arc<dyn OAuthToolkitContract>);

impl Deref for OAuthToolkitHandle {
    type Target = dyn OAuthToolkitContract;

    fn deref(&self) -> &Self::Target {
        self.0.as_ref()
    }
}

pub const OAUTH_TOOLKIT_ID: ServiceIdentifier<OAuthToolkitHandle> =
    ServiceIdentifier::new("oauthToolkit");

#[async_trait]
pub trait AuthSummaryServiceContract: Send + Sync {
    async fn summarize(&self) -> Result<Vec<AuthStatus>, AuthOperationError>;
    async fn ensure_ready(&self, model_override: Option<&str>) -> Result<(), AuthOperationError>;
}

#[derive(Clone)]
pub struct AuthSummaryServiceHandle(pub Arc<dyn AuthSummaryServiceContract>);

impl Deref for AuthSummaryServiceHandle {
    type Target = dyn AuthSummaryServiceContract;

    fn deref(&self) -> &Self::Target {
        self.0.as_ref()
    }
}

pub const AUTH_SUMMARY_SERVICE_ID: ServiceIdentifier<AuthSummaryServiceHandle> =
    ServiceIdentifier::new("authSummaryService");

#[derive(Debug)]
pub struct AuthProvisioningRequiredError(Error2);

impl AuthProvisioningRequiredError {
    pub fn new() -> Self {
        Self(Error2::with_options(
            AUTH_PROVISIONING_REQUIRED,
            "no provider configured; complete onboarding via /login or the providers endpoint",
            Error2Options {
                name: Some("AuthProvisioningRequiredError".into()),
                ..Error2Options::default()
            },
        ))
    }

    pub fn error(&self) -> &Error2 {
        &self.0
    }
}

impl Default for AuthProvisioningRequiredError {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug)]
pub struct AuthTokenMissingError {
    inner: Error2,
    pub provider_id: String,
}

impl AuthTokenMissingError {
    pub fn new(provider_id: impl Into<String>) -> Self {
        let provider_id = provider_id.into();
        Self {
            inner: Error2::with_options(
                AUTH_TOKEN_MISSING,
                format!("provider {provider_id} has no credential configured"),
                Error2Options {
                    details: Some(Map::from_iter([(
                        "provider_id".into(),
                        Value::String(provider_id.clone()),
                    )])),
                    name: Some("AuthTokenMissingError".into()),
                    ..Error2Options::default()
                },
            ),
            provider_id,
        }
    }

    pub fn error(&self) -> &Error2 {
        &self.inner
    }
}

#[derive(Debug)]
pub struct AuthModelNotResolvedError {
    inner: Error2,
    pub model_id: Option<String>,
    pub provider_id: Option<String>,
}

impl AuthModelNotResolvedError {
    pub fn new(model_id: Option<String>, provider_id: Option<String>) -> Self {
        let mut details = Map::new();
        if let Some(model_id) = model_id.as_ref() {
            details.insert("model_id".into(), Value::String(model_id.clone()));
        }
        if let Some(provider_id) = provider_id.as_ref() {
            details.insert("provider_id".into(), Value::String(provider_id.clone()));
        }
        let message = model_id.as_ref().map_or_else(
            || "no default model configured".into(),
            |model| format!("model {model} does not resolve to a configured provider"),
        );
        Self {
            inner: Error2::with_options(
                AUTH_MODEL_NOT_RESOLVED,
                message,
                Error2Options {
                    details: (!details.is_empty()).then_some(details),
                    name: Some("AuthModelNotResolvedError".into()),
                    ..Error2Options::default()
                },
            ),
            model_id,
            provider_id,
        }
    }

    pub fn error(&self) -> &Error2 {
        &self.inner
    }
}

macro_rules! impl_wrapped_error {
    ($type:ty, $field:tt) => {
        impl fmt::Display for $type {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.$field.fmt(formatter)
            }
        }

        impl Error for $type {}
    };
}

impl_wrapped_error!(AuthProvisioningRequiredError, 0);
impl_wrapped_error!(AuthTokenMissingError, inner);
impl_wrapped_error!(AuthModelNotResolvedError, inner);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifiers_and_structured_errors_preserve_source_contract() {
        assert_eq!(OAUTH_SERVICE_ID.to_string(), "oauthService");
        assert_eq!(OAUTH_TOOLKIT_ID.to_string(), "oauthToolkit");
        assert_eq!(AUTH_SUMMARY_SERVICE_ID.to_string(), "authSummaryService");

        let provisioning = AuthProvisioningRequiredError::new();
        assert_eq!(provisioning.error().code, AUTH_PROVISIONING_REQUIRED);
        assert_eq!(provisioning.error().name, "AuthProvisioningRequiredError");

        let missing = AuthTokenMissingError::new("provider-a");
        assert_eq!(missing.error().code, AUTH_TOKEN_MISSING);
        assert_eq!(missing.provider_id, "provider-a");
        assert_eq!(
            missing.error().details.as_ref().unwrap()["provider_id"],
            "provider-a"
        );

        let unresolved =
            AuthModelNotResolvedError::new(Some("model-a".into()), Some("provider-a".into()));
        assert_eq!(unresolved.error().code, AUTH_MODEL_NOT_RESOLVED);
        assert_eq!(
            unresolved.to_string(),
            "model model-a does not resolve to a configured provider"
        );
        assert_eq!(
            AuthModelNotResolvedError::new(None, None).to_string(),
            "no default model configured"
        );
    }
}
