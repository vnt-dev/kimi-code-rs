use std::time::Duration;

use tokio::task::JoinHandle;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct IntervalTimerOptions {
    pub unref: bool,
}

#[derive(Default)]
pub struct IntervalTimer {
    options: IntervalTimerOptions,
    task: Option<JoinHandle<()>>,
}

impl IntervalTimer {
    pub fn new(options: IntervalTimerOptions) -> Self {
        Self {
            options,
            task: None,
        }
    }

    // Original: packages/agent-core-v2/src/_base/utils/timer.ts, IntervalTimer.cancel().
    pub fn cancel(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }

    // Original: IntervalTimer.cancelAndSet(). Tokio tasks do not keep a runtime
    // alive, so Node's `unref` option requires no additional Rust behavior.
    pub fn cancel_and_set(
        &mut self,
        mut runner: impl FnMut() + Send + 'static,
        interval: Duration,
    ) {
        self.cancel();
        let _unref = self.options.unref;
        self.task = Some(tokio::spawn(async move {
            loop {
                tokio::time::sleep(interval).await;
                runner();
            }
        }));
    }

    pub fn is_set(&self) -> bool {
        self.task.is_some()
    }
}

impl Drop for IntervalTimer {
    fn drop(&mut self) {
        self.cancel();
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    use super::*;

    #[tokio::test(start_paused = true)]
    async fn repeats_replaces_and_cancels_the_runner() {
        let first = Arc::new(AtomicUsize::new(0));
        let second = Arc::new(AtomicUsize::new(0));
        let mut timer = IntervalTimer::default();
        let first_task = Arc::clone(&first);
        timer.cancel_and_set(
            move || {
                first_task.fetch_add(1, Ordering::Relaxed);
            },
            Duration::from_millis(20),
        );
        let second_task = Arc::clone(&second);
        timer.cancel_and_set(
            move || {
                second_task.fetch_add(1, Ordering::Relaxed);
            },
            Duration::from_millis(1),
        );
        tokio::task::yield_now().await;
        tokio::time::advance(Duration::from_millis(4)).await;
        tokio::task::yield_now().await;
        timer.cancel();
        let observed = second.load(Ordering::Relaxed);
        tokio::time::advance(Duration::from_millis(2)).await;

        assert_eq!(first.load(Ordering::Relaxed), 0);
        assert!(observed >= 1);
        assert_eq!(second.load(Ordering::Relaxed), observed);
        assert!(!timer.is_set());
    }
}
