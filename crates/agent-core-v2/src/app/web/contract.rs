//! URL fetch service contract.
//! Original: `packages/agent-core-v2/src/app/web/web.ts`, `IWebFetchService`.

use std::{ops::Deref, sync::Arc};

use crate::_base::di::instantiation::ServiceIdentifier;

use super::UrlFetcherHandle;

pub trait WebFetchServiceContract: Send + Sync {
    /// Returns the currently configured fetcher. This is synchronous because
    /// the source resolves its provider state lazily without I/O.
    fn get_url_fetcher(&self) -> UrlFetcherHandle;
}

#[derive(Clone)]
pub struct WebFetchServiceHandle(pub Arc<dyn WebFetchServiceContract>);

impl Deref for WebFetchServiceHandle {
    type Target = dyn WebFetchServiceContract;

    fn deref(&self) -> &Self::Target {
        self.0.as_ref()
    }
}

pub const WEB_FETCH_SERVICE_ID: ServiceIdentifier<WebFetchServiceHandle> =
    ServiceIdentifier::new("webFetchService");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_identifier_matches_source() {
        assert_eq!(WEB_FETCH_SERVICE_ID.to_string(), "webFetchService");
    }
}
