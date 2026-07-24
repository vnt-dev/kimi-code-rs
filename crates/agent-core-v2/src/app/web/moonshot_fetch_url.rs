//! OAuth-backed Moonshot URL fetcher with a local fallback.
//!
//! Original: `packages/agent-core-v2/src/app/web/providers/moonshot-fetch-url.ts`.

use async_trait::async_trait;
use indexmap::IndexMap;
use kimi_code_oauth::BearerTokenProvider;
use reqwest::{
    Client,
    header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderName, HeaderValue},
};

use super::{
    HttpFetchError, UrlFetchError, UrlFetchKind, UrlFetchOptions, UrlFetchResult, UrlFetcher,
    UrlFetcherHandle,
};

pub struct MoonshotFetchUrlProvider {
    token_provider: Option<BearerTokenProvider>,
    api_key: Option<String>,
    base_url: String,
    default_headers: IndexMap<String, String>,
    custom_headers: IndexMap<String, String>,
    local_fallback: UrlFetcherHandle,
    client: Client,
}

impl MoonshotFetchUrlProvider {
    pub fn new(
        token_provider: Option<BearerTokenProvider>,
        api_key: Option<String>,
        base_url: String,
        default_headers: IndexMap<String, String>,
        custom_headers: IndexMap<String, String>,
        local_fallback: UrlFetcherHandle,
    ) -> Self {
        Self {
            token_provider,
            api_key,
            base_url,
            default_headers,
            custom_headers,
            local_fallback,
            client: Client::new(),
        }
    }

    async fn resolve_api_key(&self) -> Result<String, UrlFetchError> {
        if let Some(provider) = &self.token_provider {
            match provider.get_access_token(false).await {
                Ok(token) if !token.trim().is_empty() => return Ok(token),
                Ok(_) | Err(_) if self.api_key.as_deref().is_some_and(|key| !key.is_empty()) => {
                    return Ok(self.api_key.clone().unwrap());
                }
                Err(error) => return Err(Box::new(error)),
                Ok(_) => {}
            }
        }
        self.api_key
            .clone()
            .filter(|key| !key.is_empty())
            .ok_or_else(|| {
                Box::new(std::io::Error::other(
                    "Moonshot fetch service is not configured: missing API key or token provider.",
                )) as UrlFetchError
            })
    }

    async fn fetch_via_moonshot(
        &self,
        url: &str,
        options: &UrlFetchOptions,
    ) -> Result<String, UrlFetchError> {
        let token = self.resolve_api_key().await?;
        let mut headers = header_map(&self.default_headers)?;
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {token}"))?,
        );
        headers.insert(ACCEPT, HeaderValue::from_static("text/markdown"));
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        if let Some(call_id) = options.tool_call_id.as_deref().filter(|id| !id.is_empty()) {
            headers.insert(
                HeaderName::from_static("x-msh-tool-call-id"),
                HeaderValue::from_str(call_id)?,
            );
        }
        for (name, value) in header_map(&self.custom_headers)? {
            if let Some(name) = name {
                headers.insert(name, value);
            }
        }
        let request = self
            .client
            .post(&self.base_url)
            .headers(headers)
            .json(&serde_json::json!({"url":url}))
            .send();
        let response = if let Some(cancel) = &options.cancellation {
            tokio::select! { _ = cancel.cancelled() => return Err(Box::new(std::io::Error::new(std::io::ErrorKind::Interrupted,"URL fetch cancelled"))), response = request => response? }
        } else {
            request.await?
        };
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        if status.as_u16() != 200 {
            return Err(Box::new(HttpFetchError {
                status: status.as_u16(),
                message: format!(
                    "Moonshot fetch request failed: HTTP {}. {}",
                    status.as_u16(),
                    body
                )
                .trim()
                .into(),
            }));
        }
        Ok(body)
    }
}

#[async_trait]
impl UrlFetcher for MoonshotFetchUrlProvider {
    async fn fetch(
        &self,
        url: &str,
        options: Option<UrlFetchOptions>,
    ) -> Result<UrlFetchResult, UrlFetchError> {
        let options = options.unwrap_or_default();
        match self.fetch_via_moonshot(url, &options).await {
            Ok(content) => Ok(UrlFetchResult {
                content,
                kind: UrlFetchKind::Extracted,
            }),
            Err(error)
                if options
                    .cancellation
                    .as_ref()
                    .is_some_and(tokio_util::sync::CancellationToken::is_cancelled) =>
            {
                Err(error)
            }
            Err(_) => self.local_fallback.fetch(url, Some(options)).await,
        }
    }
}

fn header_map(headers: &IndexMap<String, String>) -> Result<HeaderMap, UrlFetchError> {
    let mut out = HeaderMap::new();
    for (name, value) in headers {
        out.insert(
            HeaderName::from_bytes(name.as_bytes())?,
            HeaderValue::from_str(value)?,
        );
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn header_map_accepts_source_header_records() {
        let headers = header_map(&IndexMap::from([("X-Client".into(), "kimi".into())])).unwrap();
        assert_eq!(headers["x-client"], "kimi");
    }
}
