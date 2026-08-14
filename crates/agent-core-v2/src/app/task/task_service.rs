//! App-scoped managed task handle implementation.
//!
//! Original: `packages/agent-core-v2/src/app/task/taskService.ts`.

use parking_lot::Mutex;
use std::future::Future;
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU64, Ordering},
};

use async_trait::async_trait;
use tokio::sync::watch;

use crate::_base::{
    di::lifecycle::{Disposable, DisposeResult},
    event::{Emitter, Event},
    utils::abort::{AbortController, AbortError, AbortSignal},
};

use super::contract::{
    DeferredHandle, TaskCancelledError, TaskFailure, TaskHandle, TaskResult, TaskServiceContract,
    TaskState,
};

pub type TaskOutput = Arc<dyn Fn(&str) + Send + Sync>;

pub struct RunHandle<T> {
    id: String,
    state: Arc<Mutex<TaskState>>,
    abort_controller: AbortController,
    state_emitter: Arc<Emitter<TaskState>>,
    output_emitter: Arc<Emitter<String>>,
    result: watch::Receiver<Option<TaskResult<T>>>,
    disposed: Arc<AtomicBool>,
}

impl<T> RunHandle<T> {
    fn transition(&self, to: TaskState) {
        transition(&self.state, &self.state_emitter, &self.disposed, to);
    }
}

#[async_trait]
impl<T> TaskHandle<T> for RunHandle<T>
where
    T: Send + Sync + 'static,
{
    fn id(&self) -> &str {
        &self.id
    }
    fn state(&self) -> TaskState {
        *self.state.lock()
    }

    async fn result(&self) -> TaskResult<T> {
        await_result(self.result.clone(), &self.id).await
    }

    fn on_did_change_state(&self) -> Event<TaskState> {
        self.state_emitter.event()
    }
    fn on_did_output(&self) -> Event<String> {
        self.output_emitter.event()
    }

    fn cancel(&self) {
        if self.state().is_terminal() {
            return;
        }
        self.abort_controller.abort(Some(AbortError::new(
            TaskCancelledError {
                task_id: self.id.clone(),
            }
            .to_string(),
        )));
        self.transition(TaskState::Cancelled);
    }
}

impl<T> Disposable for RunHandle<T>
where
    T: Send + Sync + 'static,
{
    fn dispose(&self) -> DisposeResult {
        if self.disposed.swap(true, Ordering::AcqRel) {
            return Ok(());
        }
        self.cancel();
        self.state_emitter.dispose()?;
        self.output_emitter.dispose()
    }
}

impl<T> Drop for RunHandle<T> {
    fn drop(&mut self) {
        if !self.disposed.swap(true, Ordering::AcqRel) {
            if !self.state.lock().is_terminal() {
                self.abort_controller.abort(Some(AbortError::new(
                    TaskCancelledError {
                        task_id: self.id.clone(),
                    }
                    .to_string(),
                )));
            }
            let _ = self.state_emitter.dispose();
            let _ = self.output_emitter.dispose();
        }
    }
}

pub struct DeferHandle<T> {
    id: String,
    state: Arc<Mutex<TaskState>>,
    state_emitter: Arc<Emitter<TaskState>>,
    output_emitter: Arc<Emitter<String>>,
    result_sender: watch::Sender<Option<TaskResult<T>>>,
    result: watch::Receiver<Option<TaskResult<T>>>,
    disposed: Arc<AtomicBool>,
}

impl<T> DeferHandle<T> {
    fn transition(&self, to: TaskState) {
        transition(&self.state, &self.state_emitter, &self.disposed, to);
    }

    fn cancel_inner(&self) {
        if self.state.lock().is_terminal() {
            return;
        }
        self.transition(TaskState::Cancelled);
        self.result_sender
            .send_replace(Some(Err(Arc::new(TaskCancelledError {
                task_id: self.id.clone(),
            }))));
    }
}

#[async_trait]
impl<T> TaskHandle<T> for DeferHandle<T>
where
    T: Send + Sync + 'static,
{
    fn id(&self) -> &str {
        &self.id
    }
    fn state(&self) -> TaskState {
        *self.state.lock()
    }
    async fn result(&self) -> TaskResult<T> {
        await_result(self.result.clone(), &self.id).await
    }
    fn on_did_change_state(&self) -> Event<TaskState> {
        self.state_emitter.event()
    }
    fn on_did_output(&self) -> Event<String> {
        self.output_emitter.event()
    }
    fn cancel(&self) {
        self.cancel_inner();
    }
}

impl<T> DeferredHandle<T> for DeferHandle<T>
where
    T: Send + Sync + 'static,
{
    fn resolve(&self, value: T) {
        if self.state().is_terminal() {
            return;
        }
        self.transition(TaskState::Completed);
        self.result_sender.send_replace(Some(Ok(Arc::new(value))));
    }

    fn reject(&self, reason: TaskFailure) {
        if self.state().is_terminal() {
            return;
        }
        self.transition(TaskState::Failed);
        self.result_sender.send_replace(Some(Err(reason)));
    }
}

impl<T> Disposable for DeferHandle<T>
where
    T: Send + Sync + 'static,
{
    fn dispose(&self) -> DisposeResult {
        if self.disposed.swap(true, Ordering::AcqRel) {
            return Ok(());
        }
        self.cancel_inner();
        self.state_emitter.dispose()?;
        self.output_emitter.dispose()
    }
}

impl<T> Drop for DeferHandle<T> {
    fn drop(&mut self) {
        if !self.disposed.swap(true, Ordering::AcqRel) {
            if !self.state.lock().is_terminal() {
                self.result_sender
                    .send_replace(Some(Err(Arc::new(TaskCancelledError {
                        task_id: self.id.clone(),
                    }))));
            }
            let _ = self.state_emitter.dispose();
            let _ = self.output_emitter.dispose();
        }
    }
}

#[derive(Default)]
pub struct TaskService {
    next_id: AtomicU64,
}

impl TaskService {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn run<T, F, Fut>(&self, function: F) -> Arc<RunHandle<T>>
    where
        T: Send + Sync + 'static,
        F: FnOnce(AbortSignal, TaskOutput) -> Fut + Send + 'static,
        Fut: Future<Output = Result<T, TaskFailure>> + Send + 'static,
    {
        let id = self.generate_id();
        let state = Arc::new(Mutex::new(TaskState::Pending));
        let abort_controller = AbortController::new();
        let state_emitter = Arc::new(Emitter::new());
        let output_emitter = Arc::new(Emitter::new());
        let disposed = Arc::new(AtomicBool::new(false));
        let (result_sender, result) = watch::channel(None);
        let handle = Arc::new(RunHandle {
            id: id.clone(),
            state: Arc::clone(&state),
            abort_controller: abort_controller.clone(),
            state_emitter: Arc::clone(&state_emitter),
            output_emitter: Arc::clone(&output_emitter),
            result,
            disposed: Arc::clone(&disposed),
        });
        handle.transition(TaskState::Running);
        let signal = abort_controller.signal();
        let function_signal = signal.clone();
        let output_state = Arc::clone(&state);
        let output_disposed = Arc::clone(&disposed);
        let output_events = Arc::clone(&output_emitter);
        let output: TaskOutput = Arc::new(move |data| {
            if !output_state.lock().is_terminal() && !output_disposed.load(Ordering::Acquire) {
                output_events.fire(&data.to_owned());
            }
        });
        tokio::spawn(async move {
            let result = function(function_signal, output).await;
            let result = match result {
                Ok(_) if signal.aborted() => {
                    transition(&state, &state_emitter, &disposed, TaskState::Cancelled);
                    Err(Arc::new(TaskCancelledError { task_id: id }) as TaskFailure)
                }
                Ok(value) => {
                    transition(&state, &state_emitter, &disposed, TaskState::Completed);
                    Ok(Arc::new(value))
                }
                Err(error) => {
                    transition(
                        &state,
                        &state_emitter,
                        &disposed,
                        if signal.aborted() {
                            TaskState::Cancelled
                        } else {
                            TaskState::Failed
                        },
                    );
                    Err(error)
                }
            };
            result_sender.send_replace(Some(result));
        });
        handle
    }

    pub fn defer<T>(&self) -> Arc<DeferHandle<T>>
    where
        T: Send + Sync + 'static,
    {
        let (result_sender, result) = watch::channel(None);
        Arc::new(DeferHandle {
            id: self.generate_id(),
            state: Arc::new(Mutex::new(TaskState::Pending)),
            state_emitter: Arc::new(Emitter::new()),
            output_emitter: Arc::new(Emitter::new()),
            result_sender,
            result,
            disposed: Arc::new(AtomicBool::new(false)),
        })
    }

    fn generate_id(&self) -> String {
        format!("task-{}", self.next_id.fetch_add(1, Ordering::Relaxed))
    }
}

impl TaskServiceContract for TaskService {}

fn transition(
    state: &Mutex<TaskState>,
    emitter: &Emitter<TaskState>,
    disposed: &AtomicBool,
    to: TaskState,
) {
    let mut state = state.lock();
    if state.is_terminal() {
        return;
    }
    *state = to;
    drop(state);
    if !disposed.load(Ordering::Acquire) {
        emitter.fire(&to);
    }
}

async fn await_result<T>(
    mut receiver: watch::Receiver<Option<TaskResult<T>>>,
    task_id: &str,
) -> TaskResult<T> {
    loop {
        if let Some(result) = receiver.borrow().clone() {
            return result;
        }
        if receiver.changed().await.is_err() {
            return Err(Arc::new(TaskCancelledError {
                task_id: task_id.into(),
            }));
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{io, time::Duration};

    use tokio::sync::Notify;

    use super::*;

    #[tokio::test]
    async fn run_emits_output_completes_and_allows_repeated_result_waits() {
        let service = TaskService::new();
        let release = Arc::new(Notify::new());
        let task_release = Arc::clone(&release);
        let handle = service.run(move |_, output| async move {
            task_release.notified().await;
            output("hello");
            Ok(7_u8)
        });
        assert_eq!(handle.id(), "task-0");
        assert_eq!(handle.state(), TaskState::Running);
        let output = Arc::new(Mutex::new(Vec::new()));
        let captured = Arc::clone(&output);
        let _subscription = handle.on_did_output().subscribe(move |value| {
            captured.lock().push(value.clone());
        });
        release.notify_one();
        assert_eq!(*handle.result().await.unwrap(), 7);
        assert_eq!(*handle.result().await.unwrap(), 7);
        assert_eq!(handle.state(), TaskState::Completed);
        assert_eq!(*output.lock(), ["hello"]);
    }

    #[tokio::test]
    async fn cancellation_is_terminal_and_success_after_abort_becomes_cancelled_error() {
        let handle = TaskService::new().run(|signal, _| async move {
            signal.cancelled().await;
            Ok(())
        });
        handle.cancel();
        handle.cancel();
        let error = handle.result().await.unwrap_err();
        assert!(error.downcast_ref::<TaskCancelledError>().is_some());
        assert_eq!(handle.state(), TaskState::Cancelled);
    }

    #[tokio::test]
    async fn deferred_handles_resolve_reject_cancel_once_and_use_monotonic_ids() {
        let service = TaskService::new();
        let resolved = service.defer::<String>();
        let rejected = service.defer::<String>();
        assert_eq!(resolved.id(), "task-0");
        assert_eq!(rejected.id(), "task-1");
        resolved.resolve("ok".into());
        resolved.cancel();
        assert_eq!(resolved.result().await.unwrap().as_str(), "ok");
        let failure: TaskFailure = Arc::new(io::Error::other("boom"));
        rejected.reject(failure);
        rejected.resolve("late".into());
        assert_eq!(rejected.result().await.unwrap_err().to_string(), "boom");

        let cancelled = service.defer::<()>();
        cancelled.cancel();
        assert!(cancelled.result().await.is_err());
        tokio::time::sleep(Duration::ZERO).await;
    }
}
