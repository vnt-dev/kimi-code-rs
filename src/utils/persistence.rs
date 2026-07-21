use std::{
    error::Error,
    fmt,
    io::{self, Write},
    path::{Path, PathBuf},
};

use atomic_write_file::AtomicWriteFile;
use serde::Serialize;
use serde_json::Value;
use tokio::io::AsyncWriteExt;

#[derive(Debug)]
pub enum PersistenceError {
    Invalid(String),
    Io(io::Error),
    Join(tokio::task::JoinError),
    Json(serde_json::Error),
}

impl fmt::Display for PersistenceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(message) => formatter.write_str(message),
            Self::Io(error) => error.fmt(formatter),
            Self::Join(error) => write!(formatter, "persistence task failed: {error}"),
            Self::Json(error) => error.fmt(formatter),
        }
    }
}

impl Error for PersistenceError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Invalid(_) => None,
            Self::Io(error) => Some(error),
            Self::Join(error) => Some(error),
            Self::Json(error) => Some(error),
        }
    }
}

impl From<io::Error> for PersistenceError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<serde_json::Error> for PersistenceError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

// Original:
//   apps/kimi-code/src/utils/persistence.ts
//   readJsonFile()
pub async fn read_json_file<T, V>(
    file_path: &Path,
    validate: V,
    fallback: T,
) -> Result<T, PersistenceError>
where
    V: FnOnce(Value) -> Result<T, String>,
{
    let raw = match tokio::fs::read_to_string(file_path).await {
        Ok(raw) => raw,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(fallback),
        Err(error) => return Err(error.into()),
    };
    let parsed = serde_json::from_str(&raw)?;
    validate(parsed).map_err(PersistenceError::Invalid)
}

// Original:
//   apps/kimi-code/src/utils/persistence.ts
//   writeJsonFile()
pub async fn write_json_file<T, V>(
    file_path: &Path,
    validate: V,
    value: &T,
) -> Result<(), PersistenceError>
where
    T: Serialize,
    V: FnOnce(Value) -> Result<Value, String>,
{
    assert_non_config_write(file_path)?;
    let parsed = validate(serde_json::to_value(value)?).map_err(PersistenceError::Invalid)?;
    let content = serde_json::to_string_pretty(&parsed)? + "\n";
    write_atomic(file_path.to_path_buf(), content.into_bytes()).await
}

// Original:
//   apps/kimi-code/src/utils/persistence.ts
//   readJsonlFile()
pub async fn read_jsonl_file<T, V>(
    file_path: &Path,
    validate_line: V,
) -> Result<Vec<T>, PersistenceError>
where
    V: Fn(&Value) -> Option<T>,
{
    let raw = match tokio::fs::read_to_string(file_path).await {
        Ok(raw) => raw,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.into()),
    };
    Ok(raw
        .lines()
        .filter_map(|line| {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                return None;
            }
            let value = serde_json::from_str::<Value>(trimmed).ok()?;
            validate_line(&value)
        })
        .collect())
}

// Original:
//   apps/kimi-code/src/utils/persistence.ts
//   appendJsonlLine()
pub async fn append_jsonl_line<T, V>(
    file_path: &Path,
    validate: V,
    value: &T,
) -> Result<(), PersistenceError>
where
    T: Serialize,
    V: FnOnce(Value) -> Result<Value, String>,
{
    assert_non_config_write(file_path)?;
    let parsed = validate(serde_json::to_value(value)?).map_err(PersistenceError::Invalid)?;
    if let Some(parent) = file_path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let mut file = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(file_path)
        .await?;
    file.write_all((serde_json::to_string(&parsed)? + "\n").as_bytes())
        .await?;
    Ok(())
}

fn assert_non_config_write(file_path: &Path) -> Result<(), PersistenceError> {
    if file_path
        .file_name()
        .is_some_and(|name| name == "config.toml")
    {
        return Err(PersistenceError::Invalid(
            "CLI persistence helpers must not write config.toml; use core/SDK config APIs."
                .to_owned(),
        ));
    }
    Ok(())
}

async fn write_atomic(file_path: PathBuf, content: Vec<u8>) -> Result<(), PersistenceError> {
    tokio::task::spawn_blocking(move || {
        let parent = file_path.parent().unwrap_or_else(|| Path::new("."));
        std::fs::create_dir_all(parent)?;
        let mut file = AtomicWriteFile::open(file_path)?;
        file.write_all(&content)?;
        file.commit()
    })
    .await
    .map_err(PersistenceError::Join)?
    .map_err(PersistenceError::Io)
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use serde::{Deserialize, Serialize};

    use super::*;

    static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    struct Entry {
        id: u64,
    }

    fn temp_file(name: &str) -> PathBuf {
        let id = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir()
            .join(format!("kimi-persistence-{}-{id}", std::process::id()))
            .join(name)
    }

    fn validate_entry(value: Value) -> Result<Entry, String> {
        serde_json::from_value(value).map_err(|error| error.to_string())
    }

    fn validate_entry_value(value: Value) -> Result<Value, String> {
        validate_entry(value.clone()).map(|_| value)
    }

    async fn cleanup(file: &Path) {
        if let Some(parent) = file.parent() {
            let _ = tokio::fs::remove_dir_all(parent).await;
        }
    }

    #[tokio::test]
    async fn missing_json_uses_fallback_and_valid_json_is_checked() {
        let file = temp_file("state.json");
        assert_eq!(
            read_json_file(&file, validate_entry, Entry { id: 7 })
                .await
                .expect("fallback"),
            Entry { id: 7 }
        );
        tokio::fs::create_dir_all(file.parent().expect("parent"))
            .await
            .expect("create parent");
        tokio::fs::write(&file, r#"{"id":11}"#)
            .await
            .expect("write json");
        assert_eq!(
            read_json_file(&file, validate_entry, Entry { id: 7 })
                .await
                .expect("read json"),
            Entry { id: 11 }
        );
        cleanup(&file).await;
    }

    #[tokio::test]
    async fn atomically_overwrites_pretty_json_with_a_final_newline() {
        let file = temp_file("nested/state.json");
        write_json_file(&file, validate_entry_value, &Entry { id: 1 })
            .await
            .expect("first write");
        write_json_file(&file, validate_entry_value, &Entry { id: 2 })
            .await
            .expect("overwrite");
        assert_eq!(
            tokio::fs::read_to_string(&file).await.expect("read file"),
            "{\n  \"id\": 2\n}\n"
        );
        cleanup(&file).await;
    }

    #[tokio::test]
    async fn jsonl_skips_blank_corrupt_and_schema_invalid_rows() {
        let file = temp_file("history/data.jsonl");
        tokio::fs::create_dir_all(file.parent().expect("parent"))
            .await
            .expect("create parent");
        tokio::fs::write(&file, "{\"id\":1}\n\nnot-json\n{\"bad\":2}\n{\"id\":3}\n")
            .await
            .expect("write jsonl");
        let entries: Vec<Entry> =
            read_jsonl_file(&file, |value| serde_json::from_value(value.clone()).ok())
                .await
                .expect("read jsonl");
        assert_eq!(entries, vec![Entry { id: 1 }, Entry { id: 3 }]);
        cleanup(&file).await;
    }

    #[tokio::test]
    async fn append_jsonl_creates_parents_and_writes_compact_lines() {
        let file = temp_file("history/data.jsonl");
        append_jsonl_line(&file, validate_entry_value, &Entry { id: 1 })
            .await
            .expect("append first");
        append_jsonl_line(&file, validate_entry_value, &Entry { id: 2 })
            .await
            .expect("append second");
        assert_eq!(
            tokio::fs::read_to_string(&file).await.expect("read jsonl"),
            "{\"id\":1}\n{\"id\":2}\n"
        );
        cleanup(&file).await;
    }

    #[tokio::test]
    async fn refuses_to_write_config_toml() {
        let file = temp_file("config.toml");
        let error = write_json_file(&file, validate_entry_value, &Entry { id: 1 })
            .await
            .expect_err("config write rejected");
        assert!(error.to_string().contains("must not write config.toml"));
        assert!(!file.exists());
    }
}
