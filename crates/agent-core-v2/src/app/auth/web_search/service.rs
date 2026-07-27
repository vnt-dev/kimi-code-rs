//! OAuth-aware web-search provider service.
//!
//! Original: `app/auth/webSearch/webSearchService.ts`.

use std::sync::Arc;

use kimi_code_oauth::{KIMI_CODE_PROVIDER_NAME, kimi_code_base_url};

use crate::{
    _base::di::{
        descriptors::SyncDescriptor,
        instantiation::ServicesAccessorExt,
        scope::{InstantiationType, LifecycleScope, register_scoped_service},
    },
    app::{
        auth::{
            OAUTH_SERVICE_ID, OAuthServiceHandle,
            config_section::{MoonshotServiceConfig, SERVICES_SECTION},
            web_search::{
                MoonshotWebSearchProvider, MoonshotWebSearchProviderOptions,
                WEB_SEARCH_PROVIDER_SERVICE_ID, WebSearchProviderHandle,
                WebSearchProviderServiceContract, WebSearchProviderServiceHandle,
            },
        },
        config::{CONFIG_SERVICE_ID, ConfigServiceHandle},
    },
    kosong::{
        model::host_request_headers::{HOST_REQUEST_HEADERS_ID, HostRequestHeaders},
        provider::{
            PROVIDER_SERVICE_ID, ProviderServiceHandle,
            provider_definition::is_oauth_catalog_vendor,
        },
    },
};

pub struct WebSearchProviderService {
    providers: ProviderServiceHandle,
    oauth: OAuthServiceHandle,
    host_headers: HostRequestHeaders,
    config: ConfigServiceHandle,
}

impl WebSearchProviderService {
    pub fn new(
        providers: ProviderServiceHandle,
        oauth: OAuthServiceHandle,
        host_headers: HostRequestHeaders,
        config: ConfigServiceHandle,
    ) -> Self {
        Self {
            providers,
            oauth,
            host_headers,
            config,
        }
    }

    fn provider_from_services_config(&self) -> Option<WebSearchProviderHandle> {
        let search = configured_search(&self.config)?;
        let base_url = search.base_url?;
        let token_provider = search.oauth.as_ref().and_then(|oauth_ref| {
            self.oauth
                .resolve_token_provider(KIMI_CODE_PROVIDER_NAME, Some(oauth_ref))
        });
        Some(Arc::new(MoonshotWebSearchProvider::new(
            MoonshotWebSearchProviderOptions {
                token_provider,
                api_key: non_empty_string(search.api_key),
                base_url,
                default_headers: Some(self.host_headers.headers.clone()),
                custom_headers: search.custom_headers,
                http_client: None,
            },
        )))
    }

    fn provider_from_managed_oauth(&self) -> Option<WebSearchProviderHandle> {
        let provider = self.providers.get(KIMI_CODE_PROVIDER_NAME)?;
        let provider_type = provider.provider_type.as_ref().map(|value| value.as_str());
        if provider.oauth.is_none() || !is_oauth_catalog_vendor(provider_type).unwrap_or(false) {
            return None;
        }
        let token_provider = self
            .oauth
            .resolve_token_provider(KIMI_CODE_PROVIDER_NAME, provider.oauth.as_ref())?;
        Some(Arc::new(MoonshotWebSearchProvider::new(
            MoonshotWebSearchProviderOptions {
                token_provider: Some(token_provider),
                api_key: None,
                base_url: moonshot_search_url(provider.base_url.as_deref()),
                default_headers: Some(self.host_headers.headers.clone()),
                custom_headers: provider.custom_headers,
                http_client: None,
            },
        )))
    }
}

impl WebSearchProviderServiceContract for WebSearchProviderService {
    fn get_web_search_provider(&self) -> Option<WebSearchProviderHandle> {
        self.provider_from_services_config()
            .or_else(|| self.provider_from_managed_oauth())
    }
}

fn configured_search(config: &ConfigServiceHandle) -> Option<MoonshotServiceConfig> {
    let services = config.get(SERVICES_SECTION)?;
    serde_json::from_value(services.get("moonshotSearch")?.clone()).ok()
}

fn moonshot_search_url(base_url: Option<&str>) -> String {
    format!(
        "{}/search",
        base_url
            .unwrap_or(&kimi_code_base_url())
            .trim_end_matches('/')
    )
}

fn non_empty_string(value: Option<String>) -> Option<String> {
    value.and_then(|value| {
        let value = value.trim();
        (!value.is_empty()).then(|| value.to_owned())
    })
}

pub fn register_web_search_provider_service() {
    register_scoped_service(
        LifecycleScope::App,
        WEB_SEARCH_PROVIDER_SERVICE_ID,
        SyncDescriptor::new(|accessor| {
            let providers = accessor.get(PROVIDER_SERVICE_ID)?;
            let oauth = accessor.get(OAUTH_SERVICE_ID)?;
            let host_headers = accessor.get(HOST_REQUEST_HEADERS_ID)?;
            let config = accessor.get(CONFIG_SERVICE_ID)?;
            Ok(WebSearchProviderServiceHandle(Arc::new(
                WebSearchProviderService::new(
                    (*providers).clone(),
                    (*oauth).clone(),
                    (*host_headers).clone(),
                    (*config).clone(),
                ),
            )))
        }),
        InstantiationType::Eager,
        "auth",
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_managed_provider_url_and_configured_api_key() {
        assert_eq!(
            moonshot_search_url(Some("https://api.example.test/v1///")),
            "https://api.example.test/v1/search"
        );
        assert_eq!(non_empty_string(Some(" key ".into())), Some("key".into()));
        assert_eq!(non_empty_string(Some("  ".into())), None);
        assert_eq!(non_empty_string(None), None);
    }
}
