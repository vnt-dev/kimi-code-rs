use std::error::Error;

use async_trait::async_trait;
use chrono::{DateTime, SecondsFormat, Utc};

use super::types::{FetchLatestResult, UpdateCache, UpdateCacheSource};

#[async_trait]
pub trait RefreshUpdateCacheDeps: Send + Sync {
    type Error: Error + Send + Sync + 'static;

    async fn fetch_latest(&self) -> Result<FetchLatestResult, Self::Error>;

    async fn write_cache(&self, cache: &UpdateCache) -> Result<(), Self::Error>;

    fn now(&self) -> DateTime<Utc>;
}

// Original:
//   apps/kimi-code/src/cli/update/refresh.ts
//   refreshUpdateCache()
pub async fn refresh_update_cache<D>(deps: &D) -> Result<UpdateCache, D::Error>
where
    D: RefreshUpdateCacheDeps,
{
    let fetched = deps.fetch_latest().await?;
    let cache = UpdateCache {
        source: UpdateCacheSource::Cdn,
        checked_at: Some(deps.now().to_rfc3339_opts(SecondsFormat::Millis, true)),
        latest: Some(fetched.latest),
        manifest: fetched.manifest,
    };
    deps.write_cache(&cache).await?;
    Ok(cache)
}

#[cfg(test)]
mod tests {
    use std::{fmt, sync::Mutex};

    use chrono::TimeZone;

    use super::*;
    use crate::cli::update::types::{RolloutBatch, UpdateManifest};

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    struct RefreshError(&'static str);

    impl fmt::Display for RefreshError {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str(self.0)
        }
    }

    impl Error for RefreshError {}

    struct DepsMock {
        fetched: Result<FetchLatestResult, RefreshError>,
        write_error: Option<RefreshError>,
        writes: Mutex<Vec<UpdateCache>>,
    }

    #[async_trait]
    impl RefreshUpdateCacheDeps for DepsMock {
        type Error = RefreshError;

        async fn fetch_latest(&self) -> Result<FetchLatestResult, Self::Error> {
            self.fetched.clone()
        }

        async fn write_cache(&self, cache: &UpdateCache) -> Result<(), Self::Error> {
            self.writes.lock().expect("writes").push(cache.clone());
            self.write_error.map_or(Ok(()), Err)
        }

        fn now(&self) -> DateTime<Utc> {
            Utc.with_ymd_and_hms(2026, 7, 21, 3, 4, 5)
                .single()
                .expect("valid time")
                + chrono::TimeDelta::milliseconds(678)
        }
    }

    fn manifest() -> UpdateManifest {
        UpdateManifest {
            version: "0.5.0".to_owned(),
            published_at: "2026-07-21T00:00:00.000Z".to_owned(),
            rollout: vec![RolloutBatch {
                percent: 100,
                delay_seconds: 0,
            }],
        }
    }

    #[tokio::test]
    async fn fetches_builds_writes_and_returns_the_same_cache() {
        let deps = DepsMock {
            fetched: Ok(FetchLatestResult {
                latest: "0.5.0".to_owned(),
                manifest: Some(manifest()),
            }),
            write_error: None,
            writes: Mutex::new(Vec::new()),
        };

        let cache = refresh_update_cache(&deps).await.expect("refresh cache");

        assert_eq!(cache.latest.as_deref(), Some("0.5.0"));
        assert_eq!(
            cache.checked_at.as_deref(),
            Some("2026-07-21T03:04:05.678Z")
        );
        assert_eq!(cache.manifest, Some(manifest()));
        assert_eq!(deps.writes.lock().expect("writes").as_slice(), [cache]);
    }

    #[tokio::test]
    async fn fetch_failure_does_not_write_a_replacement_cache() {
        let deps = DepsMock {
            fetched: Err(RefreshError("cdn unavailable")),
            write_error: None,
            writes: Mutex::new(Vec::new()),
        };

        let error = refresh_update_cache(&deps)
            .await
            .expect_err("fetch failure");

        assert_eq!(error, RefreshError("cdn unavailable"));
        assert!(deps.writes.lock().expect("writes").is_empty());
    }

    #[tokio::test]
    async fn write_failure_is_propagated_after_the_attempted_write() {
        let deps = DepsMock {
            fetched: Ok(FetchLatestResult {
                latest: "0.5.0".to_owned(),
                manifest: None,
            }),
            write_error: Some(RefreshError("disk full")),
            writes: Mutex::new(Vec::new()),
        };

        let error = refresh_update_cache(&deps)
            .await
            .expect_err("write failure");

        assert_eq!(error, RefreshError("disk full"));
        assert_eq!(deps.writes.lock().expect("writes").len(), 1);
    }
}
