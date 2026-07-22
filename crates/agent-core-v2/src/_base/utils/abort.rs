use std::{error::Error, fmt, future::Future, sync::Arc, time::Duration};

use tokio::{sync::watch, task::JoinHandle};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AbortError {
    message: String,
    user_cancelled: bool,
}

impl AbortError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            user_cancelled: false,
        }
    }

    pub fn name(&self) -> &'static str {
        "AbortError"
    }

    pub fn is_user_cancellation(&self) -> bool {
        self.user_cancelled
    }
}

impl fmt::Display for AbortError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for AbortError {}

pub fn abort_error(message: Option<&str>) -> AbortError {
    AbortError::new(message.unwrap_or("Aborted"))
}

pub fn user_cancellation_reason() -> AbortError {
    AbortError {
        message: "Aborted by the user".into(),
        user_cancelled: true,
    }
}

pub fn is_abort_error(error: &(dyn Error + 'static)) -> bool {
    error.downcast_ref::<AbortError>().is_some()
}

pub fn is_user_cancellation(error: &(dyn Error + 'static)) -> bool {
    error
        .downcast_ref::<AbortError>()
        .is_some_and(AbortError::is_user_cancellation)
}

#[derive(Clone)]
pub struct AbortSignal {
    receiver: watch::Receiver<Option<Arc<AbortError>>>,
}

impl AbortSignal {
    pub fn aborted(&self) -> bool {
        self.receiver.borrow().is_some()
    }

    pub fn reason(&self) -> Option<Arc<AbortError>> {
        self.receiver.borrow().clone()
    }

    pub fn throw_if_aborted(&self) -> Result<(), Arc<AbortError>> {
        match self.reason() {
            Some(reason) => Err(reason),
            None => Ok(()),
        }
    }

    async fn cancelled(&self) -> Arc<AbortError> {
        if let Some(reason) = self.reason() {
            return reason;
        }
        let mut receiver = self.receiver.clone();
        loop {
            if receiver.changed().await.is_err() {
                return Arc::new(abort_error(None));
            }
            if let Some(reason) = receiver.borrow().clone() {
                return reason;
            }
        }
    }
}

#[derive(Clone)]
pub struct AbortController {
    sender: watch::Sender<Option<Arc<AbortError>>>,
}

impl Default for AbortController {
    fn default() -> Self {
        Self::new()
    }
}

impl AbortController {
    pub fn new() -> Self {
        let (sender, _) = watch::channel(None);
        Self { sender }
    }

    pub fn signal(&self) -> AbortSignal {
        AbortSignal {
            receiver: self.sender.subscribe(),
        }
    }

    pub fn abort(&self, reason: Option<AbortError>) {
        if self.sender.borrow().is_none() {
            self.sender
                .send_replace(Some(Arc::new(reason.unwrap_or_else(|| abort_error(None)))));
        }
    }
}

// Original: packages/agent-core-v2/src/_base/utils/abort.ts, abortable().
pub async fn abortable<T>(
    future: impl Future<Output = T>,
    signal: &AbortSignal,
) -> Result<T, Arc<AbortError>> {
    if let Some(reason) = signal.reason() {
        return Err(reason);
    }
    tokio::select! {
        biased;
        reason = signal.cancelled() => Err(reason),
        value = future => Ok(value),
    }
}

pub struct AbortLink {
    task: Option<JoinHandle<()>>,
}

impl AbortLink {
    pub fn unlink(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

impl Drop for AbortLink {
    fn drop(&mut self) {
        self.unlink();
    }
}

// Original: linkAbortSignal(). Dropping the returned guard removes the link.
pub fn link_abort_signal(source: &AbortSignal, target: AbortController) -> AbortLink {
    if let Some(reason) = source.reason() {
        target.abort(Some((*reason).clone()));
        return AbortLink { task: None };
    }
    let source = source.clone();
    AbortLink {
        task: Some(tokio::spawn(async move {
            let reason = source.cancelled().await;
            target.abort(Some((*reason).clone()));
        })),
    }
}

pub struct DeadlineAbortSignal {
    signal: AbortSignal,
    timed_out: Arc<std::sync::atomic::AtomicBool>,
    timer: Option<JoinHandle<()>>,
    link: AbortLink,
}

impl DeadlineAbortSignal {
    pub fn signal(&self) -> &AbortSignal {
        &self.signal
    }

    pub fn timed_out(&self) -> bool {
        self.timed_out.load(std::sync::atomic::Ordering::Acquire)
    }

    pub fn clear(&mut self) {
        if let Some(timer) = self.timer.take() {
            timer.abort();
        }
        self.link.unlink();
    }
}

impl Drop for DeadlineAbortSignal {
    fn drop(&mut self) {
        self.clear();
    }
}

// Original: createDeadlineAbortSignal().
pub fn create_deadline_abort_signal(
    source: &AbortSignal,
    timeout: Duration,
) -> DeadlineAbortSignal {
    let controller = AbortController::new();
    let signal = controller.signal();
    let link = link_abort_signal(source, controller.clone());
    let timed_out = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let timed_out_for_task = Arc::clone(&timed_out);
    let timer = tokio::spawn(async move {
        tokio::time::sleep(timeout).await;
        timed_out_for_task.store(true, std::sync::atomic::Ordering::Release);
        controller.abort(Some(abort_error(None)));
    });
    DeadlineAbortSignal {
        signal,
        timed_out,
        timer: Some(timer),
        link,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distinguishes_user_cancellation_from_generic_abort() {
        let user = user_cancellation_reason();
        let generic = abort_error(None);
        assert!(is_abort_error(&user));
        assert!(is_user_cancellation(&user));
        assert!(is_abort_error(&generic));
        assert!(!is_user_cancellation(&generic));
        assert_eq!(
            abort_error(Some("Session closed")).to_string(),
            "Session closed"
        );
    }

    #[tokio::test]
    async fn abortable_preserves_existing_and_pending_reasons() {
        let already = AbortController::new();
        already.abort(Some(user_cancellation_reason()));
        let error = abortable(async { "ok" }, &already.signal())
            .await
            .unwrap_err();
        assert!(error.is_user_cancellation());

        let pending = AbortController::new();
        let signal = pending.signal();
        let task =
            tokio::spawn(async move { abortable(std::future::pending::<()>(), &signal).await });
        pending.abort(Some(AbortError::new("cancelled")));
        assert_eq!(task.await.unwrap().unwrap_err().to_string(), "cancelled");
    }

    #[tokio::test]
    async fn deadline_reports_timeout_and_clear_cancels_it() {
        let source = AbortController::new();
        let deadline = create_deadline_abort_signal(&source.signal(), Duration::from_millis(1));
        let error = abortable(std::future::pending::<()>(), deadline.signal())
            .await
            .unwrap_err();
        assert_eq!(error.to_string(), "Aborted");
        assert!(deadline.timed_out());

        let mut cleared = create_deadline_abort_signal(&source.signal(), Duration::from_millis(1));
        cleared.clear();
        tokio::time::sleep(Duration::from_millis(2)).await;
        assert!(!cleared.timed_out());
        assert!(!cleared.signal().aborted());
    }
}
