//! Web-search provider and provider-service contracts.
//!
//! Original: `app/auth/webSearch/webSearch.ts` and
//! `app/auth/webSearch/tools/web-search.ts`.

use std::{error::Error, ops::Deref, sync::Arc};

use async_trait::async_trait;

use crate::_base::{di::instantiation::ServiceIdentifier, utils::abort::AbortSignal};

pub type WebSearchError = Box<dyn Error + Send + Sync>;

#[derive(Clone, Default)]
pub struct WebSearchOptions {
    pub tool_call_id: Option<String>,
    pub signal: Option<AbortSignal>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WebSearchResult {
    pub title: String,
    pub url: String,
    pub snippet: String,
    pub date: Option<String>,
    pub site_name: Option<String>,
}

#[async_trait]
pub trait WebSearchProvider: Send + Sync {
    async fn search(
        &self,
        query: &str,
        options: Option<WebSearchOptions>,
    ) -> Result<Vec<WebSearchResult>, WebSearchError>;
}

pub type WebSearchProviderHandle = Arc<dyn WebSearchProvider>;

pub trait WebSearchProviderServiceContract: Send + Sync {
    fn get_web_search_provider(&self) -> Option<WebSearchProviderHandle>;
}

#[derive(Clone)]
pub struct WebSearchProviderServiceHandle(pub Arc<dyn WebSearchProviderServiceContract>);

impl Deref for WebSearchProviderServiceHandle {
    type Target = dyn WebSearchProviderServiceContract;

    fn deref(&self) -> &Self::Target {
        self.0.as_ref()
    }
}

pub const WEB_SEARCH_PROVIDER_SERVICE_ID: ServiceIdentifier<WebSearchProviderServiceHandle> =
    ServiceIdentifier::new("webSearchProviderService");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_identifier_matches_the_source_decorator() {
        assert_eq!(
            WEB_SEARCH_PROVIDER_SERVICE_ID.to_string(),
            "webSearchProviderService"
        );
    }
}
