//! Persistent installed-plugin registry.
//!
//! Original: `packages/agent-core-v2/src/app/plugin/store.ts`.

use std::{
    io::ErrorKind,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

use super::types::{PluginCapabilityState, PluginGithubMetadata, PluginSource};

const INSTALLED_REL: &str = "plugins/installed.json";

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstalledRecord {
    pub id: String,
    pub root: String,
    pub source: PluginSource,
    pub enabled: bool,
    pub installed_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original_source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<PluginCapabilityState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub github: Option<PluginGithubMetadata>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct InstalledFile {
    pub version: u32,
    pub plugins: Vec<InstalledRecord>,
}

impl Default for InstalledFile {
    fn default() -> Self {
        Self {
            version: 1,
            plugins: Vec::new(),
        }
    }
}

#[derive(Debug, Error)]
pub enum InstalledStoreError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("Failed to parse {path}: {message}")]
    Parse {
        path: String,
        message: String,
        #[source]
        cause: Box<dyn std::error::Error + Send + Sync>,
    },
    #[error(transparent)]
    Encode(#[from] serde_json::Error),
}

// Original: store.ts, readInstalled().
pub async fn read_installed(
    kimi_home_dir: impl AsRef<Path>,
) -> Result<InstalledFile, InstalledStoreError> {
    let file_path = kimi_home_dir.as_ref().join(INSTALLED_REL);
    let text = match tokio::fs::read_to_string(&file_path).await {
        Ok(text) => text,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(InstalledFile::default()),
        Err(error) => return Err(error.into()),
    };

    parse_installed(&file_path, &text)
}

fn parse_installed(file_path: &Path, text: &str) -> Result<InstalledFile, InstalledStoreError> {
    let value: Value = serde_json::from_str(text).map_err(|error| parse_error(file_path, error))?;
    if !value.is_object() || !value.get("plugins").is_some_and(Value::is_array) {
        return Err(parse_error(file_path, InstalledFileShapeError));
    }
    serde_json::from_value(value).map_err(|error| parse_error(file_path, error))
}

// Original: store.ts, writeInstalled(). The fixed `.tmp` name and write-then-
// rename order are intentionally retained.
pub async fn write_installed(
    kimi_home_dir: impl AsRef<Path>,
    data: &InstalledFile,
) -> Result<(), InstalledStoreError> {
    let directory = kimi_home_dir.as_ref().join("plugins");
    tokio::fs::create_dir_all(&directory).await?;
    let final_path = directory.join("installed.json");
    let temporary_path = PathBuf::from(format!("{}.tmp", final_path.to_string_lossy()));
    let text = serde_json::to_string_pretty(data)?;
    tokio::fs::write(&temporary_path, text).await?;
    tokio::fs::rename(temporary_path, final_path).await?;
    Ok(())
}

fn parse_error(
    path: &Path,
    cause: impl std::error::Error + Send + Sync + 'static,
) -> InstalledStoreError {
    InstalledStoreError::Parse {
        path: path.to_string_lossy().into_owned(),
        message: cause.to_string(),
        cause: Box::new(cause),
    }
}

#[derive(Debug, Error)]
#[error("installed.json is not a valid InstalledFile object")]
struct InstalledFileShapeError;

#[cfg(test)]
mod tests {
    use super::*;

    fn temporary_home() -> PathBuf {
        std::env::temp_dir().join(format!("plugin-store-{}", uuid::Uuid::new_v4()))
    }

    #[tokio::test]
    async fn missing_file_returns_the_original_empty_document() {
        let home = temporary_home();
        assert_eq!(
            read_installed(&home).await.unwrap(),
            InstalledFile::default()
        );
    }

    #[tokio::test]
    async fn writes_pretty_json_without_a_trailing_newline_and_reads_it_back() {
        let home = temporary_home();
        let data = InstalledFile {
            version: 1,
            plugins: vec![InstalledRecord {
                id: "demo".to_owned(),
                root: "/plugins/demo".to_owned(),
                source: PluginSource::LocalPath,
                enabled: true,
                installed_at: "2026-01-02T03:04:05.000Z".to_owned(),
                updated_at: None,
                original_source: Some("/source/demo".to_owned()),
                capabilities: None,
                github: None,
            }],
        };

        write_installed(&home, &data).await.unwrap();
        let path = home.join(INSTALLED_REL);
        let text = tokio::fs::read_to_string(&path).await.unwrap();
        assert!(!text.ends_with('\n'));
        assert!(text.contains("\n  \"plugins\": ["));
        assert!(!path.with_file_name("installed.json.tmp").exists());
        assert_eq!(read_installed(&home).await.unwrap(), data);

        tokio::fs::remove_dir_all(home).await.unwrap();
    }

    #[tokio::test]
    async fn wraps_json_and_top_level_shape_errors_with_the_file_path() {
        let home = temporary_home();
        let directory = home.join("plugins");
        tokio::fs::create_dir_all(&directory).await.unwrap();
        let path = directory.join("installed.json");

        tokio::fs::write(&path, "[]").await.unwrap();
        let error = read_installed(&home).await.unwrap_err().to_string();
        assert!(error.contains(path.to_string_lossy().as_ref()));
        assert!(error.ends_with("installed.json is not a valid InstalledFile object"));

        tokio::fs::write(&path, "{").await.unwrap();
        let error = read_installed(&home).await.unwrap_err().to_string();
        assert!(error.starts_with(&format!("Failed to parse {}: ", path.to_string_lossy())));

        tokio::fs::remove_dir_all(home).await.unwrap();
    }
}
