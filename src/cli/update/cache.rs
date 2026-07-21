use std::{
    collections::HashSet,
    error::Error,
    fmt,
    io::{self, Write},
    path::Path,
};

use atomic_write_file::AtomicWriteFile;
use chrono::{DateTime, NaiveDate};
use semver::Version;
use serde_json::{Map, Value};

use super::types::{
    RolloutBatch, UpdateCache, UpdateCacheSource, UpdateManifest, empty_update_cache,
};
use crate::utils::paths::{HomeDirectoryUnavailable, get_update_state_file};

#[derive(Debug)]
pub enum UpdateCacheWriteError {
    Invalid(String),
    Path(HomeDirectoryUnavailable),
    Io(io::Error),
    Join(tokio::task::JoinError),
}

impl fmt::Display for UpdateCacheWriteError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(message) => write!(formatter, "invalid update cache: {message}"),
            Self::Path(error) => error.fmt(formatter),
            Self::Io(error) => write!(formatter, "failed to write update cache: {error}"),
            Self::Join(error) => write!(formatter, "update cache write task failed: {error}"),
        }
    }
}

impl Error for UpdateCacheWriteError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Invalid(_) => None,
            Self::Path(error) => Some(error),
            Self::Io(error) => Some(error),
            Self::Join(error) => Some(error),
        }
    }
}

// Original:
//   apps/kimi-code/src/cli/update/cache.ts
//   readUpdateCache()
pub async fn read_update_cache() -> UpdateCache {
    let Ok(file_path) = get_update_state_file() else {
        return empty_update_cache();
    };
    read_update_cache_from(&file_path).await
}

pub async fn read_update_cache_from(file_path: &Path) -> UpdateCache {
    let Ok(raw) = tokio::fs::read_to_string(file_path).await else {
        return empty_update_cache();
    };
    let Ok(value) = serde_json::from_str::<Value>(&raw) else {
        return empty_update_cache();
    };
    parse_update_cache(value).unwrap_or_else(empty_update_cache)
}

// Original:
//   apps/kimi-code/src/cli/update/cache.ts
//   writeUpdateCache()
//
// Rust adaptation:
//   AtomicWriteFile performs the temporary-file replacement on a blocking
//   thread, including Windows replacement semantics, so async runtime workers
//   never execute blocking filesystem calls.
pub async fn write_update_cache(value: &UpdateCache) -> Result<(), UpdateCacheWriteError> {
    let file_path = get_update_state_file().map_err(UpdateCacheWriteError::Path)?;
    write_update_cache_to(value, &file_path).await
}

pub async fn write_update_cache_to(
    value: &UpdateCache,
    file_path: &Path,
) -> Result<(), UpdateCacheWriteError> {
    let serialized_value = serde_json::to_value(value)
        .map_err(|error| UpdateCacheWriteError::Invalid(error.to_string()))?;
    parse_update_cache(serialized_value)
        .ok_or_else(|| UpdateCacheWriteError::Invalid("schema validation failed".to_owned()))?;
    let content = serde_json::to_string_pretty(value)
        .map_err(|error| UpdateCacheWriteError::Invalid(error.to_string()))?
        + "\n";
    let file_path = file_path.to_path_buf();
    tokio::task::spawn_blocking(move || write_atomic(&file_path, content.as_bytes()))
        .await
        .map_err(UpdateCacheWriteError::Join)?
        .map_err(UpdateCacheWriteError::Io)
}

fn write_atomic(file_path: &Path, content: &[u8]) -> io::Result<()> {
    if file_path
        .file_name()
        .is_some_and(|name| name == "config.toml")
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "CLI persistence helpers must not write config.toml; use core/SDK config APIs.",
        ));
    }
    let parent = file_path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)?;
    let mut file = AtomicWriteFile::open(file_path)?;
    file.write_all(content)?;
    file.commit()
}

fn parse_update_cache(value: Value) -> Option<UpdateCache> {
    let object = value.as_object()?;
    if !has_only_cache_keys(object) || object.get("source")?.as_str()? != "cdn" {
        return None;
    }
    let checked_at = nullable_nonempty_string(object.get("checkedAt")?)?;
    let latest = nullable_nonempty_string(object.get("latest")?)?;
    let manifest = object.get("manifest").cloned().unwrap_or(Value::Null);
    Some(UpdateCache {
        source: UpdateCacheSource::Cdn,
        checked_at,
        latest,
        manifest: parse_manifest(manifest),
    })
}

fn has_only_cache_keys(object: &Map<String, Value>) -> bool {
    let allowed = HashSet::from(["source", "checkedAt", "latest", "manifest"]);
    object.keys().all(|key| allowed.contains(key.as_str()))
}

fn nullable_nonempty_string(value: &Value) -> Option<Option<String>> {
    if value.is_null() {
        return Some(None);
    }
    let value = value.as_str()?;
    (!value.is_empty()).then(|| Some(value.to_owned()))
}

fn parse_manifest(value: Value) -> Option<UpdateManifest> {
    if value.is_null() {
        return None;
    }
    let object = value.as_object()?;
    let version = object.get("version")?.as_str()?;
    Version::parse(version).ok()?;
    let published_at = object.get("publishedAt")?.as_str()?;
    if !is_javascript_date(published_at) {
        return None;
    }
    let rollout = match object.get("rollout") {
        None => Vec::new(),
        Some(Value::Array(entries)) => entries
            .iter()
            .map(parse_rollout_batch)
            .collect::<Option<Vec<_>>>()?,
        Some(_) => return None,
    };
    Some(UpdateManifest {
        version: version.to_owned(),
        published_at: published_at.to_owned(),
        rollout,
    })
}

fn parse_rollout_batch(value: &Value) -> Option<RolloutBatch> {
    let object = value.as_object()?;
    let percent = object.get("percent")?.as_u64()?;
    let delay_seconds = object.get("delaySeconds")?.as_u64()?;
    Some(RolloutBatch {
        percent: u8::try_from(percent)
            .ok()
            .filter(|percent| *percent <= 100)?,
        delay_seconds,
    })
}

fn is_javascript_date(value: &str) -> bool {
    DateTime::parse_from_rfc3339(value).is_ok()
        || NaiveDate::parse_from_str(value, "%Y-%m-%d").is_ok()
}

#[cfg(test)]
mod tests {
    use std::{
        path::PathBuf,
        sync::atomic::{AtomicU64, Ordering},
    };

    use super::*;

    static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

    fn temp_file() -> PathBuf {
        let id = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir()
            .join(format!("kimi-update-cache-{}-{id}", std::process::id()))
            .join("updates")
            .join("latest.json")
    }

    fn cache_with_manifest() -> UpdateCache {
        UpdateCache {
            source: UpdateCacheSource::Cdn,
            checked_at: Some("2026-04-23T08:00:00.000Z".to_owned()),
            latest: Some("0.5.0".to_owned()),
            manifest: Some(UpdateManifest {
                version: "0.5.0".to_owned(),
                published_at: "2026-04-23T07:00:00.000Z".to_owned(),
                rollout: vec![RolloutBatch {
                    percent: 100,
                    delay_seconds: 0,
                }],
            }),
        }
    }

    async fn cleanup(file: &Path) {
        if let Some(root) = file.parent().and_then(Path::parent) {
            let _ = tokio::fs::remove_dir_all(root).await;
        }
    }

    #[tokio::test]
    async fn missing_corrupt_and_old_shape_files_return_empty_cache() {
        let file = temp_file();
        assert_eq!(read_update_cache_from(&file).await, empty_update_cache());
        tokio::fs::create_dir_all(file.parent().expect("parent"))
            .await
            .expect("create parent");
        tokio::fs::write(&file, "{\"broken\"")
            .await
            .expect("write corrupt");
        assert_eq!(read_update_cache_from(&file).await, empty_update_cache());
        tokio::fs::write(
            &file,
            r#"{"packageName":"@moonshot-ai/kimi-code","checkedAt":"2026-04-23T08:00:00.000Z","distTags":{}}"#,
        )
        .await
        .expect("write old cache");
        assert_eq!(read_update_cache_from(&file).await, empty_update_cache());
        cleanup(&file).await;
    }

    #[tokio::test]
    async fn atomically_writes_overwrites_and_reads_a_manifest_cache() {
        let file = temp_file();
        let cache = cache_with_manifest();
        write_update_cache_to(&empty_update_cache(), &file)
            .await
            .expect("initial cache");
        write_update_cache_to(&cache, &file)
            .await
            .expect("overwrite cache");

        assert_eq!(read_update_cache_from(&file).await, cache);
        let raw = tokio::fs::read_to_string(&file).await.expect("cache text");
        assert!(raw.ends_with('\n'));
        assert!(raw.contains("\n  \"source\": \"cdn\""));
        cleanup(&file).await;
    }

    #[tokio::test]
    async fn legacy_missing_manifest_defaults_to_null() {
        let file = temp_file();
        tokio::fs::create_dir_all(file.parent().expect("parent"))
            .await
            .expect("create parent");
        tokio::fs::write(
            &file,
            r#"{"source":"cdn","checkedAt":"2026-04-23T08:00:00.000Z","latest":"0.5.0"}"#,
        )
        .await
        .expect("write legacy cache");

        let cache = read_update_cache_from(&file).await;
        assert_eq!(cache.latest.as_deref(), Some("0.5.0"));
        assert_eq!(cache.manifest, None);
        cleanup(&file).await;
    }

    #[tokio::test]
    async fn malformed_manifest_becomes_null_without_discarding_latest() {
        let file = temp_file();
        tokio::fs::create_dir_all(file.parent().expect("parent"))
            .await
            .expect("create parent");
        tokio::fs::write(
            &file,
            r#"{"source":"cdn","checkedAt":"2026-04-23T08:00:00.000Z","latest":"0.5.0","manifest":{"version":"bad","publishedAt":"nope","rollout":"bad"}}"#,
        )
        .await
        .expect("write malformed manifest");

        let cache = read_update_cache_from(&file).await;
        assert_eq!(cache.latest.as_deref(), Some("0.5.0"));
        assert_eq!(cache.manifest, None);
        cleanup(&file).await;
    }

    #[tokio::test]
    async fn rejects_invalid_typed_values_before_touching_the_file() {
        let file = temp_file();
        let invalid = UpdateCache {
            checked_at: Some(String::new()),
            ..empty_update_cache()
        };
        let error = write_update_cache_to(&invalid, &file)
            .await
            .expect_err("invalid cache");
        assert!(error.to_string().contains("schema validation failed"));
        assert!(!file.exists());
    }
}
