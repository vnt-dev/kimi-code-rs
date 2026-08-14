//! Session terminal records, frame replay, and PTY lifecycle.
//!
//! Original: `session/terminal/terminalService.ts`, `SessionTerminalService`.

use parking_lot::Mutex;
use std::ops::Deref;
use std::sync::Arc;

use async_trait::async_trait;
use indexmap::IndexMap;
use serde_json::Value;
use uuid::Uuid;

use crate::{
    _base::{
        di::{
            descriptors::SyncDescriptor,
            instantiation::{ServiceIdentifier, ServicesAccessorExt},
            lifecycle::{Disposable, DisposableHandle, DisposeResult, dispose_all},
            scope::{InstantiationType, LifecycleScope, register_scoped_service},
        },
        errors::errors::Error2,
        utils::iso_date_time::now_iso_date_time,
    },
    os::interface::terminal::{
        CreateTerminalRequest, HOST_TERMINAL_SERVICE_ID, HostTerminalService, TERMINAL_NOT_FOUND,
        Terminal, TerminalAttachOptions, TerminalAttachSink, TerminalExitPayload, TerminalFrame,
        TerminalOutputPayload, TerminalProcess, TerminalProcessError, TerminalSpawnOptions,
        TerminalStatus,
    },
    session::{
        session_context::{SESSION_CONTEXT_ID, SessionContext},
        workspace_context::{
            PathAccessError, PathAccessOperation, SESSION_WORKSPACE_CONTEXT_ID,
            SessionWorkspaceContextContract,
        },
    },
};

const DEFAULT_COLS: u32 = 80;
const DEFAULT_ROWS: u32 = 24;
const DEFAULT_MAX_BUFFERED_FRAMES: usize = 2_000;

pub type SessionTerminalResult<T> = Result<T, SessionTerminalError>;

#[derive(Debug, thiserror::Error)]
pub enum SessionTerminalError {
    #[error(transparent)]
    NotFound(Box<Error2>),
    #[error(transparent)]
    Path(#[from] PathAccessError),
    #[error(transparent)]
    Process(#[from] TerminalProcessError),
}

#[async_trait]
pub trait SessionTerminalServiceContract: Send + Sync {
    async fn create(&self, input: CreateTerminalRequest) -> SessionTerminalResult<Terminal>;
    async fn list(&self) -> SessionTerminalResult<Vec<Terminal>>;
    async fn get(&self, terminal_id: String) -> SessionTerminalResult<Terminal>;
    async fn attach(
        &self,
        terminal_id: String,
        sink: Arc<dyn TerminalAttachSink>,
        options: TerminalAttachOptions,
    ) -> SessionTerminalResult<AttachResult>;
    fn detach(&self, terminal_id: &str, sink_id: &str);
    fn detach_all_for_sink(&self, sink_id: &str);
    async fn write(&self, terminal_id: String, data: String) -> SessionTerminalResult<()>;
    async fn resize(&self, terminal_id: String, cols: u32, rows: u32) -> SessionTerminalResult<()>;
    async fn close(&self, terminal_id: String) -> SessionTerminalResult<CloseResult>;
}

#[derive(Clone)]
pub struct SessionTerminalServiceHandle(pub Arc<dyn SessionTerminalServiceContract>);

impl Deref for SessionTerminalServiceHandle {
    type Target = dyn SessionTerminalServiceContract;

    fn deref(&self) -> &Self::Target {
        self.0.as_ref()
    }
}

pub const SESSION_TERMINAL_SERVICE_ID: ServiceIdentifier<SessionTerminalServiceHandle> =
    ServiceIdentifier::new("sessionTerminalService");

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AttachResult {
    pub replayed: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CloseResult {
    pub closed: bool,
}

struct TerminalRecord {
    terminal: Terminal,
    process: Arc<dyn TerminalProcess>,
    sinks: IndexMap<String, Arc<dyn TerminalAttachSink>>,
    buffer: Vec<TerminalFrame>,
    next_seq: u64,
    disposables: Vec<DisposableHandle>,
    closed: bool,
}

#[derive(Default)]
struct TerminalState {
    records: IndexMap<String, TerminalRecord>,
}

/// Rust adaptation: the source's single-threaded `Map` is guarded because
/// portable-pty emits data and exit events from dedicated threads.  Listener
/// snapshots are sent after releasing the lock, preserving source map order
/// without allowing a sink callback to deadlock the service.
pub struct SessionTerminalService {
    host: Arc<dyn HostTerminalService>,
    workspace: Arc<dyn SessionWorkspaceContextContract>,
    context: SessionContext,
    state: Arc<Mutex<TerminalState>>,
}

impl SessionTerminalService {
    pub fn new(
        host: Arc<dyn HostTerminalService>,
        workspace: Arc<dyn SessionWorkspaceContextContract>,
        context: SessionContext,
    ) -> Self {
        Self {
            host,
            workspace,
            context,
            state: Arc::new(Mutex::new(TerminalState::default())),
        }
    }

    pub async fn create(&self, input: CreateTerminalRequest) -> SessionTerminalResult<Terminal> {
        let cwd = match input.cwd.as_deref() {
            Some(cwd) => self
                .workspace
                .assert_allowed(cwd, PathAccessOperation::Execute)?,
            None => self.workspace.work_dir(),
        };
        let shell = input.shell.unwrap_or_else(default_shell);
        let cols = input.cols.unwrap_or(DEFAULT_COLS);
        let rows = input.rows.unwrap_or(DEFAULT_ROWS);
        let process = self
            .host
            .spawn(TerminalSpawnOptions {
                cwd: cwd.to_string_lossy().into_owned(),
                shell: shell.clone(),
                cols,
                rows,
            })
            .await?;
        let terminal = Terminal {
            id: format!("term_{}", Uuid::new_v4()),
            session_id: self.context.session_id.clone(),
            cwd: cwd.to_string_lossy().into_owned(),
            shell,
            cols,
            rows,
            status: TerminalStatus::Running,
            created_at: now_iso_date_time(),
            exited_at: None,
            exit_code: None,
        };
        let terminal_id = terminal.id.clone();
        self.state.lock().records.insert(
            terminal_id.clone(),
            TerminalRecord {
                terminal: terminal.clone(),
                process: Arc::clone(&process),
                sinks: IndexMap::new(),
                buffer: Vec::new(),
                next_seq: 0,
                disposables: Vec::new(),
                closed: false,
            },
        );
        let state = Arc::downgrade(&self.state);
        let data_id = terminal_id.clone();
        let data_subscription = process.on_process_data().subscribe(move |data| {
            if let Some(state) = state.upgrade() {
                on_data(&state, &data_id, data);
            }
        });
        let state = Arc::downgrade(&self.state);
        let exit_subscription = process.on_process_exit().subscribe(move |event| {
            if let Some(state) = state.upgrade() {
                mark_exited(&state, &terminal_id, event.exit_code);
            }
        });
        let mut state = self.state.lock();
        if let Some(record) = state.records.get_mut(&terminal.id) {
            if record.terminal.status == TerminalStatus::Exited {
                let _ = data_subscription.dispose();
                let _ = exit_subscription.dispose();
            } else {
                record
                    .disposables
                    .extend([data_subscription, exit_subscription]);
            }
        }
        Ok(terminal)
    }

    pub async fn list(&self) -> SessionTerminalResult<Vec<Terminal>> {
        Ok(self
            .state
            .lock()
            .records
            .values()
            .map(|record| record.terminal.clone())
            .collect())
    }

    pub async fn get(&self, terminal_id: &str) -> SessionTerminalResult<Terminal> {
        let mut state = self.state.lock();
        Ok(
            require_record(&mut state, terminal_id, &self.context.session_id)?
                .terminal
                .clone(),
        )
    }

    pub async fn attach(
        &self,
        terminal_id: &str,
        sink: Arc<dyn TerminalAttachSink>,
        options: TerminalAttachOptions,
    ) -> SessionTerminalResult<AttachResult> {
        let replay = {
            let mut state = self.state.lock();
            let record = require_record(&mut state, terminal_id, &self.context.session_id)?;
            record.sinks.insert(sink.id().to_owned(), Arc::clone(&sink));
            let since_seq = options.since_seq.unwrap_or(0);
            record
                .buffer
                .iter()
                .filter(|frame| frame_seq(frame) > since_seq)
                .cloned()
                .collect::<Vec<_>>()
        };
        for frame in &replay {
            sink.send(frame.clone());
        }
        Ok(AttachResult {
            replayed: replay.len(),
        })
    }

    pub fn detach(&self, terminal_id: &str, sink_id: &str) {
        if let Some(record) = self.state.lock().records.get_mut(terminal_id) {
            record.sinks.shift_remove(sink_id);
        }
    }

    pub fn detach_all_for_sink(&self, sink_id: &str) {
        for record in self.state.lock().records.values_mut() {
            record.sinks.shift_remove(sink_id);
        }
    }

    pub async fn write(&self, terminal_id: &str, data: &str) -> SessionTerminalResult<()> {
        let process = {
            let mut state = self.state.lock();
            Arc::clone(&require_record(&mut state, terminal_id, &self.context.session_id)?.process)
        };
        process.write(data)?;
        Ok(())
    }

    pub async fn resize(
        &self,
        terminal_id: &str,
        cols: u32,
        rows: u32,
    ) -> SessionTerminalResult<()> {
        let process = {
            let mut state = self.state.lock();
            let record = require_record(&mut state, terminal_id, &self.context.session_id)?;
            record.terminal.cols = cols;
            record.terminal.rows = rows;
            Arc::clone(&record.process)
        };
        process.resize(cols, rows)?;
        Ok(())
    }

    pub async fn close(&self, terminal_id: &str) -> SessionTerminalResult<CloseResult> {
        let process = {
            let mut state = self.state.lock();
            let record = require_record(&mut state, terminal_id, &self.context.session_id)?;
            if record.closed {
                None
            } else {
                record.closed = true;
                Some(Arc::clone(&record.process))
            }
        };
        if let Some(process) = process {
            process.kill()?;
            mark_exited(&self.state, terminal_id, None);
        }
        Ok(CloseResult { closed: true })
    }
}

fn require_record<'a>(
    state: &'a mut TerminalState,
    terminal_id: &str,
    session_id: &str,
) -> SessionTerminalResult<&'a mut TerminalRecord> {
    state.records.get_mut(terminal_id).ok_or_else(|| {
        SessionTerminalError::NotFound(Box::new(Error2::new(
            TERMINAL_NOT_FOUND,
            format!("terminal {terminal_id} does not exist in session {session_id}"),
        )))
    })
}

fn on_data(state: &Mutex<TerminalState>, terminal_id: &str, data: &str) {
    let (frame, sinks) = {
        let mut state = state.lock();
        let Some(record) = state.records.get_mut(terminal_id) else {
            return;
        };
        if record.terminal.status == TerminalStatus::Exited {
            return;
        }
        record.next_seq += 1;
        let frame = TerminalFrame::Output {
            seq: record.next_seq,
            session_id: record.terminal.session_id.clone(),
            terminal_id: record.terminal.id.clone(),
            timestamp: now_iso_date_time(),
            payload: TerminalOutputPayload { data: data.into() },
        };
        push_frame(record, frame.clone());
        (frame, record.sinks.values().cloned().collect::<Vec<_>>())
    };
    for sink in sinks {
        sink.send(frame.clone());
    }
}

fn mark_exited(state: &Mutex<TerminalState>, terminal_id: &str, exit_code: Option<i32>) {
    let (frame, sinks, disposables) = {
        let mut state = state.lock();
        let Some(record) = state.records.get_mut(terminal_id) else {
            return;
        };
        if record.terminal.status == TerminalStatus::Exited {
            return;
        }
        record.closed = true;
        record.terminal.status = TerminalStatus::Exited;
        record.terminal.exited_at = Some(now_iso_date_time());
        record.terminal.exit_code = Some(exit_code.map_or(Value::Null, Value::from));
        let frame = TerminalFrame::Exit {
            session_id: record.terminal.session_id.clone(),
            terminal_id: record.terminal.id.clone(),
            timestamp: now_iso_date_time(),
            payload: TerminalExitPayload {
                // The source always includes `exit_code`; an unknown code is
                // represented by JSON `null`, not an omitted payload field.
                exit_code: Some(exit_code.map_or(Value::Null, Value::from)),
            },
        };
        push_frame(record, frame.clone());
        (
            frame,
            record.sinks.values().cloned().collect::<Vec<_>>(),
            std::mem::take(&mut record.disposables),
        )
    };
    for sink in sinks {
        sink.send(frame.clone());
    }
    let _ = dispose_all(disposables);
}

fn push_frame(record: &mut TerminalRecord, frame: TerminalFrame) {
    record.buffer.push(frame);
    if record.buffer.len() > DEFAULT_MAX_BUFFERED_FRAMES {
        let excess = record.buffer.len() - DEFAULT_MAX_BUFFERED_FRAMES;
        record.buffer.drain(..excess);
    }
}

fn frame_seq(frame: &TerminalFrame) -> u64 {
    match frame {
        TerminalFrame::Output { seq, .. } => *seq,
        TerminalFrame::Exit { .. } => u64::MAX,
    }
}

fn default_shell() -> String {
    std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into())
}

impl Disposable for SessionTerminalService {
    fn dispose(&self) -> DisposeResult {
        let records = std::mem::take(&mut self.state.lock().records);
        for (_, record) in records {
            let _ = dispose_all(record.disposables);
            let _ = record.process.kill();
        }
        Ok(())
    }
}

#[async_trait]
impl SessionTerminalServiceContract for SessionTerminalService {
    async fn create(&self, input: CreateTerminalRequest) -> SessionTerminalResult<Terminal> {
        Self::create(self, input).await
    }
    async fn list(&self) -> SessionTerminalResult<Vec<Terminal>> {
        Self::list(self).await
    }
    async fn get(&self, terminal_id: String) -> SessionTerminalResult<Terminal> {
        Self::get(self, &terminal_id).await
    }
    async fn attach(
        &self,
        terminal_id: String,
        sink: Arc<dyn TerminalAttachSink>,
        options: TerminalAttachOptions,
    ) -> SessionTerminalResult<AttachResult> {
        Self::attach(self, &terminal_id, sink, options).await
    }
    fn detach(&self, terminal_id: &str, sink_id: &str) {
        Self::detach(self, terminal_id, sink_id);
    }
    fn detach_all_for_sink(&self, sink_id: &str) {
        Self::detach_all_for_sink(self, sink_id);
    }
    async fn write(&self, terminal_id: String, data: String) -> SessionTerminalResult<()> {
        Self::write(self, &terminal_id, &data).await
    }
    async fn resize(&self, terminal_id: String, cols: u32, rows: u32) -> SessionTerminalResult<()> {
        Self::resize(self, &terminal_id, cols, rows).await
    }
    async fn close(&self, terminal_id: String) -> SessionTerminalResult<CloseResult> {
        Self::close(self, &terminal_id).await
    }
}

pub fn register_session_terminal_service() {
    register_scoped_service(
        LifecycleScope::Session,
        SESSION_TERMINAL_SERVICE_ID,
        SyncDescriptor::new(|accessor| {
            let host = accessor.get(HOST_TERMINAL_SERVICE_ID)?;
            let workspace = accessor.get(SESSION_WORKSPACE_CONTEXT_ID)?;
            let context = accessor.get(SESSION_CONTEXT_ID)?;
            let service: Arc<dyn SessionTerminalServiceContract> =
                Arc::new(SessionTerminalService::new(
                    host.0.clone(),
                    workspace.0.clone(),
                    (*context).clone(),
                ));
            Ok(SessionTerminalServiceHandle(service))
        }),
        InstantiationType::Eager,
        "terminal",
    );
}

#[cfg(test)]
mod tests {
    use parking_lot::Mutex;
    use std::path::{Path, PathBuf};

    use crate::{
        _base::event::{Emitter, Event},
        os::interface::terminal::{TerminalProcessExit, TerminalStatus},
        session::session_context::{SessionContextInput, make_session_context},
    };

    use super::*;

    #[derive(Default)]
    struct FakeProcess {
        data: Emitter<String>,
        exit: Emitter<TerminalProcessExit>,
        writes: Mutex<Vec<String>>,
        resizes: Mutex<Vec<(u32, u32)>>,
        killed: Mutex<usize>,
    }

    impl FakeProcess {
        fn emit_data(&self, data: &str) {
            self.data.fire(&data.into());
        }

        fn emit_exit(&self, exit_code: Option<i32>) {
            self.exit.fire(&TerminalProcessExit { exit_code });
        }
    }

    impl TerminalProcess for FakeProcess {
        fn on_process_data(&self) -> Event<String> {
            self.data.event()
        }

        fn on_process_exit(&self) -> Event<TerminalProcessExit> {
            self.exit.event()
        }

        fn write(&self, data: &str) -> Result<(), TerminalProcessError> {
            self.writes.lock().push(data.into());
            Ok(())
        }

        fn resize(&self, cols: u32, rows: u32) -> Result<(), TerminalProcessError> {
            self.resizes.lock().push((cols, rows));
            Ok(())
        }

        fn kill(&self) -> Result<(), TerminalProcessError> {
            *self.killed.lock() += 1;
            Ok(())
        }
    }

    #[derive(Default)]
    struct FakeHost {
        processes: Mutex<Vec<Arc<FakeProcess>>>,
        options: Mutex<Vec<TerminalSpawnOptions>>,
    }

    #[async_trait]
    impl HostTerminalService for FakeHost {
        async fn spawn(
            &self,
            options: TerminalSpawnOptions,
        ) -> Result<Arc<dyn TerminalProcess>, TerminalProcessError> {
            let process = Arc::new(FakeProcess::default());
            self.options.lock().push(options);
            self.processes.lock().push(Arc::clone(&process));
            Ok(process)
        }
    }

    struct Workspace;

    impl SessionWorkspaceContextContract for Workspace {
        fn work_dir(&self) -> PathBuf {
            PathBuf::from("/workspace")
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
            PathBuf::from("/workspace").join(relative)
        }
        fn is_within(&self, _: &str) -> bool {
            true
        }
        fn assert_allowed(
            &self,
            path: &str,
            _: PathAccessOperation,
        ) -> Result<PathBuf, PathAccessError> {
            Ok(self.resolve(path))
        }
        fn add_additional_dir(&self, _: &str) -> std::io::Result<()> {
            Ok(())
        }
        fn remove_additional_dir(&self, _: &str) -> std::io::Result<()> {
            Ok(())
        }
    }

    struct Sink {
        id: String,
        frames: Mutex<Vec<TerminalFrame>>,
    }

    impl Sink {
        fn new(id: &str) -> Self {
            Self {
                id: id.into(),
                frames: Mutex::new(Vec::new()),
            }
        }
    }

    impl TerminalAttachSink for Sink {
        fn id(&self) -> &str {
            &self.id
        }
        fn send(&self, frame: TerminalFrame) {
            self.frames.lock().push(frame);
        }
    }

    fn service() -> (SessionTerminalService, Arc<FakeHost>) {
        let host = Arc::new(FakeHost::default());
        let host_contract: Arc<dyn HostTerminalService> = host.clone();
        let workspace: Arc<dyn SessionWorkspaceContextContract> = Arc::new(Workspace);
        let context = make_session_context(SessionContextInput {
            session_id: "session-1".into(),
            workspace_id: "workspace-1".into(),
            session_dir: "/sessions/session-1".into(),
            session_scope: "sessions/session-1".into(),
            cwd: "/workspace".into(),
            meta_scope: None,
        });
        (
            SessionTerminalService::new(host_contract, workspace, context),
            host,
        )
    }

    #[tokio::test]
    async fn creates_replays_streams_and_exits_in_source_order() {
        let (service, host) = service();
        let terminal = service
            .create(CreateTerminalRequest {
                cwd: Some("subdir".into()),
                cols: Some(100),
                rows: Some(40),
                ..Default::default()
            })
            .await
            .unwrap();
        let expected_cwd = Path::new("/workspace")
            .join("subdir")
            .to_string_lossy()
            .into_owned();
        assert_eq!(terminal.session_id, "session-1");
        assert_eq!(terminal.cwd, expected_cwd);
        assert_eq!(terminal.cols, 100);
        assert_eq!(host.options.lock()[0].cwd, terminal.cwd);

        let process = host.processes.lock()[0].clone();
        process.emit_data("first");
        let sink = Arc::new(Sink::new("client"));
        assert_eq!(
            service
                .attach(&terminal.id, sink.clone(), TerminalAttachOptions::default())
                .await
                .unwrap(),
            AttachResult { replayed: 1 }
        );
        process.emit_data("second");
        process.emit_exit(Some(7));

        {
            let frames = sink.frames.lock();
            assert!(
                matches!(&frames[0], TerminalFrame::Output { seq: 1, payload, .. } if payload.data == "first")
            );
            assert!(
                matches!(&frames[1], TerminalFrame::Output { seq: 2, payload, .. } if payload.data == "second")
            );
            assert!(
                matches!(&frames[2], TerminalFrame::Exit { payload, .. } if payload.exit_code == Some(Value::from(7)))
            );
        }
        let fetched = service.get(&terminal.id).await.unwrap();
        assert_eq!(fetched.status, TerminalStatus::Exited);
        assert_eq!(fetched.exit_code, Some(Value::from(7)));
    }

    #[tokio::test]
    async fn delegates_writes_resize_close_and_dispose() {
        let (service, host) = service();
        let terminal = service
            .create(CreateTerminalRequest::default())
            .await
            .unwrap();
        let process = host.processes.lock()[0].clone();

        service.write(&terminal.id, "ls\n").await.unwrap();
        service.resize(&terminal.id, 120, 50).await.unwrap();
        assert_eq!(*process.writes.lock(), ["ls\n"]);
        assert_eq!(*process.resizes.lock(), [(120, 50)]);
        assert_eq!(service.get(&terminal.id).await.unwrap().cols, 120);

        assert_eq!(
            service.close(&terminal.id).await.unwrap(),
            CloseResult { closed: true }
        );
        assert_eq!(*process.killed.lock(), 1);
        service.dispose().unwrap();
        assert_eq!(*process.killed.lock(), 2);
    }

    #[tokio::test]
    async fn rejects_unknown_terminals_with_the_source_error_code() {
        let (service, _) = service();
        let error = service.get("missing").await.unwrap_err();
        assert!(matches!(
            error,
            SessionTerminalError::NotFound(error) if error.code == TERMINAL_NOT_FOUND
        ));
        assert_eq!(
            SESSION_TERMINAL_SERVICE_ID.to_string(),
            "sessionTerminalService"
        );
    }
}
