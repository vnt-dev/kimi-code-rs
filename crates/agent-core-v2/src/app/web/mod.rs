//! URL fetching domain.
//! Original: `packages/agent-core-v2/src/app/web`.

pub mod contract;
pub mod fetch_url_types;
pub mod local_fetch_url;
pub mod moonshot_fetch_url;
pub mod service;

pub use contract::*;
pub use fetch_url_types::*;
pub use local_fetch_url::{LocalFetchUrlProvider, LocalFetchUrlProviderOptions};
pub use moonshot_fetch_url::MoonshotFetchUrlProvider;
pub use service::{WebFetchService, register_web_fetch_service};
