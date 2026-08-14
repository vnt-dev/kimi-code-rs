//! Best-effort, one-shot persisted config migrations.
//!
//! Original: `packages/agent-core-v2/src/app/config/migrations.ts`.

use std::{collections::HashMap, path::Path, time::SystemTime};

use chrono::{DateTime, SecondsFormat, Utc};
use serde_json::{Map, Value};

use crate::_base::utils::fs::atomic_write;
use crate::persistence::interface::atomic_document_store::AtomicDocumentStoreHandle;

const MIGRATIONS_FILE: &str = "migrations-effort.json";
const THINKING_EFFORT_MAX_TO_HIGH: &str = "thinking-effort-max-to-high";
const CONFIG_SCOPE: &str = "";

// Original: readMigrationMarkers(). Missing, corrupt, and non-object documents
// all mean that no migration has completed.
async fn read_migration_markers(home_dir: &Path) -> HashMap<String, String> {
    tokio::fs::read(home_dir.join(MIGRATIONS_FILE))
        .await
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

// Original: writeMigrationMarker(). Every failure is intentionally ignored;
// a lost marker merely causes another safe check at the next startup.
async fn write_migration_marker(home_dir: &Path, key: &str) {
    let _ = try_write_migration_marker(home_dir, key).await;
}

async fn try_write_migration_marker(home_dir: &Path, key: &str) -> std::io::Result<()> {
    create_private_dir_all(home_dir).await?;
    let mut markers = read_migration_markers(home_dir).await;
    let now: DateTime<Utc> = SystemTime::now().into();
    markers.insert(key.into(), now.to_rfc3339_opts(SecondsFormat::Millis, true));
    let mut bytes = serde_json::to_vec_pretty(&markers).map_err(std::io::Error::other)?;
    bytes.push(b'\n');
    let path = home_dir.join(MIGRATIONS_FILE);
    atomic_write(path, bytes, Some(0o600)).await
}

async fn create_private_dir_all(path: &Path) -> std::io::Result<()> {
    tokio::fs::create_dir_all(path).await?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        tokio::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).await?;
    }
    Ok(())
}

// Original: migrateThinkingEffortMaxToHigh(). This method deliberately catches
// all store and marker failures. An unreadable config remains unmarked so a
// later startup retries; a successful inspection is marked even when no edit
// was necessary.
pub async fn migrate_thinking_effort_max_to_high(
    document_store: &AtomicDocumentStoreHandle,
    config_key: &str,
    home_dir: &Path,
) {
    if read_migration_markers(home_dir)
        .await
        .contains_key(THINKING_EFFORT_MAX_TO_HIGH)
    {
        return;
    }

    let Ok(stored) = document_store.get::<Value>(CONFIG_SCOPE, config_key).await else {
        return;
    };
    let mut document = stored
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default();
    if rewrite_thinking_effort(&mut document)
        && document_store
            .set(CONFIG_SCOPE, config_key, &Value::Object(document))
            .await
            .is_err()
    {
        return;
    }
    write_migration_marker(home_dir, THINKING_EFFORT_MAX_TO_HIGH).await;
}

fn rewrite_thinking_effort(document: &mut Map<String, Value>) -> bool {
    let Some(Value::Object(thinking)) = document.get_mut("thinking") else {
        return false;
    };
    if thinking.get("effort") != Some(&Value::String("max".into())) {
        return false;
    }
    thinking.insert("effort".into(), Value::String("high".into()));
    true
}

#[cfg(test)]
mod tests {
    use parking_lot::Mutex;
    use std::path::PathBuf;
    use std::sync::Arc;

    use async_trait::async_trait;

    use crate::{
        _base::{
            di::lifecycle::{DisposableHandle, disposable_none},
            event::Event,
        },
        persistence::interface::{
            atomic_document_store::AtomicDocumentStoreService,
            storage::{STORAGE_IO_FAILED, StorageError},
        },
    };

    use super::*;

    struct StubStore {
        value: Mutex<Option<Value>>,
        fail_reads: Mutex<bool>,
    }

    #[async_trait]
    impl AtomicDocumentStoreService for StubStore {
        async fn get_value(&self, _scope: &str, _key: &str) -> Result<Option<Value>, StorageError> {
            if *self.fail_reads.lock() {
                Err(StorageError::new(STORAGE_IO_FAILED, "unreadable"))
            } else {
                Ok(self.value.lock().clone())
            }
        }

        async fn set_value(
            &self,
            _scope: &str,
            _key: &str,
            value: Value,
        ) -> Result<(), StorageError> {
            *self.value.lock() = Some(value);
            Ok(())
        }

        async fn delete(&self, _scope: &str, _key: &str) -> Result<(), StorageError> {
            Ok(())
        }

        async fn list(
            &self,
            _scope: &str,
            _prefix: Option<&str>,
        ) -> Result<Vec<String>, StorageError> {
            Ok(Vec::new())
        }

        fn watch(&self, _scope: &str, _key: &str) -> Event<()> {
            Event::none()
        }

        fn acquire(&self, _scope: &str, _key: &str) -> DisposableHandle {
            disposable_none()
        }
    }

    fn temp_home() -> PathBuf {
        std::env::temp_dir().join(format!("kimi-config-migration-{}", uuid::Uuid::new_v4()))
    }

    #[tokio::test]
    async fn rewrites_once_and_honors_a_later_user_max_value() {
        let backend = Arc::new(StubStore {
            value: Mutex::new(Some(serde_json::json!({
                "thinking": {"effort": "max", "other": true},
                "untouched": 1
            }))),
            fail_reads: Mutex::new(false),
        });
        let handle = AtomicDocumentStoreHandle(backend.clone());
        let home = temp_home();
        migrate_thinking_effort_max_to_high(&handle, "config.toml", &home).await;
        assert_eq!(
            backend.value.lock().as_ref().unwrap()["thinking"]["effort"],
            "high"
        );
        assert!(
            read_migration_markers(&home)
                .await
                .contains_key(THINKING_EFFORT_MAX_TO_HIGH)
        );

        backend.value.lock().as_mut().unwrap()["thinking"]["effort"] = Value::String("max".into());
        migrate_thinking_effort_max_to_high(&handle, "config.toml", &home).await;
        assert_eq!(
            backend.value.lock().as_ref().unwrap()["thinking"]["effort"],
            "max"
        );
        std::fs::remove_dir_all(home).unwrap();
    }

    #[tokio::test]
    async fn unreadable_config_is_not_marked_and_is_retried() {
        let backend = Arc::new(StubStore {
            value: Mutex::new(Some(serde_json::json!({"thinking": {"effort": "max"}}))),
            fail_reads: Mutex::new(true),
        });
        let handle = AtomicDocumentStoreHandle(backend.clone());
        let home = temp_home();
        migrate_thinking_effort_max_to_high(&handle, "config.toml", &home).await;
        assert!(!home.join(MIGRATIONS_FILE).exists());

        *backend.fail_reads.lock() = false;
        migrate_thinking_effort_max_to_high(&handle, "config.toml", &home).await;
        assert_eq!(
            backend.value.lock().as_ref().unwrap()["thinking"]["effort"],
            "high"
        );
        std::fs::remove_dir_all(home).unwrap();
    }
}
