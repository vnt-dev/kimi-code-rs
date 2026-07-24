//! OAuth-aware URL fetcher routing service.
//! Original: `packages/agent-core-v2/src/app/web/webService.ts`,
//! `WebFetchService.getUrlFetcher()`.
//!
//! Provider state is read on every call, preserving login/logout visibility.
//! The local provider is constructed once and used whenever the managed Kimi
//! OAuth provider is absent, not an OAuth catalog vendor, or has no token.
use std::sync::Arc;

use kimi_code_oauth::{KIMI_CODE_PROVIDER_NAME, kimi_code_base_url};

use crate::{
    _base::di::{
        descriptors::SyncDescriptor,
        instantiation::ServicesAccessorExt,
        scope::{InstantiationType, LifecycleScope, register_scoped_service},
    },
    app::auth::{OAUTH_SERVICE_ID, OAuthServiceHandle},
    kosong::{
        model::host_request_headers::{HOST_REQUEST_HEADERS_ID, HostRequestHeaders},
        provider::{
            PROVIDER_SERVICE_ID, ProviderServiceHandle,
            provider_definition::is_oauth_catalog_vendor,
        },
    },
};

use super::{
    LocalFetchUrlProvider, MoonshotFetchUrlProvider, UrlFetcherHandle, WEB_FETCH_SERVICE_ID,
    WebFetchServiceContract, WebFetchServiceHandle,
};

pub struct WebFetchService {
    providers: ProviderServiceHandle,
    oauth: OAuthServiceHandle,
    host_headers: HostRequestHeaders,
    local_fetcher: UrlFetcherHandle,
}

impl WebFetchService {
    pub fn new(
        providers: ProviderServiceHandle,
        oauth: OAuthServiceHandle,
        host_headers: HostRequestHeaders,
    ) -> Self {
        Self {
            providers,
            oauth,
            host_headers,
            local_fetcher: Arc::new(LocalFetchUrlProvider::default()),
        }
    }
}

impl WebFetchServiceContract for WebFetchService {
    fn get_url_fetcher(&self) -> UrlFetcherHandle {
        let Some(provider) = self.providers.get(KIMI_CODE_PROVIDER_NAME) else {
            return Arc::clone(&self.local_fetcher);
        };
        let provider_type = provider.provider_type.as_ref().map(|value| value.as_str());
        if provider.oauth.is_none() || !is_oauth_catalog_vendor(provider_type).unwrap_or(false) {
            return Arc::clone(&self.local_fetcher);
        }
        let Some(token_provider) = self
            .oauth
            .resolve_token_provider(KIMI_CODE_PROVIDER_NAME, provider.oauth.as_ref())
        else {
            return Arc::clone(&self.local_fetcher);
        };
        Arc::new(MoonshotFetchUrlProvider::new(
            Some(token_provider),
            None,
            moonshot_fetch_url(provider.base_url.as_deref()),
            self.host_headers.headers.clone(),
            provider.custom_headers.unwrap_or_default(),
            Arc::clone(&self.local_fetcher),
        ))
    }
}

fn moonshot_fetch_url(base_url: Option<&str>) -> String {
    format!(
        "{}/fetch",
        base_url
            .unwrap_or(&kimi_code_base_url())
            .trim_end_matches('/')
    )
}

pub fn register_web_fetch_service() {
    register_scoped_service(
        LifecycleScope::App,
        WEB_FETCH_SERVICE_ID,
        SyncDescriptor::new(|accessor| {
            let providers = accessor.get(PROVIDER_SERVICE_ID)?;
            let oauth = accessor.get(OAUTH_SERVICE_ID)?;
            let host_headers = accessor.get(HOST_REQUEST_HEADERS_ID)?;
            Ok(WebFetchServiceHandle(Arc::new(WebFetchService::new(
                (*providers).clone(),
                (*oauth).clone(),
                (*host_headers).clone(),
            ))))
        }),
        InstantiationType::Eager,
        "web",
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_source_base_url_before_adding_fetch_path() {
        assert_eq!(
            moonshot_fetch_url(Some("https://api.example.test/v1///")),
            "https://api.example.test/v1/fetch"
        );
    }
}
