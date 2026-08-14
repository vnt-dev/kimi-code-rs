use std::{
    error::Error,
    ffi::OsStr,
    fmt, fs, io,
    path::{Path, PathBuf},
};

use async_trait::async_trait;
use kimi_code_fs::atomic_write;

use super::types::{TokenInfo, token_from_wire, token_to_wire};

#[derive(Debug)]
pub enum TokenStorageError {
    InvalidName(String),
    Io(io::Error),
    Json(serde_json::Error),
    Join(tokio::task::JoinError),
}

impl fmt::Display for TokenStorageError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidName(name) => write!(formatter, "Invalid token name: \"{name}\""),
            Self::Io(error) => error.fmt(formatter),
            Self::Json(error) => error.fmt(formatter),
            Self::Join(error) => write!(formatter, "token storage task failed: {error}"),
        }
    }
}

impl Error for TokenStorageError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::InvalidName(_) => None,
            Self::Io(error) => Some(error),
            Self::Json(error) => Some(error),
            Self::Join(error) => Some(error),
        }
    }
}

impl From<io::Error> for TokenStorageError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<serde_json::Error> for TokenStorageError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

#[async_trait]
pub trait TokenStorage: Send + Sync {
    async fn load(&self, name: &str) -> Result<Option<TokenInfo>, TokenStorageError>;
    async fn save(&self, name: &str, token: &TokenInfo) -> Result<(), TokenStorageError>;
    async fn remove(&self, name: &str) -> Result<(), TokenStorageError>;
    async fn list(&self) -> Result<Vec<String>, TokenStorageError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileTokenStorage {
    directory: PathBuf,
}

impl FileTokenStorage {
    pub fn new(directory: impl Into<PathBuf>) -> Self {
        Self {
            directory: directory.into(),
        }
    }

    fn path_for(&self, name: &str) -> Result<PathBuf, TokenStorageError> {
        let safe = Path::new(name).file_name();
        if name.is_empty()
            || name.starts_with('.')
            || safe != Some(OsStr::new(name))
            || name.contains('/')
            || (cfg!(windows) && name.contains('\\'))
        {
            return Err(TokenStorageError::InvalidName(name.to_owned()));
        }
        Ok(self.directory.join(format!("{name}.json")))
    }
}

#[async_trait]
impl TokenStorage for FileTokenStorage {
    // Original:
    //   packages/oauth/src/storage.ts
    //   FileTokenStorage.load()
    async fn load(&self, name: &str) -> Result<Option<TokenInfo>, TokenStorageError> {
        let file = self.path_for(name)?;
        tokio::task::spawn_blocking(move || load_token(&file))
            .await
            .map_err(TokenStorageError::Join)?
    }

    // Original: FileTokenStorage.save()
    async fn save(&self, name: &str, token: &TokenInfo) -> Result<(), TokenStorageError> {
        let target = self.path_for(name)?;
        ensure_private_directory(&self.directory).await?;
        let data = serde_json::to_string_pretty(&token_to_wire(token))? + "\n";
        atomic_write(target, data, Some(0o600)).await?;
        Ok(())
    }

    // Original: FileTokenStorage.remove()
    async fn remove(&self, name: &str) -> Result<(), TokenStorageError> {
        let file = self.path_for(name)?;
        tokio::task::spawn_blocking(move || match fs::remove_file(file) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(TokenStorageError::Io(error)),
        })
        .await
        .map_err(TokenStorageError::Join)?
    }

    // Original: FileTokenStorage.list()
    async fn list(&self) -> Result<Vec<String>, TokenStorageError> {
        let directory = self.directory.clone();
        tokio::task::spawn_blocking(move || {
            let entries = match fs::read_dir(directory) {
                Ok(entries) => entries,
                Err(_) => return Ok(Vec::new()),
            };
            Ok(entries
                .filter_map(Result::ok)
                .filter_map(|entry| entry.file_name().into_string().ok())
                .filter_map(|name| name.strip_suffix(".json").map(str::to_owned))
                .collect())
        })
        .await
        .map_err(TokenStorageError::Join)?
    }
}

fn load_token(file: &Path) -> Result<Option<TokenInfo>, TokenStorageError> {
    let raw = match fs::read_to_string(file) {
        Ok(raw) => raw,
        Err(_) => return Ok(None),
    };
    let value = match serde_json::from_str::<serde_json::Value>(&raw) {
        Ok(value) => value,
        Err(_) => return Ok(None),
    };
    let Some(object) = value.as_object() else {
        return Ok(None);
    };
    Ok(Some(token_from_wire(object)))
}

async fn ensure_private_directory(directory: &Path) -> io::Result<()> {
    tokio::fs::create_dir_all(directory).await?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = tokio::fs::set_permissions(directory, fs::Permissions::from_mode(0o700)).await;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

    fn temp_dir() -> PathBuf {
        let id = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!("kimi-token-storage-{}-{id}", std::process::id()))
    }

    fn token(access: &str) -> TokenInfo {
        TokenInfo {
            access_token: access.to_owned(),
            refresh_token: "rt-xyz".to_owned(),
            expires_at: 1_700_000_000,
            scope: "read write".to_owned(),
            token_type: "Bearer".to_owned(),
            expires_in: 3_600,
        }
    }

    #[tokio::test]
    async fn missing_corrupt_and_non_object_files_load_as_none() {
        let directory = temp_dir();
        fs::create_dir_all(&directory).expect("directory");
        let storage = FileTokenStorage::new(&directory);
        assert_eq!(storage.load("kimi-code").await.expect("missing"), None);
        for invalid in ["{ not json", "[\"array\"]"] {
            fs::write(directory.join("kimi-code.json"), invalid).expect("invalid");
            assert_eq!(storage.load("kimi-code").await.expect("invalid"), None);
        }
        fs::remove_dir_all(directory).expect("cleanup");
    }

    #[tokio::test]
    async fn saves_overwrites_and_loads_python_compatible_wire_format() {
        let directory = temp_dir();
        let storage = FileTokenStorage::new(&directory);
        storage
            .save("kimi-code", &token("first"))
            .await
            .expect("first");
        storage
            .save("kimi-code", &token("second"))
            .await
            .expect("second");
        assert_eq!(
            storage.load("kimi-code").await.expect("load"),
            Some(token("second"))
        );
        let value: serde_json::Value = serde_json::from_str(
            &fs::read_to_string(directory.join("kimi-code.json")).expect("wire"),
        )
        .expect("json");
        assert_eq!(value["access_token"], "second");
        assert!(value.get("accessToken").is_none());
        assert!(
            fs::read_dir(&directory)
                .expect("entries")
                .filter_map(Result::ok)
                .all(|entry| !entry.file_name().to_string_lossy().contains(".tmp."))
        );
        fs::remove_dir_all(directory).expect("cleanup");
    }

    #[tokio::test]
    async fn partial_wire_defaults_numeric_and_metadata_fields() {
        let directory = temp_dir();
        fs::create_dir_all(&directory).expect("directory");
        fs::write(
            directory.join("kimi-code.json"),
            r#"{"access_token":"a","refresh_token":"r"}"#,
        )
        .expect("partial");
        let token = FileTokenStorage::new(&directory)
            .load("kimi-code")
            .await
            .expect("load")
            .expect("token");
        assert_eq!(token.expires_at, 0);
        assert_eq!(token.expires_in, 0);
        assert_eq!(token.scope, "");
        assert_eq!(token.token_type, "");
        fs::remove_dir_all(directory).expect("cleanup");
    }

    #[tokio::test]
    async fn list_filters_json_and_remove_is_idempotent() {
        let directory = temp_dir();
        let storage = FileTokenStorage::new(&directory);
        storage.save("kimi-code", &token("one")).await.expect("one");
        storage.save("other", &token("two")).await.expect("two");
        fs::write(directory.join("readme.txt"), "ignored").expect("extra");
        let mut names = storage.list().await.expect("list");
        names.sort();
        assert_eq!(names, ["kimi-code", "other"]);
        storage.remove("kimi-code").await.expect("remove");
        storage.remove("kimi-code").await.expect("remove again");
        assert_eq!(storage.load("kimi-code").await.expect("removed"), None);
        fs::remove_dir_all(directory).expect("cleanup");
    }

    #[tokio::test]
    async fn rejects_traversal_hidden_and_empty_names_for_every_operation() {
        let storage = FileTokenStorage::new(temp_dir());
        for name in ["", ".hidden", "../etc/passwd", "../../etc/passwd"] {
            assert!(matches!(
                storage.load(name).await,
                Err(TokenStorageError::InvalidName(_))
            ));
            assert!(matches!(
                storage.save(name, &token("a")).await,
                Err(TokenStorageError::InvalidName(_))
            ));
            assert!(matches!(
                storage.remove(name).await,
                Err(TokenStorageError::InvalidName(_))
            ));
        }
    }
}
