//! Host-injected URL fetcher contract.
//! Original: `packages/agent-core-v2/src/app/web/tools/fetch-url-types.ts`.

use std::{error::Error, fmt, sync::Arc};

use async_trait::async_trait;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UrlFetchKind {
    Passthrough,
    Extracted,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UrlFetchResult {
    pub content: String,
    pub kind: UrlFetchKind,
}

#[derive(Clone, Debug, Default)]
pub struct UrlFetchOptions {
    pub tool_call_id: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HttpFetchError {
    pub status: u16,
    pub message: String,
}

impl fmt::Display for HttpFetchError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for HttpFetchError {}

pub type UrlFetchError = Box<dyn Error + Send + Sync>;

#[async_trait]
pub trait UrlFetcher: Send + Sync {
    async fn fetch(
        &self,
        url: &str,
        options: Option<UrlFetchOptions>,
    ) -> Result<UrlFetchResult, UrlFetchError>;
}

pub type UrlFetcherHandle = Arc<dyn UrlFetcher>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_retains_status_and_message() {
        let error = HttpFetchError {
            status: 418,
            message: "teapot".into(),
        };
        assert_eq!(error.to_string(), "teapot");
        assert_eq!(error.status, 418);
    }
}
