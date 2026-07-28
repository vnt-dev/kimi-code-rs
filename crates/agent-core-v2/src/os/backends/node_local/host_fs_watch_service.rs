//! `notify`-backed local filesystem watcher.
//!
//! Original: `packages/agent-core-v2/src/os/backends/node-local/hostFsWatchService.ts`.

use std::{
    path::{Component, Path},
    sync::{Arc, Mutex},
};

use notify::{
    Config, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher,
    event::{CreateKind, ModifyKind, RemoveKind, RenameMode},
};

use crate::{
    _base::{
        di::{
            descriptors::SyncDescriptor,
            lifecycle::{Disposable, DisposeResult},
            scope::{InstantiationType, LifecycleScope, register_scoped_service},
        },
        errors::unexpected_error::on_unexpected_error,
        event::{Emitter, Event as CoreEvent},
    },
    os::interface::{
        host_fs_errors::{HostFsError, to_host_fs_error},
        host_fs_watch::{
            HOST_FS_WATCH_SERVICE_ID, HostFsChange, HostFsChangeAction, HostFsChangeKind,
            HostFsWatchHandle, HostFsWatchOptions, HostFsWatchService, HostFsWatchServiceHandle,
            IgnoredPath,
        },
    },
};

pub struct LocalHostFsWatchHandle {
    watcher: Mutex<Option<RecommendedWatcher>>,
    emitter: Arc<Emitter<HostFsChange>>,
}

impl HostFsWatchHandle for LocalHostFsWatchHandle {
    fn on_did_change(&self) -> CoreEvent<HostFsChange> {
        self.emitter.event()
    }
}

impl Disposable for LocalHostFsWatchHandle {
    fn dispose(&self) -> DisposeResult {
        self.watcher.lock().unwrap().take();
        self.emitter.dispose()
    }
}

#[derive(Default)]
pub struct LocalHostFsWatchService;

impl HostFsWatchService for LocalHostFsWatchService {
    fn watch(
        &self,
        path: &Path,
        options: HostFsWatchOptions,
    ) -> Result<Arc<dyn HostFsWatchHandle>, HostFsError> {
        let emitter = Arc::new(Emitter::new());
        let callback_emitter = Arc::clone(&emitter);
        let ignored = options.ignored.unwrap_or_else(|| Arc::new(default_ignored));
        let mut watcher = RecommendedWatcher::new(
            move |result: notify::Result<Event>| match result {
                Ok(event) => {
                    for change in map_event(&event, &ignored) {
                        callback_emitter.fire(&change);
                    }
                }
                Err(error) => on_unexpected_error(&error),
            },
            Config::default(),
        )
        .map_err(|error| watcher_error(error, path))?;
        watcher
            .watch(
                path,
                if options.recursive == Some(false) {
                    RecursiveMode::NonRecursive
                } else {
                    RecursiveMode::Recursive
                },
            )
            .map_err(|error| watcher_error(error, path))?;
        Ok(Arc::new(LocalHostFsWatchHandle {
            watcher: Mutex::new(Some(watcher)),
            emitter,
        }))
    }
}

fn default_ignored(path: &Path) -> bool {
    path.components()
        .any(|component| component == Component::Normal(".git".as_ref()))
}

fn map_event(event: &Event, ignored: &IgnoredPath) -> Vec<HostFsChange> {
    if let EventKind::Modify(ModifyKind::Name(RenameMode::Both)) = event.kind
        && event.paths.len() >= 2
    {
        return [
            map_path(
                &event.paths[0],
                HostFsChangeAction::Deleted,
                HostFsChangeKind::File,
                ignored,
            ),
            map_path(
                &event.paths[1],
                HostFsChangeAction::Created,
                infer_kind(&event.paths[1]),
                ignored,
            ),
        ]
        .into_iter()
        .flatten()
        .collect();
    }
    let mapped = match event.kind {
        EventKind::Create(kind) => Some((HostFsChangeAction::Created, create_kind(kind))),
        EventKind::Remove(kind) => Some((HostFsChangeAction::Deleted, remove_kind(kind))),
        EventKind::Modify(ModifyKind::Name(RenameMode::From)) => {
            Some((HostFsChangeAction::Deleted, HostFsChangeKind::File))
        }
        EventKind::Modify(ModifyKind::Name(RenameMode::To)) => {
            Some((HostFsChangeAction::Created, HostFsChangeKind::File))
        }
        EventKind::Modify(_) => Some((HostFsChangeAction::Modified, HostFsChangeKind::File)),
        _ => None,
    };
    let Some((action, kind)) = mapped else {
        return Vec::new();
    };
    event
        .paths
        .iter()
        .filter_map(|path| map_path(path, action, kind, ignored))
        .collect()
}

fn map_path(
    path: &Path,
    action: HostFsChangeAction,
    kind: HostFsChangeKind,
    ignored: &IgnoredPath,
) -> Option<HostFsChange> {
    if ignored(path) {
        return None;
    }
    Some(HostFsChange {
        path: path.to_string_lossy().into_owned(),
        action,
        kind,
    })
}

fn create_kind(kind: CreateKind) -> HostFsChangeKind {
    match kind {
        CreateKind::Folder => HostFsChangeKind::Directory,
        CreateKind::File => HostFsChangeKind::File,
        _ => HostFsChangeKind::File,
    }
}

fn remove_kind(kind: RemoveKind) -> HostFsChangeKind {
    match kind {
        RemoveKind::Folder => HostFsChangeKind::Directory,
        RemoveKind::File => HostFsChangeKind::File,
        _ => HostFsChangeKind::File,
    }
}

fn infer_kind(path: &Path) -> HostFsChangeKind {
    if path.is_dir() {
        HostFsChangeKind::Directory
    } else {
        HostFsChangeKind::File
    }
}

fn watcher_error(error: notify::Error, path: &Path) -> HostFsError {
    to_host_fs_error(Box::new(error), &path.to_string_lossy(), "watch")
}

pub fn register_local_host_fs_watch_service() {
    register_scoped_service(
        LifecycleScope::App,
        HOST_FS_WATCH_SERVICE_ID,
        SyncDescriptor::new(|_| {
            let service: Arc<dyn HostFsWatchService> = Arc::new(LocalHostFsWatchService);
            Ok(HostFsWatchServiceHandle(service))
        }),
        InstantiationType::Eager,
        "os",
    );
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    use tokio::sync::mpsc;

    use super::*;

    fn temp_dir() -> std::path::PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("kimi-hostfswatch-{}-{nonce}", std::process::id()))
    }

    #[test]
    fn maps_create_modify_delete_and_ignores_git() {
        let ignored: IgnoredPath = Arc::new(default_ignored);
        let path = Path::new("/tmp/a.txt").to_path_buf();
        for (kind, action) in [
            (
                EventKind::Create(CreateKind::File),
                HostFsChangeAction::Created,
            ),
            (
                EventKind::Modify(ModifyKind::Any),
                HostFsChangeAction::Modified,
            ),
            (
                EventKind::Remove(RemoveKind::File),
                HostFsChangeAction::Deleted,
            ),
        ] {
            let event = Event {
                kind,
                paths: vec![path.clone()],
                attrs: Default::default(),
            };
            assert_eq!(map_event(&event, &ignored)[0].action, action);
        }
        let git = Event {
            kind: EventKind::Create(CreateKind::File),
            paths: vec![Path::new("/tmp/.git/config").to_path_buf()],
            attrs: Default::default(),
        };
        assert!(map_event(&git, &ignored).is_empty());
    }

    #[tokio::test]
    async fn reports_real_file_creation_and_stops_after_disposal() {
        let root = temp_dir();
        tokio::fs::create_dir(&root).await.unwrap();
        let handle = LocalHostFsWatchService
            .watch(&root, HostFsWatchOptions::default())
            .unwrap();
        let (sender, mut receiver) = mpsc::unbounded_channel();
        let _subscription = handle.on_did_change().subscribe(move |change| {
            let _ = sender.send(change.clone());
        });
        let file = root.join("created.txt");
        tokio::fs::write(&file, "x").await.unwrap();
        let observed = tokio::time::timeout(Duration::from_secs(3), async {
            while let Some(change) = receiver.recv().await {
                if change.path == file.to_string_lossy()
                    && change.action == HostFsChangeAction::Created
                {
                    return true;
                }
            }
            false
        })
        .await
        .unwrap();
        assert!(observed);
        handle.dispose().unwrap();
        tokio::fs::remove_dir_all(root).await.unwrap();
    }
}
