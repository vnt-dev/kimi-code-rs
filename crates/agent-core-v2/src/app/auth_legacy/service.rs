use super::{
    AUTH_LEGACY_SERVICE_ID, AuthLegacyResult, AuthLegacyServiceContract, AuthLegacyServiceHandle,
    AuthSummary, ManagedProviderStatus, ManagedProviderSummary,
};
use crate::{
    _base::di::{
        descriptors::SyncDescriptor,
        instantiation::ServicesAccessorExt,
        scope::{InstantiationType, LifecycleScope, register_scoped_service},
    },
    app::{
        auth::{OAUTH_SERVICE_ID, OAuthServiceHandle},
        config::{CONFIG_SERVICE_ID, ConfigServiceHandle},
    },
    kosong::{
        model::contract::DEFAULT_MODEL_SECTION,
        provider::{PROVIDER_SERVICE_ID, ProviderServiceHandle},
    },
};
use async_trait::async_trait;
use kimi_code_oauth::KIMI_CODE_PROVIDER_NAME;
use std::sync::Arc;
pub struct AuthLegacyService {
    providers: ProviderServiceHandle,
    config: ConfigServiceHandle,
    oauth: OAuthServiceHandle,
}
impl AuthLegacyService {
    pub fn new(
        providers: ProviderServiceHandle,
        config: ConfigServiceHandle,
        oauth: OAuthServiceHandle,
    ) -> Self {
        Self {
            providers,
            config,
            oauth,
        }
    }
    async fn managed_logged_in(&self) -> bool {
        self.oauth
            .status(Some(KIMI_CODE_PROVIDER_NAME))
            .await
            .map(|status| status.logged_in)
            .unwrap_or(false)
    }
}
#[async_trait]
impl AuthLegacyServiceContract for AuthLegacyService {
    async fn get(&self) -> AuthLegacyResult<AuthSummary> {
        self.config.ready().await?;
        let providers = self.providers.list();
        let providers_count = providers.len() as u64;
        let default_model = self.config.get(DEFAULT_MODEL_SECTION).and_then(|value| {
            value
                .as_str()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_owned)
        });
        let managed_provider = if providers.contains_key(KIMI_CODE_PROVIDER_NAME) {
            Some(ManagedProviderSummary {
                name: KIMI_CODE_PROVIDER_NAME.into(),
                status: if self.managed_logged_in().await {
                    ManagedProviderStatus::Authenticated
                } else {
                    ManagedProviderStatus::Unauthenticated
                },
            })
        } else {
            None
        };
        let ready = providers_count >= 1
            && default_model.is_some()
            && !matches!(
                managed_provider.as_ref().map(|value| &value.status),
                Some(ManagedProviderStatus::Revoked)
            );
        Ok(AuthSummary {
            ready,
            providers_count,
            default_model,
            managed_provider,
        })
    }
}
pub fn register_auth_legacy_service() {
    register_scoped_service(
        LifecycleScope::App,
        AUTH_LEGACY_SERVICE_ID,
        SyncDescriptor::new(|accessor| {
            let service: Arc<dyn AuthLegacyServiceContract> = Arc::new(AuthLegacyService::new(
                (*accessor.get(PROVIDER_SERVICE_ID)?).clone(),
                (*accessor.get(CONFIG_SERVICE_ID)?).clone(),
                (*accessor.get(OAUTH_SERVICE_ID)?).clone(),
            ));
            Ok(AuthLegacyServiceHandle(service))
        }),
        InstantiationType::Eager,
        "authLegacy",
    );
}
