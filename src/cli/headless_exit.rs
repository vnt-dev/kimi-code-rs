use std::{future::Future, pin::Pin, sync::Arc, time::Duration};

use futures_util::future::join_all;
use tokio::{
    io::{AsyncWrite, AsyncWriteExt},
    task::JoinHandle,
};

pub const HEADLESS_FORCE_EXIT_GRACE: Duration = Duration::from_millis(2_000);
pub const HEADLESS_STDIO_DRAIN_TIMEOUT: Duration = Duration::from_millis(10_000);

/// Minimal process surface needed to force a completed headless run to exit.
pub trait ExitableProcess: Send + Sync + 'static {
    fn exit(&self, code: i32);
}

pub type FlushFuture<'a> = Pin<Box<dyn Future<Output = ()> + Send + 'a>>;

/// Build the ordered flush operation used by [`drain_stdio`].
pub fn flush_stream<'a, W>(stream: &'a mut W) -> FlushFuture<'a>
where
    W: AsyncWrite + Send + Unpin + ?Sized,
{
    Box::pin(async move {
        let _ = stream.flush().await;
    })
}

// Original:
//   apps/kimi-code/src/cli/headless-exit.ts
//   scheduleHeadlessForceExit()
//
// Rust adaptation:
//   A detached Tokio task has the original unref'd-timer lifecycle: it does not
//   keep a runtime alive during normal shutdown. Aborting the returned handle
//   corresponds to clearTimeout().
pub fn schedule_headless_force_exit<P, F>(
    process: Arc<P>,
    get_exit_code: F,
    grace: Duration,
) -> JoinHandle<()>
where
    P: ExitableProcess,
    F: FnOnce() -> i32 + Send + 'static,
{
    tokio::spawn(async move {
        tokio::time::sleep(grace).await;
        process.exit(get_exit_code());
    })
}

// Original: drainStdio()
pub async fn drain_stdio(streams: Vec<FlushFuture<'_>>, timeout: Duration) {
    let _ = tokio::time::timeout(timeout, join_all(streams)).await;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FinalizeHeadlessOptions {
    pub drain_timeout: Duration,
    pub grace: Duration,
}

impl Default for FinalizeHeadlessOptions {
    fn default() -> Self {
        Self {
            drain_timeout: HEADLESS_STDIO_DRAIN_TIMEOUT,
            grace: HEADLESS_FORCE_EXIT_GRACE,
        }
    }
}

// Original: finalizeHeadlessRun()
pub async fn finalize_headless_run<P, F>(
    process: Arc<P>,
    streams: Vec<FlushFuture<'_>>,
    get_exit_code: F,
    options: FinalizeHeadlessOptions,
) -> JoinHandle<()>
where
    P: ExitableProcess,
    F: FnOnce() -> i32 + Send + 'static,
{
    drain_stdio(streams, options.drain_timeout).await;
    schedule_headless_force_exit(process, get_exit_code, options.grace)
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Mutex,
        atomic::{AtomicI32, Ordering},
    };

    use tokio::sync::oneshot;

    use super::*;

    #[derive(Default)]
    struct ProcessMock {
        exit_codes: Mutex<Vec<i32>>,
    }

    impl ExitableProcess for ProcessMock {
        fn exit(&self, code: i32) {
            self.exit_codes.lock().expect("exit codes").push(code);
        }
    }

    impl ProcessMock {
        fn exit_codes(&self) -> Vec<i32> {
            self.exit_codes.lock().expect("exit codes").clone()
        }
    }

    #[tokio::test(start_paused = true)]
    async fn force_exits_with_the_lazily_resolved_code_after_grace() {
        let process = Arc::new(ProcessMock::default());
        let code = Arc::new(AtomicI32::new(0));
        let code_for_timer = Arc::clone(&code);
        let handle = schedule_headless_force_exit(
            Arc::clone(&process),
            move || code_for_timer.load(Ordering::SeqCst),
            Duration::from_millis(2_000),
        );
        code.store(7, Ordering::SeqCst);

        tokio::time::advance(Duration::from_millis(1_999)).await;
        assert!(process.exit_codes().is_empty());
        tokio::time::advance(Duration::from_millis(1)).await;
        handle.await.expect("force-exit task");
        assert_eq!(process.exit_codes(), [7]);
    }

    #[tokio::test(start_paused = true)]
    async fn aborting_the_handle_cancels_the_force_exit() {
        let process = Arc::new(ProcessMock::default());
        let handle =
            schedule_headless_force_exit(Arc::clone(&process), || 0, Duration::from_millis(2_000));
        handle.abort();
        tokio::time::advance(Duration::from_millis(5_000)).await;
        assert!(process.exit_codes().is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn drain_resolves_after_all_streams_flush() {
        let (first_tx, first_rx) = oneshot::channel::<()>();
        let (second_tx, second_rx) = oneshot::channel::<()>();
        let drain = drain_stdio(
            vec![
                Box::pin(async move {
                    let _ = first_rx.await;
                }),
                Box::pin(async move {
                    let _ = second_rx.await;
                }),
            ],
            Duration::from_millis(5_000),
        );
        tokio::pin!(drain);

        first_tx.send(()).expect("first flush");
        assert!(futures_util::poll!(&mut drain).is_pending());
        second_tx.send(()).expect("second flush");
        drain.await;
    }

    #[tokio::test(start_paused = true)]
    async fn drain_gives_up_after_the_timeout() {
        let (_flush_tx, flush_rx) = oneshot::channel::<()>();
        let drain = drain_stdio(
            vec![Box::pin(async move {
                let _ = flush_rx.await;
            })],
            Duration::from_millis(3_000),
        );
        tokio::pin!(drain);

        tokio::time::advance(Duration::from_millis(2_999)).await;
        assert!(futures_util::poll!(&mut drain).is_pending());
        tokio::time::advance(Duration::from_millis(1)).await;
        drain.await;
    }

    #[tokio::test(start_paused = true)]
    async fn finalization_arms_force_exit_only_after_stdio_drains() {
        let process = Arc::new(ProcessMock::default());
        let (flush_tx, flush_rx) = oneshot::channel::<()>();
        let finalize = finalize_headless_run(
            Arc::clone(&process),
            vec![Box::pin(async move {
                let _ = flush_rx.await;
            })],
            || 0,
            FinalizeHeadlessOptions {
                drain_timeout: Duration::from_millis(5_000),
                grace: Duration::from_millis(2_000),
            },
        );
        tokio::pin!(finalize);

        tokio::time::advance(Duration::from_millis(4_000)).await;
        assert!(futures_util::poll!(&mut finalize).is_pending());
        assert!(process.exit_codes().is_empty());

        flush_tx.send(()).expect("flush");
        let force_exit_handle = finalize.await;
        tokio::time::advance(Duration::from_millis(1_999)).await;
        assert!(process.exit_codes().is_empty());
        tokio::time::advance(Duration::from_millis(1)).await;
        force_exit_handle.await.expect("force-exit task");
        assert_eq!(process.exit_codes(), [0]);
    }
}
