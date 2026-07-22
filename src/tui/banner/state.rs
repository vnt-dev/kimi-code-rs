use std::{
    collections::{HashMap, HashSet},
    error::Error,
    fmt,
    path::Path,
};

use chrono::DateTime;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::utils::{
    paths::{HomeDirectoryUnavailable, get_banner_state_file},
    persistence::{PersistenceError, read_json_file, write_json_file},
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BannerDisplayRecord {
    pub last_shown_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BannerDisplayState {
    pub version: u8,
    pub shown: HashMap<String, BannerDisplayRecord>,
}

pub fn empty_banner_display_state() -> BannerDisplayState {
    BannerDisplayState {
        version: 1,
        shown: HashMap::new(),
    }
}

#[derive(Debug)]
pub enum BannerStateWriteError {
    Path(HomeDirectoryUnavailable),
    Persistence(PersistenceError),
}

impl fmt::Display for BannerStateWriteError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Path(error) => error.fmt(formatter),
            Self::Persistence(error) => write!(formatter, "failed to write banner state: {error}"),
        }
    }
}

impl Error for BannerStateWriteError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Path(error) => Some(error),
            Self::Persistence(error) => Some(error),
        }
    }
}

// Original:
//   apps/kimi-code/src/tui/banner/state.ts
//   readBannerDisplayState()
pub async fn read_banner_display_state() -> BannerDisplayState {
    let Ok(file_path) = get_banner_state_file() else {
        return empty_banner_display_state();
    };
    read_banner_display_state_from(&file_path).await
}

pub async fn read_banner_display_state_from(file_path: &Path) -> BannerDisplayState {
    read_json_file(
        file_path,
        |value| parse_banner_display_state(value).ok_or_else(|| "invalid banner state".to_owned()),
        empty_banner_display_state(),
    )
    .await
    .unwrap_or_else(|_| empty_banner_display_state())
}

// Original:
//   apps/kimi-code/src/tui/banner/state.ts
//   writeBannerDisplayState()
pub async fn write_banner_display_state(
    value: &BannerDisplayState,
) -> Result<(), BannerStateWriteError> {
    let file_path = get_banner_state_file().map_err(BannerStateWriteError::Path)?;
    write_banner_display_state_to(value, &file_path).await
}

pub async fn write_banner_display_state_to(
    value: &BannerDisplayState,
    file_path: &Path,
) -> Result<(), BannerStateWriteError> {
    write_json_file(
        file_path,
        |serialized| {
            let parsed = parse_banner_display_state(serialized)
                .ok_or_else(|| "invalid banner state: schema validation failed".to_owned())?;
            serde_json::to_value(parsed).map_err(|error| error.to_string())
        },
        value,
    )
    .await
    .map_err(BannerStateWriteError::Persistence)
}

fn parse_banner_display_state(value: Value) -> Option<BannerDisplayState> {
    let object = value.as_object()?;
    if !has_only_keys(object, &["version", "shown"]) || object.get("version")?.as_u64()? != 1 {
        return None;
    }

    let shown = object
        .get("shown")
        .and_then(Value::as_object)
        .map(parse_shown_records)
        .unwrap_or_default();
    Some(BannerDisplayState { version: 1, shown })
}

fn parse_shown_records(object: &Map<String, Value>) -> HashMap<String, BannerDisplayRecord> {
    object
        .iter()
        .filter_map(|(key, value)| {
            if key.is_empty() {
                return None;
            }
            let record = value.as_object()?;
            if !has_only_keys(record, &["lastShownAt"]) {
                return None;
            }
            let last_shown_at = record.get("lastShownAt")?.as_str()?;
            parse_date(last_shown_at)?;
            Some((
                key.clone(),
                BannerDisplayRecord {
                    last_shown_at: last_shown_at.to_owned(),
                },
            ))
        })
        .collect()
}

fn parse_date(value: &str) -> Option<()> {
    DateTime::parse_from_rfc3339(value).ok().map(|_| ())
}

fn has_only_keys(object: &Map<String, Value>, allowed: &[&str]) -> bool {
    let allowed = allowed.iter().copied().collect::<HashSet<_>>();
    object.keys().all(|key| allowed.contains(key.as_str()))
}

#[cfg(test)]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

    fn temporary_file(name: &str) -> std::path::PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        std::env::temp_dir()
            .join(format!("kimi-banner-state-{nonce}"))
            .join(name)
    }

    #[tokio::test]
    async fn missing_and_corrupt_files_fall_back_to_empty_state() {
        let path = temporary_file("state.json");
        assert_eq!(
            read_banner_display_state_from(&path).await,
            empty_banner_display_state()
        );
        tokio::fs::create_dir_all(path.parent().expect("parent"))
            .await
            .expect("directory");
        tokio::fs::write(&path, b"{\"broken\"")
            .await
            .expect("corrupt file");
        assert_eq!(
            read_banner_display_state_from(&path).await,
            empty_banner_display_state()
        );
        let _ = tokio::fs::remove_dir_all(path.parent().expect("parent")).await;
    }

    #[tokio::test]
    async fn future_version_falls_back_and_invalid_records_are_dropped() {
        let path = temporary_file("state.json");
        tokio::fs::create_dir_all(path.parent().expect("parent"))
            .await
            .expect("directory");
        tokio::fs::write(&path, r#"{"version":2,"shown":{}}"#)
            .await
            .expect("future state");
        assert_eq!(
            read_banner_display_state_from(&path).await,
            empty_banner_display_state()
        );
        tokio::fs::write(
            &path,
            r#"{"version":1,"shown":{"valid":{"lastShownAt":"2026-06-16T00:00:00.000Z"},"invalid":{"lastShownAt":"not-a-date"},"malformed":{"shownAt":"2026-06-16T00:00:00.000Z"}}}"#,
        )
        .await
        .expect("mixed state");
        let state = read_banner_display_state_from(&path).await;
        assert_eq!(state.shown.len(), 1);
        assert_eq!(
            state.shown["valid"].last_shown_at,
            "2026-06-16T00:00:00.000Z"
        );
        let _ = tokio::fs::remove_dir_all(path.parent().expect("parent")).await;
    }

    #[tokio::test]
    async fn writes_and_reads_state_atomically() {
        let path = temporary_file("state.json");
        let state = BannerDisplayState {
            version: 1,
            shown: HashMap::from([(
                "active".to_owned(),
                BannerDisplayRecord {
                    last_shown_at: "2026-06-16T00:00:00.000Z".to_owned(),
                },
            )]),
        };
        write_banner_display_state_to(&state, &path)
            .await
            .expect("write state");
        assert_eq!(read_banner_display_state_from(&path).await, state);
        let _ = tokio::fs::remove_dir_all(path.parent().expect("parent")).await;
    }
}
