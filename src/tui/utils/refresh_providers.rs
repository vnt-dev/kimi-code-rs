pub use crate::oauth::refresh_provider_models::{
    ProviderChange, ProviderRefreshFailure, RefreshHostError, RefreshProviderHost,
    RefreshProviderOptions, RefreshProviderScope, RefreshResult,
};

use crate::oauth::refresh_provider_models::refresh_provider_models;

/// Original:
///   apps/kimi-code/src/tui/utils/refresh-providers.ts
///   refreshAllProviderModels()
///
/// Rust uses the same structured config map at the TUI and OAuth boundaries,
/// so the TypeScript-only cast adapter becomes a typed delegation.
pub async fn refresh_all_provider_models(
    host: &dyn RefreshProviderHost,
    options: RefreshProviderOptions,
) -> Result<RefreshResult, RefreshHostError> {
    refresh_provider_models(host, &options).await
}

#[cfg(test)]
mod tests {
    use async_trait::async_trait;
    use serde_json::{Map, Value};

    use super::*;
    use crate::oauth::managed_auth::ManagedKimiOAuthRef;

    #[derive(Default)]
    struct EmptyHost;

    #[async_trait]
    impl RefreshProviderHost for EmptyHost {
        async fn get_config(&self) -> Result<Map<String, Value>, RefreshHostError> {
            Ok(Map::new())
        }

        async fn remove_provider(&self, _: &str) -> Result<Map<String, Value>, RefreshHostError> {
            panic!("empty config has no provider to remove")
        }

        async fn set_config(
            &self,
            _: Map<String, Value>,
        ) -> Result<Map<String, Value>, RefreshHostError> {
            panic!("empty config has no provider changes")
        }

        async fn resolve_oauth_token(
            &self,
            _: &str,
            _: Option<&ManagedKimiOAuthRef>,
        ) -> Result<String, RefreshHostError> {
            panic!("empty config has no OAuth provider")
        }

        fn user_agent(&self) -> Option<&str> {
            Some("kimi-code-cli/test")
        }
    }

    #[tokio::test]
    async fn delegates_typed_tui_host_to_shared_refresh_orchestrator() {
        let result = refresh_all_provider_models(
            &EmptyHost,
            RefreshProviderOptions {
                scope: RefreshProviderScope::All,
                provider_id: None,
            },
        )
        .await
        .expect("refresh");
        assert_eq!(result, RefreshResult::default());
    }
}
