//! Abort grace period for running tools.
//!
//! Original: `toolExecutorService.ts`, `raceWithAbortGrace()`.

use std::{future::Future, pin::pin, time::Duration};

use crate::_base::utils::abort::{AbortSignal, is_user_cancellation};

pub const ABORT_GRACE: Duration = Duration::from_secs(2);

// Original: toolExecutorService.ts, abortedToolOutput().
pub fn aborted_tool_output(tool_name: &str, signal: &AbortSignal) -> String {
    if signal
        .reason()
        .is_some_and(|reason| is_user_cancellation(reason.as_ref()))
    {
        return format!(
            "The user manually interrupted \"{tool_name}\" (and anything else running at the same time). This was a deliberate user action, not a system error, timeout, or capacity limit. Do not retry automatically or guess at the cause — wait for the user's next instruction."
        );
    }
    format!("Tool \"{tool_name}\" was aborted")
}

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

    #[test]
    fn abort_output_distinguishes_user_cancellation() {
        let normal = AbortController::new();
        normal.abort(None);
        assert_eq!(
            aborted_tool_output("Read", &normal.signal()),
            "Tool \"Read\" was aborted"
        );

        let user = AbortController::new();
        user.abort(Some(crate::_base::utils::abort::user_cancellation_reason()));
        assert!(
            aborted_tool_output("Read", &user.signal())
                .starts_with("The user manually interrupted \"Read\"")
        );
    }
}
