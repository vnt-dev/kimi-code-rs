//! Process-wide known-workspace registry service.
//!
//! Original:
//! `packages/agent-core-v2/src/app/workspaceRegistry/workspaceRegistryService.ts`.

use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::SystemTime,
};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use indexmap::{IndexMap, IndexSet};
use serde_json::Value;
use tokio::sync::Mutex;

use crate::{
    _base::{
        di::{
            descriptors::SyncDescriptor,
            instantiation::ServicesAccessorExt,
            scope::{InstantiationType, LifecycleScope, register_scoped_service},
        },
        errors::errors::Error2,
        utils::workdir_slug::{encode_work_dir_key, workspace_root_key},
    },
    os::interface::{
        host_file_system::{HOST_FILE_SYSTEM_SERVICE_ID, HostFileSystemService},
        host_fs_errors::{OS_FS_NOT_DIRECTORY, OS_FS_NOT_FOUND},
    },
    persistence::interface::storage::{FILE_SYSTEM_STORAGE_SERVICE_ID, FileSystemStorageService},
    session::session_fs::errors::{FS_PATH_NOT_FOUND, ensure_fs_errors_registered},
};

use super::{
    contract::{
        WORKSPACE_REGISTRY_SERVICE_ID, Workspace, WorkspaceRegistryContract,
        WorkspaceRegistryHandle, WorkspaceRegistryResult, WorkspaceUpdate,
    },
    persistence::{
        WORKSPACE_PERSISTENCE_SERVICE_ID, WorkspaceCatalog, WorkspacePersistenceContract,
    },
};

const SESSION_INDEX_SCOPE: &str = "";
const SESSION_INDEX_KEY: &str = "session_index.jsonl";

#[derive(Clone, Debug, Eq, PartialEq)]
struct SessionIndexLine {
    session_id: String,
    session_dir: String,
    work_dir: String,
}

pub struct WorkspaceRegistryService {
    store: Arc<dyn WorkspacePersistenceContract>,
    storage: Arc<dyn FileSystemStorageService>,
    host_fs: Arc<dyn HostFileSystemService>,
    merged: AtomicBool,
    operations: Mutex<()>,
    now_millis: Arc<dyn Fn() -> i64 + Send + Sync>,
}

impl WorkspaceRegistryService {
    pub fn new(
        store: Arc<dyn WorkspacePersistenceContract>,
        storage: Arc<dyn FileSystemStorageService>,
        host_fs: Arc<dyn HostFileSystemService>,
    ) -> Self {
        Self::with_clock(store, storage, host_fs, Arc::new(current_time_millis))
    }

    fn with_clock(
        store: Arc<dyn WorkspacePersistenceContract>,
        storage: Arc<dyn FileSystemStorageService>,
        host_fs: Arc<dyn HostFileSystemService>,
        now_millis: Arc<dyn Fn() -> i64 + Send + Sync>,
    ) -> Self {
        ensure_fs_errors_registered();
        Self {
            store,
            storage,
            host_fs,
            merged: AtomicBool::new(false),
            operations: Mutex::new(()),
            now_millis,
        }
    }

    // Original: WorkspaceRegistryService.collectAliasIds(). Callers hold the
    // operation mutex, so catalog and session-index reads cannot interleave
    // with this process's registry mutations.
    async fn collect_alias_ids(
        &self,
        catalog: &WorkspaceCatalog,
        root: &str,
    ) -> WorkspaceRegistryResult<Vec<String>> {
        let root_key = workspace_root_key(root);
        let mut ids = IndexSet::new();
        for workspace in &catalog.workspaces {
            if workspace_root_key(&workspace.root) == root_key {
                ids.insert(workspace.id.clone());
            }
        }
        for line in self.read_session_index_entries().await? {
            if workspace_root_key(&line.work_dir) == root_key {
                ids.insert(encode_work_dir_key(&line.work_dir));
            }
        }
        Ok(ids.into_iter().collect())
    }

    // Original: WorkspaceRegistryService.ensureMerged(). This is called only
    // while holding `operations`; merged flips only after persistence succeeds.
    async fn ensure_merged(&self) -> WorkspaceRegistryResult<()> {
        if self.merged.load(Ordering::Acquire) {
            return Ok(());
        }
        let loaded = self.store.load().await?;
        if let Some(loaded) = loaded {
            let mut by_id = catalog_by_id(&loaded.workspaces);
            let deleted_ids = loaded.deleted_ids.iter().cloned().collect::<IndexSet<_>>();
            if self
                .merge_from_session_index(&mut by_id, &deleted_ids)
                .await?
            {
                self.store
                    .save(&WorkspaceCatalog {
                        workspaces: by_id.into_values().collect(),
                        deleted_ids: deleted_ids.into_iter().collect(),
                    })
                    .await?;
            }
        } else {
            let rebuilt = self.rebuild_from_session_index().await?;
            self.store
                .save(&WorkspaceCatalog {
                    workspaces: rebuilt.into_values().collect(),
                    deleted_ids: Vec::new(),
                })
                .await?;
        }
        self.merged.store(true, Ordering::Release);
        Ok(())
    }

    async fn load_catalog(&self) -> WorkspaceRegistryResult<WorkspaceCatalog> {
        Ok(self.store.load().await?.unwrap_or_default())
    }

    // Original: WorkspaceRegistryService.mergeFromSessionIndex().
    async fn merge_from_session_index(
        &self,
        by_id: &mut IndexMap<String, Workspace>,
        deleted_ids: &IndexSet<String>,
    ) -> WorkspaceRegistryResult<bool> {
        let mut changed = false;
        let now = (self.now_millis)();
        for work_dir in self.read_session_index_work_dirs().await? {
            let id = encode_work_dir_key(&work_dir);
            if by_id.contains_key(&id) || deleted_ids.contains(&id) {
                continue;
            }
            by_id.insert(
                id.clone(),
                Workspace {
                    id,
                    root: work_dir.clone(),
                    name: path_basename(&work_dir),
                    created_at_millis: now,
                    last_opened_at_millis: now,
                },
            );
            changed = true;
        }
        Ok(changed)
    }

    // Original: WorkspaceRegistryService.rebuildFromSessionIndex().
    async fn rebuild_from_session_index(
        &self,
    ) -> WorkspaceRegistryResult<IndexMap<String, Workspace>> {
        let mut result = IndexMap::new();
        let mut seen_roots = IndexSet::new();
        let now = (self.now_millis)();
        for entry in self.read_session_index_entries().await? {
            if !is_absolute_path(&entry.work_dir) {
                continue;
            }
            let root_key = workspace_root_key(&entry.work_dir);
            if !seen_roots.insert(root_key) {
                continue;
            }
            let id = encode_work_dir_key(&entry.work_dir);
            result.insert(
                id.clone(),
                Workspace {
                    id,
                    root: entry.work_dir.clone(),
                    name: path_basename(&entry.work_dir),
                    created_at_millis: now,
                    last_opened_at_millis: now,
                },
            );
        }
        Ok(result)
    }

    async fn read_session_index_work_dirs(&self) -> WorkspaceRegistryResult<Vec<String>> {
        Ok(self
            .read_session_index_entries()
            .await?
            .into_iter()
            .filter(|entry| is_absolute_path(&entry.work_dir))
            .map(|entry| entry.work_dir)
            .collect())
    }

    // Original: WorkspaceRegistryService.readSessionIndexEntries().
    async fn read_session_index_entries(&self) -> WorkspaceRegistryResult<Vec<SessionIndexLine>> {
        let Some(bytes) = self
            .storage
            .read(SESSION_INDEX_SCOPE, SESSION_INDEX_KEY)
            .await?
        else {
            return Ok(Vec::new());
        };
        Ok(String::from_utf8_lossy(&bytes)
            .split('\n')
            .filter_map(|line| parse_session_index_line(line.trim()))
            .collect())
    }

    async fn assert_workspace_root(&self, root: &str) -> WorkspaceRegistryResult<()> {
        let mut stat = match self.host_fs.stat(Path::new(root)).await {
            Ok(stat) => stat,
            Err(error) if matches!(error.code(), OS_FS_NOT_FOUND | OS_FS_NOT_DIRECTORY) => {
                return Err(Box::new(Error2::new(
                    FS_PATH_NOT_FOUND,
                    format!("workspace root {root} does not exist"),
                )));
            }
            Err(error) => return Err(Box::new(error)),
        };
        if !stat.is_directory
            && let Ok(real_path) = self.host_fs.real_path(Path::new(root)).await
            && let Ok(real_stat) = self.host_fs.stat(Path::new(&real_path)).await
        {
            stat = real_stat;
        }
        if !stat.is_directory {
            return Err(Box::new(Error2::new(
                FS_PATH_NOT_FOUND,
                format!("workspace root {root} is not a directory"),
            )));
        }
        Ok(())
    }
}

#[async_trait]
impl WorkspaceRegistryContract for WorkspaceRegistryService {
    // Original: WorkspaceRegistryService.list().
    async fn list(&self) -> WorkspaceRegistryResult<Vec<Workspace>> {
        let _operation = self.operations.lock().await;
        self.ensure_merged().await?;
        let catalog = self.load_catalog().await?;
        Ok(dedupe_by_root(&catalog_by_id(&catalog.workspaces)))
    }

    // Original: WorkspaceRegistryService.get().
    async fn get(&self, id: &str) -> WorkspaceRegistryResult<Option<Workspace>> {
        let _operation = self.operations.lock().await;
        self.ensure_merged().await?;
        Ok(self
            .load_catalog()
            .await?
            .workspaces
            .into_iter()
            .find(|workspace| workspace.id == id))
    }

    // Original: WorkspaceRegistryService.resolveAliasIds().
    async fn resolve_alias_ids(&self, id: &str) -> WorkspaceRegistryResult<Vec<String>> {
        let _operation = self.operations.lock().await;
        self.ensure_merged().await?;
        let catalog = self.load_catalog().await?;
        let Some(entry) = catalog
            .workspaces
            .iter()
            .find(|workspace| workspace.id == id)
        else {
            return Ok(vec![id.into()]);
        };
        self.collect_alias_ids(&catalog, &entry.root).await
    }

    // Original: WorkspaceRegistryService.createOrTouch().
    async fn create_or_touch(
        &self,
        root: &str,
        name: Option<&str>,
    ) -> WorkspaceRegistryResult<Workspace> {
        let _operation = self.operations.lock().await;
        self.assert_workspace_root(root).await?;
        self.ensure_merged().await?;
        let catalog = self.load_catalog().await?;
        let mut by_id = catalog_by_id(&catalog.workspaces);
        let mut deleted_ids = catalog.deleted_ids.into_iter().collect::<IndexSet<_>>();
        let id = encode_work_dir_key(root);
        let existing = by_id.get(&id).cloned().or_else(|| {
            let root_key = workspace_root_key(root);
            by_id
                .values()
                .find(|entry| workspace_root_key(&entry.root) == root_key)
                .cloned()
        });
        let now = (self.now_millis)();
        let workspace = existing.map_or_else(
            || Workspace {
                id,
                root: root.into(),
                name: name.map_or_else(|| path_basename(root), str::to_owned),
                created_at_millis: now,
                last_opened_at_millis: now,
            },
            |mut existing| {
                existing.last_opened_at_millis = now;
                existing
            },
        );
        by_id.insert(workspace.id.clone(), workspace.clone());
        deleted_ids.shift_remove(&workspace.id);
        self.store
            .save(&WorkspaceCatalog {
                workspaces: by_id.into_values().collect(),
                deleted_ids: deleted_ids.into_iter().collect(),
            })
            .await?;
        Ok(workspace)
    }

    // Original: WorkspaceRegistryService.update().
    async fn update(
        &self,
        id: &str,
        patch: WorkspaceUpdate,
    ) -> WorkspaceRegistryResult<Option<Workspace>> {
        let _operation = self.operations.lock().await;
        self.ensure_merged().await?;
        let mut catalog = self.load_catalog().await?;
        let Some(index) = catalog
            .workspaces
            .iter()
            .position(|workspace| workspace.id == id)
        else {
            return Ok(None);
        };
        let mut updated = catalog.workspaces[index].clone();
        if let Some(name) = patch.name {
            updated.name = name;
        }
        catalog.workspaces[index] = updated.clone();
        self.store.save(&catalog).await?;
        Ok(Some(updated))
    }

    // Original: WorkspaceRegistryService.delete().
    async fn delete(&self, id: &str) -> WorkspaceRegistryResult<()> {
        let _operation = self.operations.lock().await;
        self.ensure_merged().await?;
        let catalog = self.load_catalog().await?;
        let mut root = catalog
            .workspaces
            .iter()
            .find(|workspace| workspace.id == id)
            .map(|workspace| workspace.root.clone());
        if root.is_none() {
            root = self
                .read_session_index_entries()
                .await?
                .into_iter()
                .find(|line| encode_work_dir_key(&line.work_dir) == id)
                .map(|line| line.work_dir);
        }
        let Some(root) = root else {
            let mut deleted = catalog.deleted_ids.into_iter().collect::<IndexSet<_>>();
            deleted.insert(id.into());
            self.store
                .save(&WorkspaceCatalog {
                    workspaces: catalog
                        .workspaces
                        .into_iter()
                        .filter(|workspace| workspace.id != id)
                        .collect(),
                    deleted_ids: deleted.into_iter().collect(),
                })
                .await?;
            return Ok(());
        };

        let root_key = workspace_root_key(&root);
        let aliases = self.collect_alias_ids(&catalog, &root).await?;
        let mut deleted = catalog.deleted_ids.into_iter().collect::<IndexSet<_>>();
        deleted.extend(aliases);
        self.store
            .save(&WorkspaceCatalog {
                workspaces: catalog
                    .workspaces
                    .into_iter()
                    .filter(|workspace| workspace_root_key(&workspace.root) != root_key)
                    .collect(),
                deleted_ids: deleted.into_iter().collect(),
            })
            .await?;
        Ok(())
    }
}

fn parse_session_index_line(line: &str) -> Option<SessionIndexLine> {
    if line.is_empty() {
        return None;
    }
    let Value::Object(value) = serde_json::from_str(line).ok()? else {
        return None;
    };
    Some(SessionIndexLine {
        session_id: value.get("sessionId")?.as_str()?.into(),
        session_dir: value.get("sessionDir")?.as_str()?.into(),
        work_dir: value.get("workDir")?.as_str()?.into(),
    })
}

// Original: dedupeByRoot().
fn dedupe_by_root(by_id: &IndexMap<String, Workspace>) -> Vec<Workspace> {
    let mut by_root = IndexMap::<String, Workspace>::new();
    for workspace in by_id.values() {
        let root_key = workspace_root_key(&workspace.root);
        match by_root.get(&root_key) {
            None => {
                by_root.insert(root_key, workspace.clone());
            }
            Some(existing)
                if existing.id != encode_work_dir_key(&workspace.root)
                    && workspace.id == encode_work_dir_key(&workspace.root) =>
            {
                by_root.insert(root_key, workspace.clone());
            }
            Some(_) => {}
        }
    }
    by_root.into_values().collect()
}

fn catalog_by_id(workspaces: &[Workspace]) -> IndexMap<String, Workspace> {
    workspaces
        .iter()
        .map(|workspace| (workspace.id.clone(), workspace.clone()))
        .collect()
}

fn path_basename(path: &str) -> String {
    path.replace('\\', "/")
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or_default()
        .into()
}

fn is_absolute_path(path: &str) -> bool {
    let bytes = path.as_bytes();
    path.starts_with('/')
        || path.starts_with("\\\\")
        || path.starts_with("//")
        || (bytes.len() >= 3
            && bytes[0].is_ascii_alphabetic()
            && bytes[1] == b':'
            && matches!(bytes[2], b'/' | b'\\'))
}

fn current_time_millis() -> i64 {
    let now: DateTime<Utc> = SystemTime::now().into();
    now.timestamp_millis()
}

pub fn register_workspace_registry_service() {
    ensure_fs_errors_registered();
    register_scoped_service(
        LifecycleScope::App,
        WORKSPACE_REGISTRY_SERVICE_ID,
        SyncDescriptor::new(|accessor| {
            let store = accessor.get(WORKSPACE_PERSISTENCE_SERVICE_ID)?;
            let storage = accessor.get(FILE_SYSTEM_STORAGE_SERVICE_ID)?;
            let host_fs = accessor.get(HOST_FILE_SYSTEM_SERVICE_ID)?;
            let service: Arc<dyn WorkspaceRegistryContract> =
                Arc::new(WorkspaceRegistryService::new(
                    Arc::clone(&store.0),
                    Arc::clone(&storage.0),
                    Arc::clone(&host_fs.0),
                ));
            Ok(WorkspaceRegistryHandle(service))
        }),
        InstantiationType::Eager,
        "workspaceRegistry",
    );
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex as StdMutex;

    use crate::{
        os::backends::node_local::host_fs_service::HostFileSystem,
        persistence::{
            backends::memory::in_memory_storage_service::InMemoryStorageService,
            interface::storage::{FileSystemStorageService, StorageWriteOptions},
        },
    };

    use super::*;

    #[derive(Default)]
    struct MemoryWorkspaceStore {
        catalog: StdMutex<Option<WorkspaceCatalog>>,
        saves: StdMutex<Vec<WorkspaceCatalog>>,
    }

    #[async_trait]
    impl WorkspacePersistenceContract for MemoryWorkspaceStore {
        async fn load(&self) -> super::super::WorkspacePersistenceResult<Option<WorkspaceCatalog>> {
            Ok(self.catalog.lock().unwrap().clone())
        }

        async fn save(
            &self,
            catalog: &WorkspaceCatalog,
        ) -> super::super::WorkspacePersistenceResult<()> {
            *self.catalog.lock().unwrap() = Some(catalog.clone());
            self.saves.lock().unwrap().push(catalog.clone());
            Ok(())
        }
    }

    fn service(
        catalog: Option<WorkspaceCatalog>,
    ) -> (
        WorkspaceRegistryService,
        Arc<MemoryWorkspaceStore>,
        Arc<InMemoryStorageService>,
    ) {
        let store = Arc::new(MemoryWorkspaceStore {
            catalog: StdMutex::new(catalog),
            saves: StdMutex::new(Vec::new()),
        });
        let storage = Arc::new(InMemoryStorageService::default());
        let registry = WorkspaceRegistryService::with_clock(
            Arc::clone(&store) as Arc<dyn WorkspacePersistenceContract>,
            Arc::clone(&storage) as Arc<dyn FileSystemStorageService>,
            Arc::new(HostFileSystem),
            Arc::new(|| 42),
        );
        (registry, store, storage)
    }

    async fn write_session_index(storage: &InMemoryStorageService, lines: &[Value]) {
        let mut text = lines
            .iter()
            .map(Value::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        text.push_str("\nmalformed\n");
        storage
            .write(
                SESSION_INDEX_SCOPE,
                SESSION_INDEX_KEY,
                text.as_bytes(),
                StorageWriteOptions::default(),
            )
            .await
            .unwrap();
    }

    fn line(work_dir: &str) -> Value {
        serde_json::json!({
            "sessionId": "s", "sessionDir": "/sessions/s", "workDir": work_dir
        })
    }

    #[test]
    fn parser_and_dedupe_preserve_legacy_tolerance_and_canonical_preference() {
        assert!(parse_session_index_line("not json").is_none());
        assert!(parse_session_index_line(r#"{"sessionId":"s"}"#).is_none());
        assert!(is_absolute_path(r"C:\repo"));
        let root = r"C:\Repo";
        let canonical = Workspace {
            id: encode_work_dir_key(root),
            root: root.into(),
            name: "canonical".into(),
            created_at_millis: 1,
            last_opened_at_millis: 1,
        };
        let legacy = Workspace {
            id: "legacy".into(),
            name: "legacy".into(),
            ..canonical.clone()
        };
        let by_id = IndexMap::from([
            (legacy.id.clone(), legacy),
            (canonical.id.clone(), canonical.clone()),
        ]);
        assert_eq!(dedupe_by_root(&by_id), [canonical]);
    }

    #[tokio::test]
    async fn first_list_rebuilds_from_absolute_distinct_session_roots() {
        let (registry, store, storage) = service(None);
        write_session_index(
            &storage,
            &[line("/repo/a"), line("relative"), line("/repo/a")],
        )
        .await;

        let listed = registry.list().await.unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].root, "/repo/a");
        assert_eq!(listed[0].created_at_millis, 42);
        assert_eq!(store.saves.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn merge_respects_tombstones_and_alias_resolution_reads_split_buckets() {
        let primary = Workspace {
            id: encode_work_dir_key(r"C:\Repo"),
            root: r"C:\Repo".into(),
            name: "Repo".into(),
            created_at_millis: 1,
            last_opened_at_millis: 1,
        };
        let deleted = encode_work_dir_key("/deleted");
        let (registry, store, storage) = service(Some(WorkspaceCatalog {
            workspaces: vec![primary.clone()],
            deleted_ids: vec![deleted.clone()],
        }));
        write_session_index(
            &storage,
            &[line(r"c:/repo"), line("/deleted"), line("/new")],
        )
        .await;

        let aliases = registry.resolve_alias_ids(&primary.id).await.unwrap();
        assert_eq!(
            aliases,
            [primary.id.clone(), encode_work_dir_key("c:/repo")]
        );
        let catalog = store.catalog.lock().unwrap().clone().unwrap();
        assert!(catalog.workspaces.iter().any(|item| item.root == "/new"));
        assert!(
            !catalog
                .workspaces
                .iter()
                .any(|item| item.root == "/deleted")
        );
    }

    #[tokio::test]
    async fn create_touch_update_and_delete_preserve_order_and_tombstones() {
        let root = std::env::temp_dir().join(format!("registry-{}", uuid::Uuid::new_v4()));
        tokio::fs::create_dir_all(&root).await.unwrap();
        let (registry, store, _) = service(Some(WorkspaceCatalog::default()));
        let root_text = root.to_string_lossy();

        let created = registry
            .create_or_touch(&root_text, Some("custom"))
            .await
            .unwrap();
        assert_eq!(created.name, "custom");
        let updated = registry
            .update(
                &created.id,
                WorkspaceUpdate {
                    name: Some("renamed".into()),
                },
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(updated.name, "renamed");
        registry.delete(&created.id).await.unwrap();
        let catalog = store.catalog.lock().unwrap().clone().unwrap();
        assert!(catalog.workspaces.is_empty());
        assert_eq!(catalog.deleted_ids, [created.id]);
        tokio::fs::remove_dir_all(root).await.unwrap();
    }

    #[tokio::test]
    async fn missing_root_uses_fs_path_not_found_error() {
        let (registry, _, _) = service(Some(WorkspaceCatalog::default()));
        let missing = std::env::temp_dir().join(format!("missing-{}", uuid::Uuid::new_v4()));
        let error = registry
            .create_or_touch(missing.to_str().unwrap(), None)
            .await
            .unwrap_err();
        assert_eq!(
            error.downcast_ref::<Error2>().unwrap().code,
            FS_PATH_NOT_FOUND
        );
    }
}
