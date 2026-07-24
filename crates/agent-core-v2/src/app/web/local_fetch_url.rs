//! Local, SSRF-hardened URL fetcher.
//! Original: `packages/agent-core-v2/src/app/web/providers/local-fetch-url.ts`,
//! `LocalFetchURLProvider`.
//!
//! Rust adaptation: reqwest owns each HTTP response body rather than exposing
//! a cancellable Node stream. Redirects remain manual, and approved DNS
//! answers are installed as a per-request resolver override unless a proxy
//! will carry the request.
use std::{
    io::{self, Cursor},
    net::{IpAddr, SocketAddr},
};

use async_trait::async_trait;
use futures_util::StreamExt;
use reqwest::{Client, StatusCode, redirect::Policy};
use scraper::{Html, Selector};
use tokio_util::sync::CancellationToken;
use url::Url;

use crate::_base::utils::proxy::{
    Env, is_proxy_configured, make_no_proxy_matcher, resolve_no_proxy,
};

use super::{
    HttpFetchError, UrlFetchError, UrlFetchKind, UrlFetchOptions, UrlFetchResult, UrlFetcher,
};

const DEFAULT_USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/91.0.4472.124 Safari/537.36";
const DEFAULT_MAX_BYTES: u64 = 10 * 1024 * 1024;
const MAX_REDIRECT_HOPS: usize = 10;

#[derive(Clone, Debug, Default)]
pub struct LocalFetchUrlProviderOptions {
    pub user_agent: Option<String>,
    pub max_bytes: Option<u64>,
    pub allow_private_addresses: Option<bool>,
}

#[derive(Clone, Debug)]
pub struct LocalFetchUrlProvider {
    user_agent: String,
    max_bytes: u64,
    allow_private_addresses: bool,
    environment: Env,
}

impl Default for LocalFetchUrlProvider {
    fn default() -> Self {
        Self::new(LocalFetchUrlProviderOptions::default())
    }
}

impl LocalFetchUrlProvider {
    pub fn new(options: LocalFetchUrlProviderOptions) -> Self {
        Self::with_environment(options, std::env::vars().collect())
    }

    pub(crate) fn with_environment(
        options: LocalFetchUrlProviderOptions,
        environment: Env,
    ) -> Self {
        Self {
            user_agent: options
                .user_agent
                .unwrap_or_else(|| DEFAULT_USER_AGENT.into()),
            max_bytes: options.max_bytes.unwrap_or(DEFAULT_MAX_BYTES),
            allow_private_addresses: options.allow_private_addresses.unwrap_or(false),
            environment,
        }
    }

    async fn request_with_validated_redirects(
        &self,
        original_url: &str,
        cancellation: Option<&CancellationToken>,
    ) -> Result<reqwest::Response, UrlFetchError> {
        let mut current_url = original_url.to_owned();
        let mut redirects = 0;
        loop {
            check(cancellation)?;
            let target =
                resolve_safe_fetch_target(&current_url, self.allow_private_addresses).await?;
            let client = self.client_for(&target)?;
            let request = client
                .get(&current_url)
                .header(reqwest::header::USER_AGENT, &self.user_agent)
                .send();
            let response = await_cancellable(request, cancellation).await??;
            if !is_redirect(response.status()) {
                return Ok(response);
            }
            let Some(location) = response.headers().get(reqwest::header::LOCATION) else {
                return Ok(response);
            };
            let location = location
                .to_str()
                .map_err(|error| Box::new(error) as UrlFetchError)?;
            if redirects >= MAX_REDIRECT_HOPS {
                return Err(Box::new(io::Error::other(format!(
                    "Too many redirects while fetching \"{original_url}\" (limit {MAX_REDIRECT_HOPS})."
                ))));
            }
            redirects += 1;
            current_url = Url::parse(&current_url)
                .and_then(|base| base.join(location))
                .map_err(|error| Box::new(error) as UrlFetchError)?
                .to_string();
        }
    }

    fn client_for(&self, target: &SafeFetchTarget) -> Result<Client, UrlFetchError> {
        let mut builder = Client::builder().redirect(Policy::none());
        if let Some(addresses) = &target.addresses
            && self.should_pin(target)
        {
            builder = builder.resolve_to_addrs(&target.host, addresses);
        }
        builder
            .build()
            .map_err(|error| Box::new(error) as UrlFetchError)
    }

    fn should_pin(&self, target: &SafeFetchTarget) -> bool {
        !is_proxy_configured(&self.environment)
            || make_no_proxy_matcher(&resolve_no_proxy(&self.environment))
                .is_match(&target.host, Some(target.port))
    }

    async fn read_response(
        &self,
        response: reqwest::Response,
        cancellation: Option<&CancellationToken>,
    ) -> Result<UrlFetchResult, UrlFetchError> {
        let status = response.status();
        if status.as_u16() >= 400 {
            return Err(Box::new(HttpFetchError {
                status: status.as_u16(),
                message: format!(
                    "HTTP {} {}",
                    status.as_u16(),
                    status.canonical_reason().unwrap_or_default()
                )
                .trim()
                .to_owned(),
            }));
        }
        if let Some(length) = response
            .headers()
            .get(reqwest::header::CONTENT_LENGTH)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<f64>().ok())
            .filter(|value| value.is_finite() && *value > self.max_bytes as f64)
        {
            return Err(body_too_large(length as u64, self.max_bytes));
        }

        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_ascii_lowercase();
        let response_url = response.url().clone();
        let mut body = Vec::new();
        let mut stream = response.bytes_stream();
        while let Some(chunk) = await_cancellable(stream.next(), cancellation).await? {
            let chunk = chunk.map_err(|error| Box::new(error) as UrlFetchError)?;
            let next_length = u64::try_from(body.len())
                .unwrap_or(u64::MAX)
                .saturating_add(u64::try_from(chunk.len()).unwrap_or(u64::MAX));
            if next_length > self.max_bytes {
                return Err(body_too_large(next_length, self.max_bytes));
            }
            body.extend_from_slice(&chunk);
        }
        let body = String::from_utf8_lossy(&body).into_owned();
        if content_type.starts_with("text/plain") || content_type.starts_with("text/markdown") {
            return Ok(UrlFetchResult {
                content: body,
                kind: UrlFetchKind::Passthrough,
            });
        }
        Ok(UrlFetchResult {
            content: extract_main_content(body, response_url).await?,
            kind: UrlFetchKind::Extracted,
        })
    }
}

#[async_trait]
impl UrlFetcher for LocalFetchUrlProvider {
    async fn fetch(
        &self,
        url: &str,
        options: Option<UrlFetchOptions>,
    ) -> Result<UrlFetchResult, UrlFetchError> {
        let options = options.unwrap_or_default();
        let response = self
            .request_with_validated_redirects(url, options.cancellation.as_ref())
            .await?;
        self.read_response(response, options.cancellation.as_ref())
            .await
    }
}

#[derive(Clone, Debug)]
struct SafeFetchTarget {
    host: String,
    port: u16,
    addresses: Option<Vec<SocketAddr>>,
}

async fn resolve_safe_fetch_target(
    url: &str,
    allow_private: bool,
) -> Result<SafeFetchTarget, UrlFetchError> {
    let parsed = Url::parse(url).map_err(|_| {
        Box::new(io::Error::other(format!("Invalid URL: \"{url}\""))) as UrlFetchError
    })?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(Box::new(io::Error::other(format!(
            "Unsupported URL scheme \"{}:\" — only http(s) allowed.",
            parsed.scheme()
        ))));
    }
    let host = parsed
        .host_str()
        .ok_or_else(|| {
            Box::new(io::Error::other(format!("Invalid URL: \"{url}\""))) as UrlFetchError
        })?
        .to_ascii_lowercase();
    let port = parsed.port_or_known_default().unwrap_or(80);
    if allow_private {
        return Ok(SafeFetchTarget {
            host,
            port,
            addresses: None,
        });
    }
    if let Ok(address) = host.parse::<IpAddr>() {
        if is_blocked_address(address) {
            return Err(Box::new(io::Error::other(format!(
                "Refusing to fetch private address: \"{host}\""
            ))));
        }
        return Ok(SafeFetchTarget {
            host,
            port,
            addresses: None,
        });
    }
    if host == "localhost" || host.ends_with(".localhost") {
        return Err(Box::new(io::Error::other(format!(
            "Refusing to fetch private host: \"{host}\""
        ))));
    }
    let addresses = tokio::net::lookup_host((host.as_str(), port))
        .await
        .map_err(|error| {
            Box::new(io::Error::other(format!(
                "Cannot resolve host \"{host}\" for the fetch safety check: {error}"
            ))) as UrlFetchError
        })?
        .collect::<Vec<_>>();
    for address in &addresses {
        if is_blocked_address(address.ip()) {
            return Err(Box::new(io::Error::other(format!(
                "Refusing to fetch host \"{host}\": resolves to private address \"{}\".",
                address.ip()
            ))));
        }
    }
    Ok(SafeFetchTarget {
        host,
        port,
        addresses: Some(addresses),
    })
}

fn is_blocked_address(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(address) => is_blocked_v4(address.octets()),
        IpAddr::V6(address) => {
            address
                .to_ipv4_mapped()
                .is_some_and(|address| is_blocked_v4(address.octets()))
                || is_blocked_v6(address.segments())
        }
    }
}

fn is_blocked_v4(octets: [u8; 4]) -> bool {
    octets[0] == 0
        || octets[0] == 10
        || octets[0] == 127
        || (octets[0] == 100 && (64..=127).contains(&octets[1]))
        || (octets[0] == 169 && octets[1] == 254)
        || (octets[0] == 172 && (16..=31).contains(&octets[1]))
        || (octets[0] == 192 && octets[1] == 168)
}

fn is_blocked_v6(segments: [u16; 8]) -> bool {
    segments == [0; 8]
        || segments == [0, 0, 0, 0, 0, 0, 0, 1]
        || (segments[0] & 0xfe00) == 0xfc00
        || (segments[0] & 0xffc0) == 0xfe80
}

fn is_redirect(status: StatusCode) -> bool {
    matches!(status.as_u16(), 301 | 302 | 303 | 307 | 308)
}

async fn await_cancellable<F, T>(
    future: F,
    cancellation: Option<&CancellationToken>,
) -> Result<T, UrlFetchError>
where
    F: std::future::Future<Output = T>,
{
    if let Some(cancellation) = cancellation {
        tokio::select! {
            _ = cancellation.cancelled() => Err(Box::new(io::Error::new(io::ErrorKind::Interrupted, "URL fetch cancelled"))),
            value = future => Ok(value),
        }
    } else {
        Ok(future.await)
    }
}

fn check(cancellation: Option<&CancellationToken>) -> Result<(), UrlFetchError> {
    if cancellation.is_some_and(CancellationToken::is_cancelled) {
        Err(Box::new(io::Error::new(
            io::ErrorKind::Interrupted,
            "URL fetch cancelled",
        )))
    } else {
        Ok(())
    }
}

fn body_too_large(actual: u64, max: u64) -> UrlFetchError {
    Box::new(io::Error::other(format!(
        "Response body too large: {actual} bytes exceeds maxBytes ({max})."
    )))
}

async fn extract_main_content(html: String, url: Url) -> Result<String, UrlFetchError> {
    tokio::task::spawn_blocking(move || extract_main_content_blocking(&html, &url))
        .await
        .map_err(|error| Box::new(error) as UrlFetchError)?
}

fn extract_main_content_blocking(html: &str, url: &Url) -> Result<String, UrlFetchError> {
    let mut input = Cursor::new(html.as_bytes());
    if let Ok(article) = readability::extractor::extract(&mut input, url) {
        let text = article.text.trim();
        if !text.is_empty() {
            let title = article.title.trim();
            return Ok(if title.is_empty() {
                text.to_owned()
            } else {
                format!("# {title}\n\n{text}")
            });
        }
    }
    let document = Html::parse_document(html);
    let title = select_text(&document, "title").unwrap_or_default();
    let text = ["article", "main", "body"]
        .iter()
        .find_map(|selector| select_text(&document, selector))
        .unwrap_or_default();
    if text.is_empty() {
        return Err(Box::new(io::Error::other(
            "Failed to extract meaningful content from the page. The page may require JavaScript to render.",
        )));
    }
    Ok(if title.is_empty() {
        text
    } else {
        format!("# {title}\n\n{text}")
    })
}

fn select_text(document: &Html, selector: &str) -> Option<String> {
    let selector = Selector::parse(selector).ok()?;
    document
        .select(&selector)
        .next()
        .map(|element| element.text().collect::<String>().trim().to_owned())
        .filter(|text| !text.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_source_private_network_ranges() {
        for address in [
            "0.2.3.4",
            "10.0.0.1",
            "100.64.0.1",
            "127.0.0.1",
            "169.254.1.1",
            "172.16.0.1",
            "192.168.1.1",
            "::",
            "::1",
            "fc00::1",
            "fe80::1",
            "::ffff:127.0.0.1",
        ] {
            assert!(is_blocked_address(address.parse().unwrap()), "{address}");
        }
        assert!(!is_blocked_address("8.8.8.8".parse().unwrap()));
        assert!(!is_blocked_address("2001:4860:4860::8888".parse().unwrap()));
    }

    #[tokio::test]
    async fn rejects_private_hosts_and_unsupported_schemes_before_requesting() {
        let private = resolve_safe_fetch_target("http://localhost/", false)
            .await
            .unwrap_err();
        assert!(private.to_string().contains("private host"));
        let scheme = resolve_safe_fetch_target("file:///tmp/x", false)
            .await
            .unwrap_err();
        assert!(scheme.to_string().contains("only http(s) allowed"));
    }

    #[test]
    fn extracts_readable_fallback_content() {
        let output = extract_main_content_blocking(
            "<html><head><title>Page</title></head><body><main>Hello <b>world</b></main></body></html>",
            &Url::parse("https://example.com/").unwrap(),
        )
        .unwrap();
        assert!(output.contains("Hello world"));
    }
}
