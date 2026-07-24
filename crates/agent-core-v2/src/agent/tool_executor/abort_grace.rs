//! Abort grace period for running tools.
//!
//! Original: `toolExecutorService.ts`, `raceWithAbortGrace()`.

use std::{future::Future, pin::pin, time::Duration};

use crate::_base::utils::abort::AbortSignal;

pub const ABORT_GRACE: Duration = Duration::from_secs(2);

pub async fn race_with_abort_grace<R>(
    future: impl Future<Output = R>,
    signal: &AbortSignal,
    fallback: impl FnOnce() -> R,
) -> R {
    let mut future = pin!(future);
    tokio::select! {
        value = &mut future => value,
        _ = signal.cancelled() => {
            tokio::select! {
                value = &mut future => value,
                _ = tokio::time::sleep(ABORT_GRACE) => fallback(),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::_base::utils::abort::AbortController;

    #[tokio::test(start_paused = true)]
    async fn completion_within_grace_beats_fallback() {
        let controller = AbortController::new();
        let signal = controller.signal();
        controller.abort(None);
        let task = tokio::spawn(async move {
            race_with_abort_grace(
                async {
                    tokio::time::sleep(Duration::from_secs(1)).await;
                    "completed"
                },
                &signal,
                || "aborted",
            )
            .await
        });
        tokio::task::yield_now().await;
        tokio::time::advance(Duration::from_secs(1)).await;
        assert_eq!(task.await.unwrap(), "completed");
    }
}
