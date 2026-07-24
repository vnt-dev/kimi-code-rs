//! Monotonic wall-clock deadline scheduler.
//!
//! Original: `packages/agent-core-v2/src/agent/goal/goalDeadlineScheduler.ts`
//! and `goalDeadlineSchedulerService.ts`.

use std::{
    ops::Deref,
    sync::{
        Arc, LazyLock,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use crate::_base::di::{
    descriptors::SyncDescriptor,
    instantiation::{ServiceIdentifier, ServicesAccessor},
    lifecycle::{DisposableHandle, to_disposable},
    scope::{InstantiationType, LifecycleScope, register_scoped_service},
};

static MONOTONIC_EPOCH: LazyLock<Instant> = LazyLock::new(Instant::now);

pub type DeadlineCallback = Arc<dyn Fn() + Send + Sync>;

pub trait GoalDeadlineSchedulerContract: Send + Sync {
    fn now(&self) -> f64;
    fn schedule(&self, delay_ms: f64, callback: DeadlineCallback) -> DisposableHandle;
}

#[derive(Clone)]
pub struct GoalDeadlineSchedulerHandle(pub Arc<dyn GoalDeadlineSchedulerContract>);

impl Deref for GoalDeadlineSchedulerHandle {
    type Target = dyn GoalDeadlineSchedulerContract;

    fn deref(&self) -> &Self::Target {
        self.0.as_ref()
    }
}

pub const GOAL_DEADLINE_SCHEDULER_ID: ServiceIdentifier<GoalDeadlineSchedulerHandle> =
    ServiceIdentifier::new("goalDeadlineScheduler");

#[derive(Default)]
pub struct GoalDeadlineSchedulerService;

impl GoalDeadlineSchedulerService {
    fn timeout_duration(delay_ms: f64) -> Duration {
        // Node's setTimeout coerces NaN, zero, negatives, and delays beyond a
        // signed 32-bit millisecond value to one millisecond. This is applied
        // after the source's Math.max(0, delayMs).
        let delay_ms = delay_ms.max(0.0);
        if !delay_ms.is_finite() || delay_ms < 1.0 || delay_ms > i32::MAX as f64 {
            Duration::from_millis(1)
        } else {
            Duration::from_millis(delay_ms.trunc() as u64)
        }
    }
}

impl GoalDeadlineSchedulerContract for GoalDeadlineSchedulerService {
    // Original: GoalDeadlineSchedulerService.now().
    fn now(&self) -> f64 {
        MONOTONIC_EPOCH.elapsed().as_millis() as f64
    }

    // Original: GoalDeadlineSchedulerService.schedule(). A tokio task is the
    // Rust async timer equivalent of Node's unref'd timeout; cancellation is
    // observed immediately before invoking the callback.
    fn schedule(&self, delay_ms: f64, callback: DeadlineCallback) -> DisposableHandle {
        let cancelled = Arc::new(AtomicBool::new(false));
        let timer_cancelled = Arc::clone(&cancelled);
        tokio::spawn(async move {
            tokio::time::sleep(Self::timeout_duration(delay_ms)).await;
            if !timer_cancelled.load(Ordering::Acquire) {
                callback();
            }
        });
        to_disposable(move || {
            cancelled.store(true, Ordering::Release);
        })
    }
}

pub fn register_goal_deadline_scheduler_service() {
    register_scoped_service(
        LifecycleScope::App,
        GOAL_DEADLINE_SCHEDULER_ID,
        SyncDescriptor::new(|_accessor: &dyn ServicesAccessor| {
            let service: Arc<dyn GoalDeadlineSchedulerContract> =
                Arc::new(GoalDeadlineSchedulerService);
            Ok(GoalDeadlineSchedulerHandle(service))
        }),
        InstantiationType::Delayed,
        "goal",
    );
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicUsize;

    use super::*;

    #[test]
    fn service_identifier_and_timeout_coercion_match_node_timers() {
        assert_eq!(
            GOAL_DEADLINE_SCHEDULER_ID.to_string(),
            "goalDeadlineScheduler"
        );
        assert_eq!(
            GoalDeadlineSchedulerService::timeout_duration(-2.0),
            Duration::from_millis(1)
        );
        assert_eq!(
            GoalDeadlineSchedulerService::timeout_duration(f64::NAN),
            Duration::from_millis(1)
        );
        assert_eq!(
            GoalDeadlineSchedulerService::timeout_duration(3.8),
            Duration::from_millis(3)
        );
    }

    #[tokio::test(start_paused = true)]
    async fn scheduled_callbacks_run_once_unless_disposed() {
        let scheduler = GoalDeadlineSchedulerService;
        let fired = Arc::new(AtomicUsize::new(0));
        let callback_fired = Arc::clone(&fired);
        let cancel = scheduler.schedule(
            10.0,
            Arc::new(move || {
                callback_fired.fetch_add(1, Ordering::Relaxed);
            }),
        );
        cancel.dispose().unwrap();
        tokio::time::advance(Duration::from_millis(10)).await;
        tokio::task::yield_now().await;
        assert_eq!(fired.load(Ordering::Relaxed), 0);

        let callback_fired = Arc::clone(&fired);
        let _timer = scheduler.schedule(
            10.0,
            Arc::new(move || {
                callback_fired.fetch_add(1, Ordering::Relaxed);
            }),
        );
        tokio::task::yield_now().await;
        tokio::time::advance(Duration::from_millis(10)).await;
        tokio::task::yield_now().await;
        assert_eq!(fired.load(Ordering::Relaxed), 1);
    }
}
