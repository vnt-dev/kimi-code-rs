//! Host-provided default headers for outbound model-provider requests.
//!
//! Original: `packages/agent-core-v2/src/kosong/model/hostRequestHeaders.ts`.

use std::sync::Arc;

use indexmap::IndexMap;

use crate::_base::di::{
    descriptors::SyncDescriptor,
    instantiation::ServiceIdentifier,
    scope::{InstantiationType, LifecycleScope, register_scoped_service},
    service_collection::ServiceCollection,
};

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct HostRequestHeaders {
    pub headers: IndexMap<String, String>,
}

impl HostRequestHeaders {
    // Original: HostRequestHeaders.constructor().
    pub fn new(headers: IndexMap<String, String>) -> Self {
        Self { headers }
    }
}

pub const HOST_REQUEST_HEADERS_ID: ServiceIdentifier<HostRequestHeaders> =
    ServiceIdentifier::new("hostRequestHeaders");

// Original: hostRequestHeadersSeed(). `ServiceCollection` is the Rust
// scope-seed representation consumed by `ScopeOptions.extra`.
pub fn host_request_headers_seed(headers: IndexMap<String, String>) -> ServiceCollection {
    let mut seed = ServiceCollection::new();
    seed.set_instance(
        HOST_REQUEST_HEADERS_ID,
        Arc::new(HostRequestHeaders::new(headers)),
    );
    seed
}

// Original: registerScopedService(LifecycleScope.App, ..., Eager, 'model').
// Rust registrations are explicit composition-root calls rather than module
// import side effects.
pub fn register_host_request_headers() {
    register_scoped_service(
        LifecycleScope::App,
        HOST_REQUEST_HEADERS_ID,
        SyncDescriptor::new(|_| Ok(HostRequestHeaders::default())),
        InstantiationType::Eager,
        "model",
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seed_preserves_header_order_and_identity() {
        let headers = IndexMap::from([
            ("User-Agent".to_owned(), "kimi-code-test".to_owned()),
            ("X-Msh-Device-Id".to_owned(), "device-1".to_owned()),
        ]);
        let seed = host_request_headers_seed(headers.clone());

        let value = seed.get(HOST_REQUEST_HEADERS_ID).unwrap().unwrap();
        assert_eq!(value.headers, headers);
        assert_eq!(
            value.headers.keys().collect::<Vec<_>>(),
            vec!["User-Agent", "X-Msh-Device-Id"]
        );
        assert_eq!(HOST_REQUEST_HEADERS_ID.to_string(), "hostRequestHeaders");
    }
}
