//! Session filesystem watch service.
//!
//! Original: `packages/agent-core-v2/src/session/sessionFs/fsWatchService.ts`.

use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

use async_trait::async_trait;
use indexmap::IndexSet;
use serde_json::{Map, Value};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;

use crate::{
    _base::{
        di::{
            descriptors::SyncDescriptor,
            instantiation::ServicesAccessorExt,
            lifecycle::{Disposable, DisposableHandle, DisposeResult},
            scope::{InstantiationType, LifecycleScope, register_scoped_service},
        },
        errors::errors::{Error2, Error2Options},
        event::{Emitter, Event},
    },
    os::interface::{
        host_file_system::{HOST_FILE_SYSTEM_SERVICE_ID, HostFileSystemServiceHandle},
        host_fs_watch::{
            HOST_FS_WATCH_SERVICE_ID, HostFsChange, HostFsChangeAction, HostFsChangeKind,
            HostFsWatchHandle, HostFsWatchOptions, HostFsWatchServiceHandle,
        },
    },
    session::workspace_context::{SESSION_WORKSPACE_CONTEXT_ID, SessionWorkspaceContextHandle},
};

use super::{
    FS_PATH_ESCAPES, FsChangeAction, FsChangeEntry, FsChangeEvent, FsChangeKind, GitignoreMatcher,
    SESSION_FS_WATCH_SERVICE_ID, SessionFsWatchError, SessionFsWatchServiceContract,
    SessionFsWatchServiceHandle, ensure_fs_errors_registered,
};

const DEFAULT_DEBOUNCE_MS: u64 = 200;
const DEFAULT_MAX_CHANGES_PER_WINDOW: usize = 500;

enum WatchCommand {
    Raw(HostFsChange),
    Clear,
    Flush(oneshot::Sender<()>),
}

#[derive(Default)]
struct WatchWindow {
    pending: Vec<FsChangeEntry>,
    raw_count: usize,
    truncated: bool,
    deadline: Option<tokio::time::Instant>,
}

struct WatchState {
    watched: IndexSet<String>,
    handle: Option<Arc<dyn HostFsWatchHandle>>,
    handle_subscription: Option<DisposableHandle>,
}

impl Default for WatchState {
    fn default() -> Self {
        Self {
            watched: IndexSet::new(),
            handle: None,
            handle_subscription: None,
        }
    }
}

struct WatchInner {
    workspace: SessionWorkspaceContextHandle,
    host_watch: HostFsWatchServiceHandle,
    host_fs: HostFileSystemServiceHandle,
    emitter: Emitter<FsChangeEvent>,
    state: Mutex<WatchState>,
    commands: mpsc::UnboundedSender<WatchCommand>,
    worker_task: Mutex<Option<JoinHandle<()>>>,
    debounce_ms: u64,
    max_changes_per_window: usize,
}

pub struct SessionFsWatchService {
    inner: Arc<WatchInner>,
}

impl SessionFsWatchService {
    pub fn new(
        workspace: SessionWorkspaceContextHandle,
        host_watch: HostFsWatchServiceHandle,
        host_fs: HostFileSystemServiceHandle,
    ) -> Self {
        ensure_fs_errors_registered();
        let (commands, receiver) = mpsc::unbounded_channel();
        let inner = Arc::new(WatchInner {
            workspace,
            host_watch,
            host_fs,
            emitter: Emitter::new(),
            state: Mutex::new(WatchState::default()),
            commands,
            worker_task: Mutex::new(None),
            debounce_ms: read_positive_int_env(
                "KIMI_CODE_FS_WATCH_DEBOUNCE_MS",
                DEFAULT_DEBOUNCE_MS,
            ),
            max_changes_per_window: read_positive_int_env(
                "KIMI_CODE_FS_WATCH_MAX_CHANGES_PER_WINDOW",
                DEFAULT_MAX_CHANGES_PER_WINDOW as u64,
            ) as usize,
        });
        let worker = tokio::spawn(run_watch_worker(Arc::downgrade(&inner), receiver));
        *inner.worker_task.lock().unwrap() = Some(worker);
        Self { inner }
    }

    fn ensure_handle(&self) -> Result<(), SessionFsWatchError> {
        if self.inner.state.lock().unwrap().handle.is_some() {
            return Ok(());
        }
        let work_dir = self.inner.workspace.work_dir();
        let handle = self.inner.host_watch.watch(
            &work_dir,
            HostFsWatchOptions {
                recursive: Some(true),
                ignored: None,
            },
        )?;
        let commands = self.inner.commands.clone();
        let subscription = handle.on_did_change().subscribe(move |change| {
            let _ = commands.send(WatchCommand::Raw(change.clone()));
        });
        let mut state = self.inner.state.lock().unwrap();
        if state.handle.is_none() {
            state.handle = Some(handle);
            state.handle_subscription = Some(subscription);
        } else {
            let _ = subscription.dispose();
            let _ = handle.dispose();
        }
        Ok(())
    }

    fn teardown_handle(&self) {
        let (subscription, handle) = {
            let mut state = self.inner.state.lock().unwrap();
            (state.handle_subscription.take(), state.handle.take())
        };
        if let Some(subscription) = subscription {
            let _ = subscription.dispose();
        }
        if let Some(handle) = handle {
            let _ = handle.dispose();
        }
    }

    fn clear_window(&self) {
        let _ = self.inner.commands.send(WatchCommand::Clear);
    }

    fn resolve_within(&self, input: &str) -> Result<PathBuf, SessionFsWatchError> {
        if input.is_empty() || input == "/" {
            return Err(path_escape_error(input, "empty", "rejected (empty)"));
        }
        if Path::new(input).is_absolute() {
            return Err(path_escape_error(input, "absolute", "rejected (absolute)"));
        }
        if split_segments(input).any(|segment| segment == "..") {
            return Err(path_escape_error(
                input,
                "dotdot_segment",
                "rejected (dotdot segment)",
            ));
        }
        let absolute = self.inner.workspace.resolve(input);
        if !self.inner.workspace.is_within(&absolute.to_string_lossy()) {
            return Err(path_escape_error(
                input,
                "resolved_outside",
                "escapes workspace",
            ));
        }
        Ok(absolute)
    }
}

#[async_trait]
impl SessionFsWatchServiceContract for SessionFsWatchService {
    fn set_watched_paths(&self, paths: &[String]) -> Result<(), SessionFsWatchError> {
        let mut watched = IndexSet::new();
        for path in paths {
            let absolute = self.resolve_within(path)?;
            watched.insert(to_relative(&self.inner.workspace.work_dir(), &absolute));
        }
        let empty = watched.is_empty();
        self.inner.state.lock().unwrap().watched = watched;
        if empty {
            self.teardown_handle();
            self.clear_window();
        } else {
            self.ensure_handle()?;
        }
        Ok(())
    }

    fn watched_paths(&self) -> Vec<String> {
        self.inner
            .state
            .lock()
            .unwrap()
            .watched
            .iter()
            .cloned()
            .collect()
    }

    fn on_did_change_files(&self) -> Event<FsChangeEvent> {
        self.inner.emitter.event()
    }

    async fn flush_pending(&self) {
        let (sender, receiver) = oneshot::channel();
        if self
            .inner
            .commands
            .send(WatchCommand::Flush(sender))
            .is_ok()
        {
            let _ = receiver.await;
        }
    }
}

impl Disposable for SessionFsWatchService {
    fn dispose(&self) -> DisposeResult {
        self.clear_window();
        self.teardown_handle();
        if let Some(worker) = self.inner.worker_task.lock().unwrap().take() {
            worker.abort();
        }
        self.inner.emitter.dispose()
    }
}

async fn run_watch_worker(
    inner: std::sync::Weak<WatchInner>,
    mut commands: mpsc::UnboundedReceiver<WatchCommand>,
) {
    let Some(service) = inner.upgrade() else {
        return;
    };
    let mut matcher = GitignoreMatcher::new();
    matcher.add(".git/");
    let gitignore_path = service.workspace.work_dir().join(".gitignore");
    if let Ok(contents) = service.host_fs.read_text(&gitignore_path, None).await {
        matcher.add(&contents);
    }
    drop(service);

    let mut window = WatchWindow::default();
    loop {
        let command = if let Some(deadline) = window.deadline {
            tokio::select! {
                command = commands.recv() => command,
                _ = tokio::time::sleep_until(deadline) => {
                    if let Some(service) = inner.upgrade() {
                        flush_window(&service, &mut window);
                        continue;
                    }
                    return;
                }
            }
        } else {
            commands.recv().await
        };
        let Some(command) = command else {
            return;
        };
        let Some(service) = inner.upgrade() else {
            return;
        };
        match command {
            WatchCommand::Raw(event) => process_raw(&service, &matcher, &mut window, &event),
            WatchCommand::Clear => clear_window(&mut window),
            WatchCommand::Flush(sender) => {
                flush_window(&service, &mut window);
                let _ = sender.send(());
            }
        }
    }
}

fn process_raw(
    inner: &WatchInner,
    matcher: &GitignoreMatcher,
    window: &mut WatchWindow,
    event: &HostFsChange,
) {
    let relative = to_relative(&inner.workspace.work_dir(), Path::new(&event.path));
    if relative == "." {
        return;
    }
    let probe = if event.kind == HostFsChangeKind::Directory {
        format!("{relative}/")
    } else {
        relative.clone()
    };
    if matcher.ignores(&probe) {
        return;
    }

    if !is_under_any(&relative, &inner.state.lock().unwrap().watched) {
        return;
    }
    window.pending.push(FsChangeEntry {
        path: relative,
        change: match event.action {
            HostFsChangeAction::Created => FsChangeAction::Created,
            HostFsChangeAction::Modified => FsChangeAction::Modified,
            HostFsChangeAction::Deleted => FsChangeAction::Deleted,
        },
        kind: match event.kind {
            HostFsChangeKind::File => FsChangeKind::File,
            HostFsChangeKind::Directory => FsChangeKind::Directory,
        },
        size_delta: None,
        etag: None,
    });
    window.raw_count += 1;
    if window.pending.len() > inner.max_changes_per_window {
        window.truncated = true;
        window.pending.clear();
    }
    if window.deadline.is_none() {
        window.deadline =
            Some(tokio::time::Instant::now() + Duration::from_millis(inner.debounce_ms));
    }
}

fn clear_window(window: &mut WatchWindow) {
    window.pending.clear();
    window.raw_count = 0;
    window.truncated = false;
    window.deadline = None;
}

fn flush_window(inner: &WatchInner, window: &mut WatchWindow) {
    window.deadline = None;
    if window.raw_count == 0 {
        return;
    }
    let truncated = window.truncated;
    let count = window.raw_count;
    let changes = if truncated {
        window.pending.clear();
        Vec::new()
    } else {
        std::mem::take(&mut window.pending)
    };
    window.raw_count = 0;
    window.truncated = false;
    let event = FsChangeEvent {
        changes,
        coalesced_window_ms: inner.debounce_ms,
        truncated: truncated.then_some(true),
        count: truncated.then_some(count),
    };
    inner.emitter.fire(&event);
}

fn is_under_any(relative: &str, parents: &IndexSet<String>) -> bool {
    parents.iter().any(|parent| {
        parent.is_empty()
            || parent == "."
            || relative == parent
            || relative.starts_with(&format!("{parent}/"))
    })
}

fn to_relative(root: &Path, absolute: &Path) -> String {
    if absolute == root {
        return ".".into();
    }
    absolute
        .strip_prefix(root)
        .map(|path| normalize_separators(&path.to_string_lossy()))
        .unwrap_or_else(|_| absolute.to_string_lossy().into_owned())
}

fn normalize_separators(path: &str) -> String {
    path.replace('\\', "/")
}

fn split_segments(path: &str) -> impl Iterator<Item = &str> {
    path.split(['/', '\\'])
}

fn path_escape_error(path: &str, reason: &str, suffix: &str) -> SessionFsWatchError {
    Box::new(Error2::with_options(
        FS_PATH_ESCAPES,
        format!("path \"{path}\" {suffix}"),
        Error2Options {
            details: Some(Map::from_iter([
                ("path".into(), Value::String(path.into())),
                ("reason".into(), Value::String(reason.into())),
            ])),
            ..Error2Options::default()
        },
    ))
}

fn read_positive_int_env(name: &str, fallback: u64) -> u64 {
    let Ok(raw) = std::env::var(name) else {
        return fallback;
    };
    let raw = raw.trim_start();
    let digits = raw
        .chars()
        .take_while(|character| character.is_ascii_digit())
        .collect::<String>();
    digits
        .parse::<u64>()
        .ok()
        .filter(|value| *value > 0)
        .unwrap_or(fallback)
}

pub fn register_session_fs_watch_service() {
    register_scoped_service(
        LifecycleScope::Session,
        SESSION_FS_WATCH_SERVICE_ID,
        SyncDescriptor::new(|accessor| {
            let workspace = accessor.get(SESSION_WORKSPACE_CONTEXT_ID)?;
            let host_watch = accessor.get(HOST_FS_WATCH_SERVICE_ID)?;
            let host_fs = accessor.get(HOST_FILE_SYSTEM_SERVICE_ID)?;
            let service: Arc<dyn SessionFsWatchServiceContract> =
                Arc::new(SessionFsWatchService::new(
                    (*workspace).clone(),
                    (*host_watch).clone(),
                    (*host_fs).clone(),
                ));
            Ok(SessionFsWatchServiceHandle(service))
        })
        .disposable(),
        InstantiationType::Eager,
        "sessionFsWatch",
    );
}

#[cfg(test)]
mod tests {
    use std::{
        path::PathBuf,
        sync::{
            Arc, Mutex,
            atomic::{AtomicBool, Ordering},
        },
        time::{SystemTime, UNIX_EPOCH},
    };

    use crate::{
        os::{
            backends::node_local::host_fs_service::HostFileSystem,
            interface::{
                host_fs_errors::HostFsError,
                host_fs_watch::{HostFsWatchService, HostFsWatchServiceHandle},
            },
        },
        session::workspace_context::{
            PathAccessError, PathAccessOperation, SessionWorkspaceContextContract,
        },
    };

    use super::*;

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> Self {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "kimi-session-fs-watch-{}-{nonce}",
                std::process::id()
            ));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    struct Workspace(PathBuf);

    impl SessionWorkspaceContextContract for Workspace {
        fn work_dir(&self) -> PathBuf {
            self.0.clone()
        }
        fn additional_dirs(&self) -> Vec<PathBuf> {
            Vec::new()
        }
        fn set_work_dir(&self, _: &str) -> std::io::Result<()> {
            Ok(())
        }
        fn set_additional_dirs(&self, _: &[String]) -> std::io::Result<()> {
            Ok(())
        }
        fn resolve(&self, relative: &str) -> PathBuf {
            if Path::new(relative).is_absolute() {
                PathBuf::from(relative)
            } else {
                self.0.join(relative)
            }
        }
        fn is_within(&self, absolute_path: &str) -> bool {
            Path::new(absolute_path).strip_prefix(&self.0).is_ok()
        }
        fn assert_allowed(
            &self,
            absolute_path: &str,
            operation: PathAccessOperation,
        ) -> Result<PathBuf, PathAccessError> {
            let path = self.resolve(absolute_path);
            if self.is_within(&path.to_string_lossy()) {
                Ok(path)
            } else {
                Err(PathAccessError { path, operation })
            }
        }
        fn add_additional_dir(&self, _: &str) -> std::io::Result<()> {
            Ok(())
        }
        fn remove_additional_dir(&self, _: &str) -> std::io::Result<()> {
            Ok(())
        }
    }

    struct WatchHandle {
        emitter: Emitter<HostFsChange>,
        disposed: AtomicBool,
    }

    impl HostFsWatchHandle for WatchHandle {
        fn on_did_change(&self) -> Event<HostFsChange> {
            self.emitter.event()
        }
    }

    impl Disposable for WatchHandle {
        fn dispose(&self) -> DisposeResult {
            self.disposed.store(true, Ordering::Release);
            self.emitter.dispose()
        }
    }

    struct WatchService {
        handle: Arc<WatchHandle>,
        calls: Mutex<Vec<PathBuf>>,
    }

    impl HostFsWatchService for WatchService {
        fn watch(
            &self,
            path: &Path,
            _: HostFsWatchOptions,
        ) -> Result<Arc<dyn HostFsWatchHandle>, HostFsError> {
            self.calls.lock().unwrap().push(path.to_path_buf());
            Ok(self.handle.clone())
        }
    }

    fn service(root: &Path) -> (SessionFsWatchService, Arc<WatchService>) {
        let watcher = Arc::new(WatchService {
            handle: Arc::new(WatchHandle {
                emitter: Emitter::new(),
                disposed: AtomicBool::new(false),
            }),
            calls: Mutex::new(Vec::new()),
        });
        (
            SessionFsWatchService::new(
                SessionWorkspaceContextHandle(Arc::new(Workspace(root.to_path_buf()))),
                HostFsWatchServiceHandle(watcher.clone()),
                HostFileSystemServiceHandle(Arc::new(HostFileSystem)),
            ),
            watcher,
        )
    }

    #[tokio::test(start_paused = true)]
    async fn starts_filters_coalesces_truncates_and_disposes_like_source() {
        let directory = TestDirectory::new();
        let (service, watcher) = service(&directory.0);
        let events = Arc::new(Mutex::new(Vec::new()));
        let captured = Arc::clone(&events);
        service
            .on_did_change_files()
            .subscribe(move |event| captured.lock().unwrap().push(event.clone()));

        service.set_watched_paths(&["src".into()]).unwrap();
        assert_eq!(
            watcher.calls.lock().unwrap().as_slice(),
            std::slice::from_ref(&directory.0)
        );
        assert_eq!(service.watched_paths(), ["src"]);
        let callback_handle = Arc::clone(&watcher.handle);
        let callback_path = directory.0.join("src/a.ts").to_string_lossy().into_owned();
        std::thread::spawn(move || {
            callback_handle.emitter.fire(&HostFsChange {
                path: callback_path,
                action: HostFsChangeAction::Created,
                kind: HostFsChangeKind::File,
            });
        })
        .join()
        .unwrap();
        watcher.handle.emitter.fire(&HostFsChange {
            path: directory.0.join("lib/b.ts").to_string_lossy().into_owned(),
            action: HostFsChangeAction::Created,
            kind: HostFsChangeKind::File,
        });
        service.flush_pending().await;
        assert_eq!(events.lock().unwrap()[0].changes.len(), 1);
        assert_eq!(events.lock().unwrap()[0].changes[0].path, "src/a.ts");

        service.set_watched_paths(&[]).unwrap();
        assert!(watcher.handle.disposed.load(Ordering::Acquire));
    }

    #[tokio::test(start_paused = true)]
    async fn gitignore_and_overflow_emit_source_shapes() {
        let directory = TestDirectory::new();
        std::fs::write(directory.0.join(".gitignore"), "dist/\n").unwrap();
        let (service, watcher) = service(&directory.0);
        let events = Arc::new(Mutex::new(Vec::new()));
        let captured = Arc::clone(&events);
        service
            .on_did_change_files()
            .subscribe(move |event| captured.lock().unwrap().push(event.clone()));
        service.set_watched_paths(&[".".into()]).unwrap();
        // Raw changes may arrive while `.gitignore` is still loading. They stay
        // queued behind that async initialization and are filtered afterward.
        watcher.handle.emitter.fire(&HostFsChange {
            path: directory.0.join("dist/x.js").to_string_lossy().into_owned(),
            action: HostFsChangeAction::Created,
            kind: HostFsChangeKind::File,
        });
        for index in 0..501 {
            watcher.handle.emitter.fire(&HostFsChange {
                path: directory
                    .0
                    .join(format!("src/f{index}.ts"))
                    .to_string_lossy()
                    .into_owned(),
                action: HostFsChangeAction::Created,
                kind: HostFsChangeKind::File,
            });
        }
        service.flush_pending().await;
        let events = events.lock().unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].truncated, Some(true));
        assert_eq!(events[0].count, Some(501));
        assert!(events[0].changes.is_empty());
    }

    #[tokio::test]
    async fn rejects_escaping_watch_paths() {
        let directory = TestDirectory::new();
        let (service, _) = service(&directory.0);
        assert!(service.set_watched_paths(&["../x".into()]).is_err());
        assert!(
            service
                .set_watched_paths(&[directory.0.to_string_lossy().into_owned()])
                .is_err()
        );
    }
}
