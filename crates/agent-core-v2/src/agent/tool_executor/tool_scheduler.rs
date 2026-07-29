//! Per-step resource-conflict scheduler for tool calls.
//!
//! Original: `packages/agent-core-v2/src/agent/toolExecutor/toolScheduler.ts`.

use std::{collections::VecDeque, sync::Arc};

use futures_util::future::BoxFuture;
use tokio::task::JoinSet;

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

struct ScheduledToolCallTask<R> {
    id: u64,
    task: ToolCallTask<R>,
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
    state: SchedulerState<R>,
    tasks: JoinSet<(u64, ToolTaskResult<R>)>,
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
            state: SchedulerState::default(),
            tasks: JoinSet::new(),
        }
    }

    // Original: ToolScheduler.add(). JoinSet owns every task in this batch,
    // so dropping the scheduler cancels the whole remaining task tree.
    pub fn add(&mut self, task: ToolCallTask<R>) {
        let scheduled = ScheduledToolCallTask {
            id: self.state.next_id,
            task,
        };
        self.state.next_id += 1;
        if is_blocked(
            &scheduled.task,
            &self.state.active_tasks,
            &self.state.queued_tasks,
        ) {
            self.state.queued_tasks.push_back(scheduled);
            return;
        }
        self.state.active_tasks.push(ActiveToolCallTask {
            id: scheduled.id,
            accesses: scheduled.task.accesses.clone(),
        });
        self.start(scheduled);
    }

    pub fn has_pending(&self) -> bool {
        !self.tasks.is_empty() || !self.state.queued_tasks.is_empty()
    }

    pub async fn next(&mut self) -> Option<ToolTaskResult<R>> {
        let joined = self.tasks.join_next().await?;
        match joined {
            Ok((id, result)) => {
                let ready = finish(&mut self.state, id);
                for task in ready {
                    self.start(task);
                }
                Some(result)
            }
            Err(error) => {
                // A panic or runtime cancellation invalidates the scheduler's
                // active-resource bookkeeping. Cancel and reap the batch
                // before surfacing the failure.
                self.tasks.abort_all();
                while self.tasks.join_next().await.is_some() {}
                self.state.active_tasks.clear();
                self.state.queued_tasks.clear();
                Some(Err(Box::new(error)))
            }
        }
    }

    fn start(&mut self, task: ScheduledToolCallTask<R>) {
        let id = task.id;
        let started = (task.task.start)();
        self.tasks.spawn(async move {
            let result = match started.await {
                Ok(started) => started.result.await,
                Err(error) => Err(error),
            };
            (id, result)
        });
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

fn finish<R>(state: &mut SchedulerState<R>, id: u64) -> Vec<ScheduledToolCallTask<R>> {
    if let Some(index) = state.active_tasks.iter().position(|task| task.id == id) {
        state.active_tasks.remove(index);
    }
    take_ready_tasks(state)
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
        let mut scheduler = ToolScheduler::new();
        let release_write = Arc::new(tokio::sync::Notify::new());
        let write_started = Arc::new(AtomicBool::new(false));
        let read_other_started = Arc::new(AtomicBool::new(false));
        let conflicting_read_started = Arc::new(AtomicBool::new(false));

        scheduler.add(task(
            ToolAccess::write_file("/workspace/a"),
            "write",
            Arc::clone(&write_started),
            Some(Arc::clone(&release_write)),
        ));
        scheduler.add(task(
            ToolAccess::read_file("/workspace/b"),
            "other",
            Arc::clone(&read_other_started),
            None,
        ));
        scheduler.add(task(
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
        let mut results = Vec::new();
        while scheduler.has_pending() {
            results.push(scheduler.next().await.unwrap().unwrap());
        }
        results.sort();
        assert_eq!(results, ["after-write", "other", "write"]);
        assert!(conflicting_read_started.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn mutually_conflicting_queued_tasks_are_released_one_at_a_time() {
        let mut scheduler = ToolScheduler::new();
        let release_a = Arc::new(tokio::sync::Notify::new());
        let release_b = Arc::new(tokio::sync::Notify::new());
        let a_started = Arc::new(AtomicBool::new(false));
        let b_started = Arc::new(AtomicBool::new(false));
        let c_started = Arc::new(AtomicBool::new(false));

        scheduler.add(task(
            ToolAccess::write_file("/workspace/shared"),
            "a",
            Arc::clone(&a_started),
            Some(Arc::clone(&release_a)),
        ));
        scheduler.add(task(
            ToolAccess::write_file("/workspace/shared"),
            "b",
            Arc::clone(&b_started),
            Some(Arc::clone(&release_b)),
        ));
        scheduler.add(task(
            ToolAccess::write_file("/workspace/shared"),
            "c",
            Arc::clone(&c_started),
            None,
        ));

        wait_until_started(&a_started).await;
        assert!(!b_started.load(Ordering::SeqCst));
        assert!(!c_started.load(Ordering::SeqCst));

        release_a.notify_one();
        assert_eq!(scheduler.next().await.unwrap().unwrap(), "a");
        wait_until_started(&b_started).await;
        assert!(!c_started.load(Ordering::SeqCst));

        release_b.notify_one();
        assert_eq!(scheduler.next().await.unwrap().unwrap(), "b");
        wait_until_started(&c_started).await;
        assert_eq!(scheduler.next().await.unwrap().unwrap(), "c");
        assert!(!scheduler.has_pending());
    }

    #[tokio::test]
    async fn dropping_scheduler_cancels_running_batch_tasks() {
        struct DropSignal(Arc<AtomicBool>);
        impl Drop for DropSignal {
            fn drop(&mut self) {
                self.0.store(true, Ordering::SeqCst);
            }
        }

        let entered = Arc::new(AtomicBool::new(false));
        let dropped = Arc::new(AtomicBool::new(false));
        let mut scheduler = ToolScheduler::<String>::new();
        scheduler.add(ToolCallTask {
            accesses: ToolAccess::all(),
            start: Arc::new({
                let entered = Arc::clone(&entered);
                let dropped = Arc::clone(&dropped);
                move || {
                    let entered = Arc::clone(&entered);
                    let dropped = Arc::clone(&dropped);
                    Box::pin(async move {
                        Ok(ToolTaskStarted {
                            result: Box::pin(async move {
                                let _drop_signal = DropSignal(dropped);
                                entered.store(true, Ordering::SeqCst);
                                std::future::pending().await
                            }),
                        })
                    })
                }
            }),
        });
        wait_until_started(&entered).await;

        drop(scheduler);

        tokio::time::timeout(Duration::from_secs(1), async {
            while !dropped.load(Ordering::SeqCst) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }

    #[test]
    fn promotion_is_committed_to_active_state_before_queue_scan_returns() {
        let mut state = SchedulerState::default();
        state.queued_tasks.push_back(ScheduledToolCallTask {
            id: 1,
            task: task(
                ToolAccess::write_file("/workspace/shared"),
                "b",
                Arc::new(AtomicBool::new(false)),
                None,
            ),
        });
        state.queued_tasks.push_back(ScheduledToolCallTask {
            id: 2,
            task: task(
                ToolAccess::write_file("/workspace/shared"),
                "c",
                Arc::new(AtomicBool::new(false)),
                None,
            ),
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
