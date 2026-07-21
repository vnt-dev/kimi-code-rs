use std::{error::Error, fmt, time::Duration};

use tokio::task::{JoinError, JoinHandle};

pub const PROMPT_CLEANUP_TIMEOUT: Duration = Duration::from_millis(8_000);

#[derive(Debug)]
pub enum CleanupTaskError<E> {
    Cleanup(E),
    Join(JoinError),
}

impl<E: fmt::Display> fmt::Display for CleanupTaskError<E> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Cleanup(error) => write!(formatter, "cleanup failed: {error}"),
            Self::Join(error) => write!(formatter, "cleanup task failed: {error}"),
        }
    }
}

impl<E> Error for CleanupTaskError<E>
where
    E: Error + 'static,
{
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Cleanup(error) => Some(error),
            Self::Join(error) => Some(error),
        }
    }
}

// Original:
//   apps/kimi-code/src/cli/run-prompt.ts
//   raceWithTimeout()
//
// Rust adaptation:
//   Cleanup is supplied as a spawned task. Dropping a Tokio JoinHandle detaches
//   rather than cancels the task, matching the original Promise continuing
//   after the caller gives up waiting. A result that arrives in time is still
//   propagated exactly.
pub async fn race_with_timeout<E>(
    mut cleanup: JoinHandle<Result<(), E>>,
    timeout: Duration,
) -> Result<(), CleanupTaskError<E>> {
    tokio::select! {
        biased;
        result = &mut cleanup => {
            result.map_err(CleanupTaskError::Join)?.map_err(CleanupTaskError::Cleanup)
        }
        () = tokio::time::sleep(timeout) => Ok(()),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelNotConfiguredError;

impl fmt::Display for ModelNotConfiguredError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(
            "No model configured. Run `kimi` and use /login to sign in, then retry; or set default_model in config.toml.",
        )
    }
}

impl Error for ModelNotConfiguredError {}

// Original: configuredModel()
pub fn configured_model<'a>(models: impl IntoIterator<Item = Option<&'a str>>) -> Option<&'a str> {
    models
        .into_iter()
        .flatten()
        .find(|model| !model.trim().is_empty())
}

// Original: requireConfiguredModel()
pub fn require_configured_model<'a>(
    models: impl IntoIterator<Item = Option<&'a str>>,
) -> Result<&'a str, ModelNotConfiguredError> {
    configured_model(models).ok_or(ModelNotConfiguredError)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminationSignal {
    Sigint,
    Sighup,
    Sigterm,
}

// Original: signalExitCode()
pub const fn signal_exit_code(signal: TerminationSignal) -> i32 {
    match signal {
        TerminationSignal::Sigint => 130,
        TerminationSignal::Sighup => 129,
        TerminationSignal::Sigterm => 143,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnErrorPayload {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnEndReason {
    Completed,
    Cancelled,
    Failed,
    Blocked,
}

impl TurnEndReason {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::Cancelled => "cancelled",
            Self::Failed => "failed",
            Self::Blocked => "blocked",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnEndedFailure {
    pub reason: TurnEndReason,
    pub error: Option<TurnErrorPayload>,
}

// Original: formatTurnEndedFailure()
pub fn format_turn_ended_failure(event: &TurnEndedFailure) -> String {
    if event
        .error
        .as_ref()
        .is_some_and(|error| error.code == "provider.filtered")
    {
        return "Provider safety policy blocked the response.".to_owned();
    }
    if let Some(error) = &event.error {
        return format!("{}: {}", error.code, error.message);
    }
    if event.reason == TurnEndReason::Blocked {
        return "Prompt hook blocked the request.".to_owned();
    }
    format!("Prompt turn ended with reason: {}", event.reason.as_str())
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };

    use super::*;

    #[tokio::test(start_paused = true)]
    async fn timeout_detaches_cleanup_instead_of_cancelling_it() {
        let completed = Arc::new(AtomicBool::new(false));
        let completed_in_task = Arc::clone(&completed);
        let task = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(200)).await;
            completed_in_task.store(true, Ordering::SeqCst);
            Ok::<_, std::io::Error>(())
        });

        let wait = race_with_timeout(task, Duration::from_millis(100));
        tokio::pin!(wait);
        tokio::task::yield_now().await;
        assert!(futures_util::poll!(&mut wait).is_pending());
        tokio::time::advance(Duration::from_millis(100)).await;
        assert!(wait.await.is_ok());
        assert!(!completed.load(Ordering::SeqCst));
        tokio::time::advance(Duration::from_millis(100)).await;
        tokio::task::yield_now().await;
        assert!(completed.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn propagates_cleanup_errors_that_arrive_before_timeout() {
        let task = tokio::spawn(async { Err::<(), _>(std::io::Error::other("close failed")) });
        let error = race_with_timeout(task, Duration::from_secs(1))
            .await
            .expect_err("cleanup failure");
        assert!(error.to_string().contains("close failed"));
    }

    #[test]
    fn chooses_the_first_non_blank_model_without_trimming_it() {
        assert_eq!(
            configured_model([None, Some("  "), Some(" model-a "), Some("model-b")]),
            Some(" model-a ")
        );
        assert_eq!(configured_model([None, Some("\t")]), None);
        assert_eq!(
            require_configured_model([None, Some("")])
                .unwrap_err()
                .to_string(),
            "No model configured. Run `kimi` and use /login to sign in, then retry; or set default_model in config.toml."
        );
    }

    #[test]
    fn maps_process_signals_to_shell_exit_codes() {
        assert_eq!(signal_exit_code(TerminationSignal::Sigint), 130);
        assert_eq!(signal_exit_code(TerminationSignal::Sighup), 129);
        assert_eq!(signal_exit_code(TerminationSignal::Sigterm), 143);
    }

    #[test]
    fn formats_turn_failure_precedence() {
        assert_eq!(
            format_turn_ended_failure(&TurnEndedFailure {
                reason: TurnEndReason::Failed,
                error: Some(TurnErrorPayload {
                    code: "provider.filtered".to_owned(),
                    message: "details".to_owned(),
                }),
            }),
            "Provider safety policy blocked the response."
        );
        assert_eq!(
            format_turn_ended_failure(&TurnEndedFailure {
                reason: TurnEndReason::Failed,
                error: Some(TurnErrorPayload {
                    code: "provider.error".to_owned(),
                    message: "model failed".to_owned(),
                }),
            }),
            "provider.error: model failed"
        );
        assert_eq!(
            format_turn_ended_failure(&TurnEndedFailure {
                reason: TurnEndReason::Blocked,
                error: None,
            }),
            "Prompt hook blocked the request."
        );
        assert_eq!(
            format_turn_ended_failure(&TurnEndedFailure {
                reason: TurnEndReason::Cancelled,
                error: None,
            }),
            "Prompt turn ended with reason: cancelled"
        );
    }
}
