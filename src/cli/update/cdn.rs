use std::{error::Error, fmt, time::Duration};

use async_trait::async_trait;
use semver::Version;

use super::{
    cache::parse_manifest,
    types::{FetchLatestResult, UpdateManifest},
};

pub const KIMI_CODE_CDN_BASE: &str = "https://code.kimi.com/kimi-code";
pub const KIMI_CODE_CDN_LATEST_URL: &str = "https://code.kimi.com/kimi-code/latest";
pub const KIMI_CODE_CDN_LATEST_JSON_URL: &str = "https://code.kimi.com/kimi-code/latest.json";
pub const CDN_FETCH_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CdnResponse {
    pub status: u16,
    pub body: String,
}

impl CdnResponse {
    fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }
}

#[async_trait]
pub trait CdnFetch: Send + Sync {
    type Error: Error + Send + Sync + 'static;

    async fn fetch(&self, url: &str) -> Result<CdnResponse, Self::Error>;
}

#[derive(Debug)]
pub enum CdnError<E> {
    Fetch(E),
    Timeout { url: String },
    Http { endpoint: &'static str, status: u16 },
    InvalidSemver(String),
    InvalidManifest,
}

impl<E: fmt::Display> fmt::Display for CdnError<E> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Fetch(error) => error.fmt(formatter),
            Self::Timeout { url } => write!(formatter, "CDN request timed out: {url}"),
            Self::Http { endpoint, status } => {
                write!(formatter, "CDN {endpoint} returned HTTP {status}")
            }
            Self::InvalidSemver(raw) => {
                write!(formatter, "CDN /latest returned invalid semver: {raw:?}")
            }
            Self::InvalidManifest => formatter.write_str("CDN /latest.json was invalid"),
        }
    }
}

impl<E> Error for CdnError<E>
where
    E: Error + 'static,
{
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Fetch(error) => Some(error),
            Self::Timeout { .. }
            | Self::Http { .. }
            | Self::InvalidSemver(_)
            | Self::InvalidManifest => None,
        }
    }
}

// Original:
//   apps/kimi-code/src/cli/update/cdn.ts
//   fetchLatestVersionFromCdn()
pub async fn fetch_latest_version_from_cdn<F>(fetcher: &F) -> Result<String, CdnError<F::Error>>
where
    F: CdnFetch,
{
    let response = fetch_with_timeout(fetcher, KIMI_CODE_CDN_LATEST_URL).await?;
    if !response.is_success() {
        return Err(CdnError::Http {
            endpoint: "/latest",
            status: response.status,
        });
    }
    let raw = response.body.trim();
    if Version::parse(raw).is_err() {
        return Err(CdnError::InvalidSemver(raw.to_owned()));
    }
    Ok(raw.to_owned())
}

// Original:
//   apps/kimi-code/src/cli/update/cdn.ts
//   fetchLatestFromCdn()
pub async fn fetch_latest_from_cdn<F>(fetcher: &F) -> Result<FetchLatestResult, CdnError<F::Error>>
where
    F: CdnFetch,
{
    if let Ok(manifest) = fetch_update_manifest_from_cdn(fetcher).await {
        return Ok(FetchLatestResult {
            latest: manifest.version.clone(),
            manifest: Some(manifest),
        });
    }
    let latest = fetch_latest_version_from_cdn(fetcher).await?;
    Ok(FetchLatestResult {
        latest,
        manifest: None,
    })
}

async fn fetch_update_manifest_from_cdn<F>(
    fetcher: &F,
) -> Result<UpdateManifest, CdnError<F::Error>>
where
    F: CdnFetch,
{
    let response = fetch_with_timeout(fetcher, KIMI_CODE_CDN_LATEST_JSON_URL).await?;
    if !response.is_success() {
        return Err(CdnError::Http {
            endpoint: "/latest.json",
            status: response.status,
        });
    }
    let value = serde_json::from_str(&response.body).map_err(|_| CdnError::InvalidManifest)?;
    parse_manifest(value).ok_or(CdnError::InvalidManifest)
}

async fn fetch_with_timeout<F>(fetcher: &F, url: &str) -> Result<CdnResponse, CdnError<F::Error>>
where
    F: CdnFetch,
{
    match tokio::time::timeout(CDN_FETCH_TIMEOUT, fetcher.fetch(url)).await {
        Ok(result) => result.map_err(CdnError::Fetch),
        Err(_) => Err(CdnError::Timeout {
            url: url.to_owned(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::HashMap, sync::Mutex};

    use super::*;

    #[derive(Debug, Clone, PartialEq, Eq)]
    struct FetchError(&'static str);

    impl fmt::Display for FetchError {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str(self.0)
        }
    }

    impl Error for FetchError {}

    #[derive(Clone)]
    enum Route {
        Response(CdnResponse),
        Error(FetchError),
        Hang,
    }

    struct FetchMock {
        routes: HashMap<String, Route>,
        calls: Mutex<Vec<String>>,
    }

    #[async_trait]
    impl CdnFetch for FetchMock {
        type Error = FetchError;

        async fn fetch(&self, url: &str) -> Result<CdnResponse, Self::Error> {
            self.calls.lock().expect("calls").push(url.to_owned());
            match self
                .routes
                .get(url)
                .cloned()
                .unwrap_or(Route::Response(CdnResponse {
                    status: 404,
                    body: String::new(),
                })) {
                Route::Response(response) => Ok(response),
                Route::Error(error) => Err(error),
                Route::Hang => std::future::pending().await,
            }
        }
    }

    fn response(status: u16, body: impl Into<String>) -> Route {
        Route::Response(CdnResponse {
            status,
            body: body.into(),
        })
    }

    fn fetcher(routes: impl IntoIterator<Item = (&'static str, Route)>) -> FetchMock {
        FetchMock {
            routes: routes
                .into_iter()
                .map(|(url, route)| (url.to_owned(), route))
                .collect(),
            calls: Mutex::new(Vec::new()),
        }
    }

    fn manifest_body() -> String {
        serde_json::json!({
            "schemaVersion": 1,
            "version": "2.0.0",
            "publishedAt": "2026-06-12T00:00:00.000Z",
            "rollout": [
                { "percent": 30, "delaySeconds": 0 },
                { "percent": 30, "delaySeconds": 43_200 },
                { "percent": 40, "delaySeconds": 86_400 }
            ]
        })
        .to_string()
    }

    #[tokio::test]
    async fn plain_latest_trims_and_validates_semver() {
        let ok = fetcher([(KIMI_CODE_CDN_LATEST_URL, response(200, "  0.5.0\n"))]);
        assert_eq!(
            fetch_latest_version_from_cdn(&ok).await.expect("latest"),
            "0.5.0"
        );

        let http = fetcher([(KIMI_CODE_CDN_LATEST_URL, response(404, ""))]);
        assert!(
            fetch_latest_version_from_cdn(&http)
                .await
                .expect_err("http error")
                .to_string()
                .contains("HTTP 404")
        );
        let invalid = fetcher([(KIMI_CODE_CDN_LATEST_URL, response(200, "not-a-version"))]);
        assert!(
            fetch_latest_version_from_cdn(&invalid)
                .await
                .expect_err("semver error")
                .to_string()
                .contains("invalid semver")
        );
    }

    #[tokio::test]
    async fn parses_lenient_manifest_without_falling_back() {
        let fetch = fetcher([(
            KIMI_CODE_CDN_LATEST_JSON_URL,
            response(200, manifest_body()),
        )]);
        let result = fetch_latest_from_cdn(&fetch).await.expect("manifest");
        let manifest = result.manifest.expect("manifest present");
        assert_eq!(result.latest, "2.0.0");
        assert_eq!(manifest.rollout.len(), 3);
        assert_eq!(fetch.calls.lock().expect("calls").len(), 1);
    }

    #[tokio::test]
    async fn defaults_missing_rollout_and_ignores_future_fields() {
        let body = serde_json::json!({
            "version": "2.0.0",
            "publishedAt": "2026-06-12T00:00:00.000Z",
            "futureField": { "nested": true }
        })
        .to_string();
        let fetch = fetcher([(KIMI_CODE_CDN_LATEST_JSON_URL, response(200, body))]);
        let result = fetch_latest_from_cdn(&fetch).await.expect("manifest");
        assert!(result.manifest.expect("manifest").rollout.is_empty());
    }

    #[tokio::test]
    async fn malformed_manifest_falls_back_to_plain_latest() {
        for malformed in [
            "not json".to_owned(),
            serde_json::json!({
                "version": "nope", "publishedAt": "2026-06-12T00:00:00.000Z"
            })
            .to_string(),
            serde_json::json!({
                "version": "2.0.0", "publishedAt": "garbage"
            })
            .to_string(),
            serde_json::json!({
                "version": "2.0.0",
                "publishedAt": "2026-06-12T00:00:00.000Z",
                "rollout": [{ "percent": 150, "delaySeconds": 0 }]
            })
            .to_string(),
            serde_json::json!({
                "version": "2.0.0",
                "publishedAt": "2026-06-12T00:00:00.000Z",
                "rollout": [{ "percent": 100, "delaySeconds": -1 }]
            })
            .to_string(),
        ] {
            let fetch = fetcher([
                (KIMI_CODE_CDN_LATEST_JSON_URL, response(200, malformed)),
                (KIMI_CODE_CDN_LATEST_URL, response(200, "1.9.0\n")),
            ]);
            let result = fetch_latest_from_cdn(&fetch).await.expect("fallback");
            assert_eq!(result.latest, "1.9.0");
            assert_eq!(result.manifest, None);
        }
    }

    #[tokio::test]
    async fn manifest_fetch_error_falls_back_but_plain_error_propagates() {
        let fetch = fetcher([
            (
                KIMI_CODE_CDN_LATEST_JSON_URL,
                Route::Error(FetchError("json down")),
            ),
            (
                KIMI_CODE_CDN_LATEST_URL,
                Route::Error(FetchError("plain down")),
            ),
        ]);
        assert_eq!(
            fetch_latest_from_cdn(&fetch)
                .await
                .expect_err("both fail")
                .to_string(),
            "plain down"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn hanging_manifest_times_out_then_uses_plain_latest() {
        let fetch = fetcher([
            (KIMI_CODE_CDN_LATEST_JSON_URL, Route::Hang),
            (KIMI_CODE_CDN_LATEST_URL, response(200, "1.9.0\n")),
        ]);
        let result = fetch_latest_from_cdn(&fetch)
            .await
            .expect("timeout fallback");
        assert_eq!(result.latest, "1.9.0");
        assert_eq!(result.manifest, None);
    }
}
