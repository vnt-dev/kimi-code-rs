//! Moonshot web-search provider.
//!
//! Original:
//! `app/auth/webSearch/providers/moonshot-web-search.ts`.

use indexmap::IndexMap;
use kimi_code_oauth::{BearerTokenProvider, OAuthManagerError};
use reqwest::{
    Client, Response,
    header::{AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderName, HeaderValue},
};
use serde_json::Value;

use crate::app::auth::web_search::{
    WebSearchError, WebSearchOptions, WebSearchProvider, WebSearchResult,
};

#[derive(Debug, thiserror::Error)]
pub enum MoonshotWebSearchError {
    #[error("Search cancelled: {0}")]
    Cancelled(String),
    #[error("Moonshot search request failed: HTTP {status} (auth/unauthorized). {detail}")]
    Unauthorized { status: u16, detail: String },
    #[error("Moonshot search request failed: HTTP {status}. {detail}")]
    HttpStatus { status: u16, detail: String },
    #[error(transparent)]
    Request(#[from] reqwest::Error),
    #[error("Moonshot search service is not configured: missing API key or token provider.")]
    NotConfigured,
    #[error(transparent)]
    Token(#[from] OAuthManagerError),
    #[error(transparent)]
    HeaderValue(#[from] reqwest::header::InvalidHeaderValue),
    #[error(transparent)]
    HeaderName(#[from] reqwest::header::InvalidHeaderName),
}

pub struct MoonshotWebSearchProviderOptions {
    pub token_provider: Option<BearerTokenProvider>,
    pub api_key: Option<String>,
    pub base_url: String,
    pub default_headers: Option<IndexMap<String, String>>,
    pub custom_headers: Option<IndexMap<String, String>>,
    pub http_client: Option<Client>,
}

pub struct MoonshotWebSearchProvider {
    token_provider: Option<BearerTokenProvider>,
    api_key: Option<String>,
    base_url: String,
    default_headers: IndexMap<String, String>,
    custom_headers: IndexMap<String, String>,
    client: Client,
}

impl MoonshotWebSearchProvider {
    pub fn new(options: MoonshotWebSearchProviderOptions) -> Self {
        Self {
            token_provider: options.token_provider,
            api_key: options.api_key,
            base_url: options.base_url,
            default_headers: options.default_headers.unwrap_or_default(),
            custom_headers: options.custom_headers.unwrap_or_default(),
            client: options.http_client.unwrap_or_default(),
        }
    }

    async fn post(
        &self,
        body: &Value,
        options: &WebSearchOptions,
    ) -> Result<Response, WebSearchError> {
        let access_token = self.resolve_api_key().await?;
        let headers = request_headers(
            &self.default_headers,
            &self.custom_headers,
            &access_token,
            options.tool_call_id.as_deref(),
        )?;
        let request = self
            .client
            .post(&self.base_url)
            .headers(headers)
            .json(body)
            .send();
        if let Some(signal) = &options.signal {
            tokio::select! {
                reason = signal.cancelled() => Err(Box::new(
                    MoonshotWebSearchError::Cancelled(reason.to_string()),
                )),
                response = request => Ok(response?),
            }
        } else {
            Ok(request.await?)
        }
    }

    async fn resolve_api_key(&self) -> Result<String, WebSearchError> {
        if let Some(provider) = &self.token_provider {
            match provider.get_access_token(false).await {
                Ok(token) => return Ok(token),
                Err(_) if self.api_key.as_deref().is_some_and(|key| !key.is_empty()) => {
                    return Ok(self.api_key.clone().expect("checked as present"));
                }
                Err(error) => return Err(Box::new(MoonshotWebSearchError::Token(error))),
            }
        }
        self.api_key
            .clone()
            .filter(|key| !key.is_empty())
            .ok_or_else(|| Box::new(MoonshotWebSearchError::NotConfigured) as WebSearchError)
    }
}

#[async_trait::async_trait]
impl WebSearchProvider for MoonshotWebSearchProvider {
    async fn search(
        &self,
        query: &str,
        options: Option<WebSearchOptions>,
    ) -> Result<Vec<WebSearchResult>, WebSearchError> {
        let options = options.unwrap_or_default();
        let response = self
            .post(&serde_json::json!({"text_query": query}), &options)
            .await?;
        let status = response.status().as_u16();
        if status == 401 {
            let detail = response.text().await.unwrap_or_default();
            return Err(Box::new(MoonshotWebSearchError::Unauthorized {
                status,
                detail: detail.trim().to_owned(),
            }));
        }
        if status != 200 {
            let detail = response.text().await.unwrap_or_default();
            return Err(Box::new(MoonshotWebSearchError::HttpStatus {
                status,
                detail: detail.trim().to_owned(),
            }));
        }

        let json: Value = response.json().await?;
        let results = json
            .get("search_results")
            .and_then(Value::as_array)
            .map_or_else(Vec::new, |results| {
                results.iter().map(search_result_from_json).collect()
            });
        Ok(results)
    }
}

fn request_headers(
    default_headers: &IndexMap<String, String>,
    custom_headers: &IndexMap<String, String>,
    access_token: &str,
    tool_call_id: Option<&str>,
) -> Result<HeaderMap, WebSearchError> {
    let mut headers = HeaderMap::new();
    extend_headers(&mut headers, default_headers)?;
    headers.insert(
        AUTHORIZATION,
        HeaderValue::from_str(&format!("Bearer {access_token}"))?,
    );
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    if let Some(tool_call_id) = tool_call_id.filter(|id| !id.is_empty()) {
        headers.insert(
            HeaderName::from_static("x-msh-tool-call-id"),
            HeaderValue::from_str(tool_call_id)?,
        );
    }
    extend_headers(&mut headers, custom_headers)?;
    Ok(headers)
}

fn extend_headers(
    target: &mut HeaderMap,
    source: &IndexMap<String, String>,
) -> Result<(), WebSearchError> {
    for (name, value) in source {
        target.insert(
            HeaderName::from_bytes(name.as_bytes())?,
            HeaderValue::from_str(value)?,
        );
    }
    Ok(())
}

fn search_result_from_json(value: &Value) -> WebSearchResult {
    let string = |key: &str| {
        value
            .get(key)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned()
    };
    let optional_string = |key: &str| {
        value
            .get(key)
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
    };
    WebSearchResult {
        title: string("title"),
        url: string("url"),
        snippet: string("snippet"),
        date: optional_string("date"),
        site_name: optional_string("site_name"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };

    #[test]
    fn maps_source_results_and_omits_empty_optional_fields() {
        assert_eq!(
            search_result_from_json(&serde_json::json!({
                "title": "Result",
                "url": "https://example.test",
                "snippet": "Summary",
                "date": "2026-07-27",
                "site_name": "Example",
                "content": "ignored"
            })),
            WebSearchResult {
                title: "Result".into(),
                url: "https://example.test".into(),
                snippet: "Summary".into(),
                date: Some("2026-07-27".into()),
                site_name: Some("Example".into()),
            }
        );
        let empty = search_result_from_json(&serde_json::json!({
            "date": "",
            "site_name": ""
        }));
        assert_eq!(empty.title, "");
        assert_eq!(empty.date, None);
        assert_eq!(empty.site_name, None);
    }

    #[test]
    fn custom_headers_override_defaults_and_request_headers() {
        let headers = request_headers(
            &IndexMap::from([("X-Client".into(), "default".into())]),
            &IndexMap::from([
                ("X-Client".into(), "custom".into()),
                ("Authorization".into(), "Bearer override".into()),
            ]),
            "token",
            Some("call-1"),
        )
        .unwrap();
        assert_eq!(headers["x-client"], "custom");
        assert_eq!(headers[AUTHORIZATION], "Bearer override");
        assert_eq!(headers["x-msh-tool-call-id"], "call-1");
        assert_eq!(headers[CONTENT_TYPE], "application/json");
    }

    #[tokio::test]
    async fn posts_authenticated_query_and_maps_the_moonshot_response() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let mut buffer = [0_u8; 2048];
            loop {
                let count = stream.read(&mut buffer).await.unwrap();
                if count == 0 {
                    break;
                }
                request.extend_from_slice(&buffer[..count]);
                let Some(header_end) = request.windows(4).position(|bytes| bytes == b"\r\n\r\n")
                else {
                    continue;
                };
                let headers = String::from_utf8_lossy(&request[..header_end]).to_lowercase();
                let content_length = headers
                    .lines()
                    .find_map(|line| line.strip_prefix("content-length:"))
                    .and_then(|value| value.trim().parse::<usize>().ok())
                    .unwrap_or_default();
                if request.len() >= header_end + 4 + content_length {
                    break;
                }
            }
            let request = String::from_utf8(request).unwrap();
            let lower = request.to_lowercase();
            assert!(lower.starts_with("post /search http/1.1\r\n"));
            assert!(lower.contains("\r\nauthorization: bearer test-key\r\n"));
            assert!(lower.contains("\r\nx-msh-tool-call-id: call-7\r\n"));
            assert!(lower.contains("\r\nx-client: desktop\r\n"));
            assert!(request.ends_with(r#"{"text_query":"rust async"}"#));

            let body = serde_json::json!({
                "search_results": [{
                    "title": "Rust",
                    "url": "https://www.rust-lang.org",
                    "snippet": "A language",
                    "site_name": "Rust",
                    "date": "2026-07-27"
                }]
            })
            .to_string();
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(response.as_bytes()).await.unwrap();
        });

        let provider = MoonshotWebSearchProvider::new(MoonshotWebSearchProviderOptions {
            token_provider: None,
            api_key: Some("test-key".into()),
            base_url: format!("http://{address}/search"),
            default_headers: Some(IndexMap::from([("X-Client".into(), "desktop".into())])),
            custom_headers: None,
            http_client: None,
        });
        let results = provider
            .search(
                "rust async",
                Some(WebSearchOptions {
                    tool_call_id: Some("call-7".into()),
                    signal: None,
                }),
            )
            .await
            .unwrap();
        server.await.unwrap();
        assert_eq!(
            results,
            vec![WebSearchResult {
                title: "Rust".into(),
                url: "https://www.rust-lang.org".into(),
                snippet: "A language".into(),
                date: Some("2026-07-27".into()),
                site_name: Some("Rust".into()),
            }]
        );
    }
}
