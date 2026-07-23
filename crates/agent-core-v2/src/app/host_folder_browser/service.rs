//! Host filesystem folder browsing and recent workspace roots.
//!
//! Original: `packages/agent-core-v2/src/app/hostFolderBrowser/hostFolderBrowserService.ts`.

use std::{cmp::Ordering, path::Path, sync::Arc};

use async_trait::async_trait;

use crate::{
    _base::di::{
        descriptors::SyncDescriptor,
        instantiation::ServicesAccessorExt,
        scope::{InstantiationType, LifecycleScope, register_scoped_service},
    },
    app::{
        bootstrap::BOOTSTRAP_SERVICE_ID,
        workspace_registry::{WORKSPACE_REGISTRY_SERVICE_ID, WorkspaceRegistryContract},
    },
    os::interface::{
        host_file_system::{HOST_FILE_SYSTEM_SERVICE_ID, HostFileSystemService},
        host_fs_errors::{
            HostFsError, OS_FS_NOT_DIRECTORY, OS_FS_NOT_FOUND, OS_FS_PERMISSION_DENIED,
        },
    },
};

use super::contract::{
    FS_HOST_FOLDER_BROWSER_ID, FsBrowseEntry, FsBrowseResponse, FsHomeResponse,
    HostFolderBrowserContract, HostFolderBrowserError, HostFolderBrowserHandle,
    HostFolderBrowserResult, HostFolderNotAbsoluteError, HostFolderNotFoundError,
    HostFolderPermissionError, RECENT_ROOTS_LIMIT,
};

pub struct HostFolderBrowser {
    registry: Arc<dyn WorkspaceRegistryContract>,
    host_fs: Arc<dyn HostFileSystemService>,
    home_dir: String,
}

impl HostFolderBrowser {
    // Original: HostFolderBrowser.constructor(). The original obtains
    // `homedir()` in each public method. Rust receives the frozen bootstrap
    // value so the host fact remains injectable and process-wide.
    pub fn new(
        registry: Arc<dyn WorkspaceRegistryContract>,
        host_fs: Arc<dyn HostFileSystemService>,
        home_dir: impl Into<String>,
    ) -> Self {
        Self {
            registry,
            host_fs,
            home_dir: home_dir.into(),
        }
    }
}

#[async_trait]
impl HostFolderBrowserContract for HostFolderBrowser {
    // Original: HostFolderBrowser.browse(). Filesystem calls remain
    // sequential: `realpath` must complete before `readdir` starts.
    async fn browse(
        &self,
        absolute_path: Option<&str>,
    ) -> HostFolderBrowserResult<FsBrowseResponse> {
        let target = absolute_path.unwrap_or(&self.home_dir);
        if !Path::new(target).is_absolute() {
            return Err(Box::new(HostFolderNotAbsoluteError::new(target)));
        }

        let real_target = self
            .host_fs
            .real_path(Path::new(target))
            .await
            .map_err(|error| map_fs_error(error, target))?;
        let dirents = self
            .host_fs
            .read_dir(Path::new(&real_target))
            .await
            .map_err(|error| map_fs_error(error, &real_target))?;

        let mut entries = dirents
            .into_iter()
            .filter(|entry| entry.is_directory)
            .map(|entry| FsBrowseEntry {
                path: Path::new(&real_target)
                    .join(&entry.name)
                    .to_string_lossy()
                    .into_owned(),
                name: entry.name,
                is_dir: true,
            })
            .collect::<Vec<_>>();
        entries.sort_by(compare_browse_entries);

        let parent = Path::new(&real_target)
            .parent()
            .map(|path| path.to_string_lossy().into_owned())
            .filter(|path| path != &real_target && !path.is_empty());

        Ok(FsBrowseResponse {
            path: real_target,
            parent,
            entries,
        })
    }

    // Original: HostFolderBrowser.home(). Registry ordering is preserved and
    // only the first eight roots are exposed.
    async fn home(&self) -> HostFolderBrowserResult<FsHomeResponse> {
        let workspaces = self.registry.list().await?;
        let recent_roots = workspaces
            .into_iter()
            .take(RECENT_ROOTS_LIMIT)
            .map(|workspace| workspace.root)
            .collect();
        Ok(FsHomeResponse {
            home: self.home_dir.clone(),
            recent_roots,
        })
    }
}

// Original: mapFsError(). The host filesystem boundary has already converted
// platform errno values into stable codes, so this preserves the same mapping.
fn map_fs_error(error: HostFsError, path: &str) -> HostFolderBrowserError {
    match error.code() {
        OS_FS_NOT_FOUND | OS_FS_NOT_DIRECTORY => Box::new(HostFolderNotFoundError::new(path)),
        OS_FS_PERMISSION_DENIED => Box::new(HostFolderPermissionError::new(path)),
        _ => Box::new(error),
    }
}

// Original: compareBrowseEntries(). Rust's stable slice sort retains the
// source ordering when two names compare equal.
fn compare_browse_entries(left: &FsBrowseEntry, right: &FsBrowseEntry) -> Ordering {
    match (left.name.starts_with('.'), right.name.starts_with('.')) {
        (true, false) => Ordering::Greater,
        (false, true) => Ordering::Less,
        _ => left.name.cmp(&right.name),
    }
}

pub fn register_host_folder_browser() {
    register_scoped_service(
        LifecycleScope::App,
        FS_HOST_FOLDER_BROWSER_ID,
        SyncDescriptor::new(|accessor| {
            let registry = accessor.get(WORKSPACE_REGISTRY_SERVICE_ID)?;
            let host_fs = accessor.get(HOST_FILE_SYSTEM_SERVICE_ID)?;
            let bootstrap = accessor.get(BOOTSTRAP_SERVICE_ID)?;
            let service: Arc<dyn HostFolderBrowserContract> = Arc::new(HostFolderBrowser::new(
                Arc::clone(&registry.0),
                Arc::clone(&host_fs.0),
                bootstrap.os_home_dir().to_string_lossy(),
            ));
            Ok(HostFolderBrowserHandle(service))
        }),
        InstantiationType::Eager,
        "hostFolderBrowser",
    );
}

#[cfg(test)]
mod tests {
    use std::{
        path::PathBuf,
        sync::atomic::{AtomicU64, Ordering as AtomicOrdering},
        time::{SystemTime, UNIX_EPOCH},
    };

    use crate::{
        app::workspace_registry::{Workspace, WorkspaceRegistryResult, WorkspaceUpdate},
        os::backends::node_local::host_fs_service::HostFileSystem,
    };

    use super::*;

    struct StubRegistry {
        workspaces: Vec<Workspace>,
    }

    #[async_trait]
    impl WorkspaceRegistryContract for StubRegistry {
        async fn list(&self) -> WorkspaceRegistryResult<Vec<Workspace>> {
            Ok(self.workspaces.clone())
        }

        async fn get(&self, _: &str) -> WorkspaceRegistryResult<Option<Workspace>> {
            Ok(None)
        }

        async fn resolve_alias_ids(&self, _: &str) -> WorkspaceRegistryResult<Vec<String>> {
            Ok(Vec::new())
        }

        async fn create_or_touch(
            &self,
            _: &str,
            _: Option<&str>,
        ) -> WorkspaceRegistryResult<Workspace> {
            unreachable!("not used by HostFolderBrowser")
        }

        async fn update(
            &self,
            _: &str,
            _: WorkspaceUpdate,
        ) -> WorkspaceRegistryResult<Option<Workspace>> {
            unreachable!("not used by HostFolderBrowser")
        }

        async fn delete(&self, _: &str) -> WorkspaceRegistryResult<()> {
            unreachable!("not used by HostFolderBrowser")
        }
    }

    fn registry(workspaces: Vec<Workspace>) -> Arc<dyn WorkspaceRegistryContract> {
        Arc::new(StubRegistry { workspaces })
    }

    fn workspace(index: u64) -> Workspace {
        Workspace {
            id: format!("wd_{index}"),
            root: format!("/workspace/{index}"),
            name: index.to_string(),
            created_at_millis: index as i64,
            last_opened_at_millis: index as i64,
        }
    }

    fn temp_dir() -> PathBuf {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "kimi-host-folder-browser-{}-{nonce}-{}",
            std::process::id(),
            NEXT.fetch_add(1, AtomicOrdering::Relaxed)
        ))
    }

    fn service(home: &Path) -> HostFolderBrowser {
        HostFolderBrowser::new(
            registry(Vec::new()),
            Arc::new(HostFileSystem),
            home.to_string_lossy(),
        )
    }

    #[tokio::test]
    async fn browse_resolves_real_path_filters_files_and_sorts_dot_directories_last() {
        let root = temp_dir();
        tokio::fs::create_dir_all(root.join("beta")).await.unwrap();
        tokio::fs::create_dir(root.join(".zeta")).await.unwrap();
        tokio::fs::create_dir(root.join("alpha")).await.unwrap();
        tokio::fs::write(root.join("file.txt"), b"ignored")
            .await
            .unwrap();

        let response = service(&root).browse(None).await.unwrap();
        let canonical = tokio::fs::canonicalize(&root).await.unwrap();
        assert_eq!(response.path, canonical.to_string_lossy());
        assert_eq!(
            response
                .entries
                .iter()
                .map(|entry| entry.name.as_str())
                .collect::<Vec<_>>(),
            ["alpha", "beta", ".zeta"]
        );
        assert!(response.entries.iter().all(|entry| entry.is_dir));
        assert_eq!(
            response.entries[0].path,
            canonical.join("alpha").to_string_lossy()
        );

        tokio::fs::remove_dir_all(root).await.unwrap();
    }

    #[tokio::test]
    async fn browse_rejects_relative_and_maps_missing_paths() {
        let root = temp_dir();
        let browser = service(&root);

        let relative = browser.browse(Some("relative/path")).await.unwrap_err();
        assert!(
            relative
                .downcast_ref::<HostFolderNotAbsoluteError>()
                .is_some()
        );

        let missing_path = root.to_string_lossy().into_owned();
        let missing = browser.browse(Some(&missing_path)).await.unwrap_err();
        let missing = missing
            .downcast_ref::<HostFolderNotFoundError>()
            .expect("missing path is translated");
        assert_eq!(missing.path, missing_path);
    }

    #[tokio::test]
    async fn home_preserves_registry_order_and_limits_recent_roots() {
        let workspaces = (0..10).map(workspace).collect();
        let browser =
            HostFolderBrowser::new(registry(workspaces), Arc::new(HostFileSystem), "/home/test");

        let response = browser.home().await.unwrap();
        assert_eq!(response.home, "/home/test");
        assert_eq!(response.recent_roots.len(), RECENT_ROOTS_LIMIT);
        assert_eq!(response.recent_roots[0], "/workspace/0");
        assert_eq!(response.recent_roots[7], "/workspace/7");
    }
}
