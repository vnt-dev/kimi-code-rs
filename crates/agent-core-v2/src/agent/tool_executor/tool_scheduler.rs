//! Per-step resource-conflict scheduler for tool calls.
//!
//! Original: `packages/agent-core-v2/src/agent/toolExecutor/toolScheduler.ts`.

use std::{
    collections::VecDeque,
    sync::{Arc, Mutex, Weak},
};

use futures_util::future::BoxFuture;
use tokio::sync::oneshot;

use crate::{
    _base::lifecycle::lifecycle_machine::BoxError,
    tool::{ToolAccess, ToolAccesses},
};

pub type ToolTaskResult<R> = Result<R, BoxError>;
pub type ToolTaskFuture<R> = BoxFuture<'static, ToolTaskResult<R>>;

/// The source scheduler first awaits `start()`, then awaits its returned
/// `result` promise. Keeping those two futures distinct preserves that order.
pub struct ToolTaskStarted<R> {
    pub result: ToolTaskFuture<R>,
}

pub type ToolTaskStartFuture<R> = BoxFuture<'static, Result<ToolTaskStarted<R>, BoxError>>;
pub type ToolTaskStart<R> = Arc<dyn Fn() -> ToolTaskStartFuture<R> + Send + Sync>;

pub struct ToolCallTask<R> {
    pub accesses: ToolAccesses,
    pub start: ToolTaskStart<R>,
}

pub type ToolSchedulerReceipt<R> = oneshot::Receiver<ToolTaskResult<R>>;

struct ScheduledToolCallTask<R> {
    id: u64,
    task: ToolCallTask<R>,
    sender: oneshot::Sender<ToolTaskResult<R>>,
}

struct ActiveToolCallTask {
    id: u64,
    accesses: ToolAccesses,
}

struct SchedulerState<R> {
    next_id: u64,
    active_tasks: Vec<ActiveToolCallTask>,
    queued_tasks: VecDeque<ScheduledToolCallTask<R>>,
}

impl<R> Default for SchedulerState<R> {
    fn default() -> Self {
        Self {
            next_id: 1,
            active_tasks: Vec::new(),
            queued_tasks: VecDeque::new(),
        }
    }
}

/// Stateful scheduler. Tasks begin promptly if no active or earlier queued
/// task has conflicting accesses; otherwise they wait until a completion can
/// release them.
pub struct ToolScheduler<R> {
    state: Arc<Mutex<SchedulerState<R>>>,
}

impl<R> Default for ToolScheduler<R>
where
    R: Send + 'static,
{
    fn default() -> Self {
        Self::new()
    }
}

impl<R> ToolScheduler<R>
where
    R: Send + 'static,
{
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(SchedulerState::default())),
        }
    }

    // Original: ToolScheduler.add(). The returned receiver is the Rust
    // equivalent of the controlled promise returned to the caller.
    pub fn add(&self, task: ToolCallTask<R>) -> ToolSchedulerReceipt<R> {
        let (sender, receiver) = oneshot::channel();
        let scheduled = {
            let mut state = self.state.lock().unwrap();
            let scheduled = ScheduledToolCallTask {
                id: state.next_id,
                task,
                sender,
            };
            state.next_id += 1;
            if is_blocked(&scheduled.task, &state.active_tasks, &state.queued_tasks) {
                state.queued_tasks.push_back(scheduled);
                return receiver;
            }
            state.active_tasks.push(ActiveToolCallTask {
                id: scheduled.id,
                accesses: scheduled.task.accesses.clone(),
            });
            scheduled
        };
        start(Arc::downgrade(&self.state), scheduled);
        receiver
    }
}

fn is_blocked<R>(
    task: &ToolCallTask<R>,
    active_tasks: &[ActiveToolCallTask],
    queued_tasks: &VecDeque<ScheduledToolCallTask<R>>,
) -> bool {
    active_tasks
        .iter()
        .any(|candidate| ToolAccess::conflict(&task.accesses, &candidate.accesses))
        || queued_tasks
            .iter()
            .any(|candidate| ToolAccess::conflict(&task.accesses, &candidate.task.accesses))
}

fn start<R>(state: Weak<Mutex<SchedulerState<R>>>, task: ScheduledToolCallTask<R>)
where
    R: Send + 'static,
{
    let id = task.id;
    let started = (task.task.start)();
    tokio::spawn(async move {
        let result = match started.await {
            Ok(started) => started.result.await,
            Err(error) => Err(error),
        };
        let _ = task.sender.send(result);
        finish(&state, id);
    });
}

fn finish<R>(state: &Weak<Mutex<SchedulerState<R>>>, id: u64)
where
    R: Send + 'static,
{
    let Some(state) = state.upgrade() else { return };
    let ready = {
        let mut state = state.lock().unwrap();
        if let Some(index) = state.active_tasks.iter().position(|task| task.id == id) {
            state.active_tasks.remove(index);
        }
        take_ready_tasks(&mut state)
    };
    for task in ready {
        start(Arc::downgrade(&state), task);
    }
}

fn take_ready_tasks<R>(state: &mut SchedulerState<R>) -> Vec<ScheduledToolCallTask<R>> {
    let mut ready = Vec::new();
    let mut still_queued = VecDeque::new();
    while let Some(task) = state.queued_tasks.pop_front() {
        if is_blocked(&task.task, &state.active_tasks, &still_queued) {
            still_queued.push_back(task);
        } else {
            state.active_tasks.push(ActiveToolCallTask {
                id: task.id,
                accesses: task.task.accesses.clone(),
            });
            ready.push(task);
        }
    }
    state.queued_tasks = still_queued;
    ready
}

#[cfg(test)]
mod tests {
    use std::{
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        },
        time::Duration,
    };

    use super::*;

    fn task(
        accesses: ToolAccesses,
        value: &'static str,
        started: Arc<AtomicBool>,
        release: Option<Arc<tokio::sync::Notify>>,
    ) -> ToolCallTask<String> {
        ToolCallTask {
            accesses,
            start: Arc::new(move || {
                started.store(true, Ordering::SeqCst);
                let release = release.clone();
                Box::pin(async move {
                    Ok(ToolTaskStarted {
                        result: Box::pin(async move {
                            if let Some(release) = release {
                                release.notified().await;
                            }
                            Ok(value.to_owned())
                        }),
                    })
                })
            }),
        }
    }

    #[tokio::test]
    async fn conflicts_wait_while_independent_tasks_start_immediately() {
        let scheduler = ToolScheduler::new();
        let release_write = Arc::new(tokio::sync::Notify::new());
        let write_started = Arc::new(AtomicBool::new(false));
        let read_other_started = Arc::new(AtomicBool::new(false));
        let conflicting_read_started = Arc::new(AtomicBool::new(false));

        let write = scheduler.add(task(
            ToolAccess::write_file("/workspace/a"),
            "write",
            Arc::clone(&write_started),
            Some(Arc::clone(&release_write)),
        ));
        let read_other = scheduler.add(task(
            ToolAccess::read_file("/workspace/b"),
            "other",
            Arc::clone(&read_other_started),
            None,
        ));
        let conflicting_read = scheduler.add(task(
            ToolAccess::read_file("/workspace/a"),
            "after-write",
            Arc::clone(&conflicting_read_started),
            None,
        ));

        tokio::time::timeout(Duration::from_secs(1), async {
            while !write_started.load(Ordering::SeqCst)
                || !read_other_started.load(Ordering::SeqCst)
            {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert!(!conflicting_read_started.load(Ordering::SeqCst));

        release_write.notify_one();
        assert_eq!(write.await.unwrap().unwrap(), "write");
        assert_eq!(read_other.await.unwrap().unwrap(), "other");
        assert_eq!(conflicting_read.await.unwrap().unwrap(), "after-write");
        assert!(conflicting_read_started.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn mutually_conflicting_queued_tasks_are_released_one_at_a_time() {
        let scheduler = ToolScheduler::new();
        let release_a = Arc::new(tokio::sync::Notify::new());
        let release_b = Arc::new(tokio::sync::Notify::new());
        let a_started = Arc::new(AtomicBool::new(false));
        let b_started = Arc::new(AtomicBool::new(false));
        let c_started = Arc::new(AtomicBool::new(false));

        let a = scheduler.add(task(
            ToolAccess::write_file("/workspace/shared"),
            "a",
            Arc::clone(&a_started),
            Some(Arc::clone(&release_a)),
        ));
        let b = scheduler.add(task(
            ToolAccess::write_file("/workspace/shared"),
            "b",
            Arc::clone(&b_started),
            Some(Arc::clone(&release_b)),
        ));
        let c = scheduler.add(task(
            ToolAccess::write_file("/workspace/shared"),
            "c",
            Arc::clone(&c_started),
            None,
        ));

        wait_until_started(&a_started).await;
        assert!(!b_started.load(Ordering::SeqCst));
        assert!(!c_started.load(Ordering::SeqCst));

        release_a.notify_one();
        wait_until_started(&b_started).await;
        assert!(!c_started.load(Ordering::SeqCst));

        release_b.notify_one();
        wait_until_started(&c_started).await;

        assert_eq!(a.await.unwrap().unwrap(), "a");
        assert_eq!(b.await.unwrap().unwrap(), "b");
        assert_eq!(c.await.unwrap().unwrap(), "c");
    }

    #[test]
    fn promotion_is_committed_to_active_state_before_queue_scan_returns() {
        let (b_sender, _b_receiver) = oneshot::channel();
        let (c_sender, _c_receiver) = oneshot::channel();
        let mut state = SchedulerState::default();
        state.queued_tasks.push_back(ScheduledToolCallTask {
            id: 1,
            task: task(
                ToolAccess::write_file("/workspace/shared"),
                "b",
                Arc::new(AtomicBool::new(false)),
                None,
            ),
            sender: b_sender,
        });
        state.queued_tasks.push_back(ScheduledToolCallTask {
            id: 2,
            task: task(
                ToolAccess::write_file("/workspace/shared"),
                "c",
                Arc::new(AtomicBool::new(false)),
                None,
            ),
            sender: c_sender,
        });

        let ready = take_ready_tasks(&mut state);

        assert_eq!(ready.len(), 1);
        assert_eq!(ready[0].id, 1);
        assert_eq!(state.active_tasks.len(), 1);
        assert_eq!(state.active_tasks[0].id, 1);
        assert_eq!(state.queued_tasks.len(), 1);
        assert_eq!(state.queued_tasks[0].id, 2);
    }

    async fn wait_until_started(started: &AtomicBool) {
        tokio::time::timeout(Duration::from_secs(1), async {
            while !started.load(Ordering::SeqCst) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }
}
