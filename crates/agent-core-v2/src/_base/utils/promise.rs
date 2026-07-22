use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
    time::Duration,
};

pub struct TimeoutOutcome<T> {
    sleep: Option<Pin<Box<tokio::time::Sleep>>>,
    outcome: Option<T>,
}

impl<T> TimeoutOutcome<T> {
    pub fn clear(&mut self) {
        self.sleep = None;
    }

    pub fn is_set(&self) -> bool {
        self.sleep.is_some()
    }
}

impl<T: Unpin> Future for TimeoutOutcome<T> {
    type Output = T;

    fn poll(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        let Some(sleep) = self.sleep.as_mut() else {
            return Poll::Pending;
        };
        if sleep.as_mut().poll(context).is_pending() {
            return Poll::Pending;
        }
        self.sleep = None;
        Poll::Ready(self.outcome.take().expect("outcome resolves only once"))
    }
}

// Original: packages/agent-core-v2/src/_base/utils/promise.ts, timeoutOutcome().
pub fn timeout_outcome<T>(timeout: Option<Duration>, outcome: T) -> TimeoutOutcome<T> {
    let sleep = timeout
        .filter(|timeout| !timeout.is_zero())
        .map(|timeout| Box::pin(tokio::time::sleep(timeout)));
    TimeoutOutcome {
        sleep,
        outcome: Some(outcome),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn resolves_after_a_positive_timeout() {
        assert_eq!(
            timeout_outcome(Some(Duration::from_millis(1)), "timed-out").await,
            "timed-out"
        );
    }

    #[tokio::test]
    async fn clear_returns_to_the_never_resolving_state() {
        let mut outcome = timeout_outcome(Some(Duration::from_millis(1)), ());
        outcome.clear();
        assert!(!outcome.is_set());
        assert!(
            tokio::time::timeout(Duration::from_millis(2), outcome)
                .await
                .is_err()
        );
    }
}
