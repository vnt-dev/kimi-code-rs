use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use indexmap::IndexMap;
use thiserror::Error;
use tokio::fs::{self, OpenOptions};
use tokio::io::AsyncWriteExt;
use tokio::sync::Mutex;
use uuid::Uuid;

pub trait GuiStoreLogger: Send + Sync {
    fn warn_parse_failed(&self, file_path: &Path, error: &toml::de::Error);
}

#[derive(Debug)]
struct NoopLogger;

impl GuiStoreLogger for NoopLogger {
    fn warn_parse_failed(&self, _file_path: &Path, _error: &toml::de::Error) {}
}

#[derive(Debug, Error)]
pub enum GuiStoreError {
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error(transparent)]
    Serialize(#[from] toml::ser::Error),
}

/// Persistent TOML-backed store mirroring browser `localStorage`.
///
/// MIGRATION-TODO:
/// Original `guiStore.ts` registers this interface with agent-core-v2's
/// `createDecorator`. The service implementation is complete, but DI
/// registration must wait for the core-v2 container contract to be complete.
pub struct GuiStoreService {
    file_path: PathBuf,
    logger: Arc<dyn GuiStoreLogger>,
    mutation_lock: Mutex<()>,
}

impl std::fmt::Debug for GuiStoreService {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("GuiStoreService")
            .field("file_path", &self.file_path)
            .finish_non_exhaustive()
    }
}

impl GuiStoreService {
    pub fn new(home_dir: impl AsRef<Path>) -> Self {
        Self::with_logger(home_dir, Arc::new(NoopLogger))
    }

    pub fn with_logger(home_dir: impl AsRef<Path>, logger: Arc<dyn GuiStoreLogger>) -> Self {
        Self {
            file_path: home_dir.as_ref().join("gui.toml"),
            logger,
            mutation_lock: Mutex::new(()),
        }
    }

    // Original: guiStoreService.ts, GuiStoreService.getItem().
    pub async fn get_item(&self, key: &str) -> Result<Option<String>, GuiStoreError> {
        Ok(self.read_all().await?.get(key).cloned())
    }

    pub async fn set_item(&self, key: String, value: String) -> Result<(), GuiStoreError> {
        let _guard = self.mutation_lock.lock().await;
        let mut all = self.read_all().await?;
        all.insert(key, value);
        self.write_all(&all).await
    }

    pub async fn remove_item(&self, key: &str) -> Result<(), GuiStoreError> {
        let _guard = self.mutation_lock.lock().await;
        let mut all = self.read_all().await?;
        if all.shift_remove(key).is_some() {
            self.write_all(&all).await?;
        }
        Ok(())
    }

    pub async fn clear(&self) -> Result<(), GuiStoreError> {
        let _guard = self.mutation_lock.lock().await;
        self.write_all(&IndexMap::new()).await
    }

    pub async fn len(&self) -> Result<usize, GuiStoreError> {
        Ok(self.read_all().await?.len())
    }

    pub async fn is_empty(&self) -> Result<bool, GuiStoreError> {
        Ok(self.len().await? == 0)
    }

    async fn read_all(&self) -> Result<IndexMap<String, String>, GuiStoreError> {
        let text = match fs::read_to_string(&self.file_path).await {
            Ok(text) => text,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(IndexMap::new());
            }
            Err(error) => return Err(error.into()),
        };
        if text.trim().is_empty() {
            return Ok(IndexMap::new());
        }

        let table = match text.parse::<toml::Table>() {
            Ok(table) => table,
            Err(error) => {
                self.logger.warn_parse_failed(&self.file_path, &error);
                return Ok(IndexMap::new());
            }
        };
        Ok(table
            .into_iter()
            .filter_map(|(key, value)| value.as_str().map(|value| (key, value.to_owned())))
            .collect())
    }

    async fn write_all(&self, values: &IndexMap<String, String>) -> Result<(), GuiStoreError> {
        let directory = self.file_path.parent().unwrap_or_else(|| Path::new("."));
        fs::create_dir_all(directory).await?;
        let text = if values.is_empty() {
            String::new()
        } else {
            toml::to_string(values)?
        };
        write_atomic_private(&self.file_path, text.as_bytes()).await?;
        Ok(())
    }
}

async fn write_atomic_private(path: &Path, content: &[u8]) -> io::Result<()> {
    let mut temporary = path.as_os_str().to_owned();
    temporary.push(format!(
        ".tmp.{}.{}",
        std::process::id(),
        &Uuid::new_v4().simple().to_string()[..8]
    ));
    let temporary = PathBuf::from(temporary);
    let result = async {
        let mut options = OpenOptions::new();
        options.create_new(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temporary).await?;
        file.write_all(content).await?;
        drop(file);
        #[cfg(windows)]
        if let Err(error) = fs::remove_file(path).await
            && error.kind() != io::ErrorKind::NotFound
        {
            return Err(error);
        }
        fs::rename(&temporary, path).await
    }
    .await;
    if result.is_err() {
        let _ = fs::remove_file(&temporary).await;
    }
    result
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    #[tokio::test]
    async fn supports_local_storage_operations_and_persistence() {
        let directory = tempfile::tempdir().unwrap();
        let store = GuiStoreService::new(directory.path());
        assert_eq!(store.get_item("theme").await.unwrap(), None);
        assert!(store.is_empty().await.unwrap());

        store
            .set_item("theme".into(), "modern".into())
            .await
            .unwrap();
        store.set_item("other".into(), "2".into()).await.unwrap();
        store
            .set_item("theme".into(), "terminal".into())
            .await
            .unwrap();
        assert_eq!(
            store.get_item("theme").await.unwrap(),
            Some("terminal".into())
        );
        assert_eq!(store.len().await.unwrap(), 2);

        store.remove_item("theme").await.unwrap();
        store.remove_item("missing").await.unwrap();
        assert_eq!(store.get_item("theme").await.unwrap(), None);
        assert_eq!(store.get_item("other").await.unwrap(), Some("2".into()));
        store.clear().await.unwrap();
        assert!(store.is_empty().await.unwrap());
        assert_eq!(
            fs::read_to_string(directory.path().join("gui.toml"))
                .await
                .unwrap(),
            ""
        );
    }

    #[tokio::test]
    async fn quotes_dotted_and_javascript_prototype_keys() {
        let directory = tempfile::tempdir().unwrap();
        let store = GuiStoreService::new(directory.path());
        for (key, value) in [
            ("kimi-web.theme", "modern"),
            ("toString", "a"),
            ("constructor", "b"),
            ("hasOwnProperty", "c"),
            ("__proto__", "d"),
        ] {
            store
                .set_item(key.to_owned(), value.to_owned())
                .await
                .unwrap();
        }
        let text = fs::read_to_string(directory.path().join("gui.toml"))
            .await
            .unwrap();
        assert!(text.contains("\"kimi-web.theme\" = \"modern\""));
        assert_eq!(store.get_item("__proto__").await.unwrap(), Some("d".into()));
    }

    struct CountingLogger(AtomicUsize);

    impl GuiStoreLogger for CountingLogger {
        fn warn_parse_failed(&self, _file_path: &Path, _error: &toml::de::Error) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[tokio::test]
    async fn malformed_toml_is_an_empty_store_and_warns() {
        let directory = tempfile::tempdir().unwrap();
        fs::write(directory.path().join("gui.toml"), "{not valid")
            .await
            .unwrap();
        let logger = Arc::new(CountingLogger(AtomicUsize::new(0)));
        let store = GuiStoreService::with_logger(
            directory.path(),
            Arc::clone(&logger) as Arc<dyn GuiStoreLogger>,
        );
        assert!(store.is_empty().await.unwrap());
        assert_eq!(logger.0.load(Ordering::SeqCst), 1);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn writes_file_with_private_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::tempdir().unwrap();
        let store = GuiStoreService::new(directory.path());
        store.set_item("key".into(), "value".into()).await.unwrap();
        let mode = fs::metadata(directory.path().join("gui.toml"))
            .await
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
    }
}
