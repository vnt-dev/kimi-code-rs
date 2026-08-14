use parking_lot::Mutex;
use std::collections::VecDeque;
use std::sync::Arc;

use futures_util::{Stream, stream};
use tokio::sync::oneshot;

pub struct AsyncEventQueue<T, E> {
    state: Mutex<QueueState<T, E>>,
}

struct QueueState<T, E> {
    values: VecDeque<T>,
    waiters: VecDeque<oneshot::Sender<Result<Option<T>, E>>>,
    failure: Option<E>,
    ended: bool,
}

impl<T, E> Default for AsyncEventQueue<T, E> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T, E> AsyncEventQueue<T, E> {
    pub fn new() -> Self {
        Self {
            state: Mutex::new(QueueState {
                values: VecDeque::new(),
                waiters: VecDeque::new(),
                failure: None,
                ended: false,
            }),
        }
    }

    // Original:
    //   packages/agent-core-v2/src/_base/asyncEventQueue.ts
    //   AsyncEventQueue.push()
    //
    // Rust adaptation: a cancelled `next()` drops its oneshot receiver. Send
    // failure returns ownership of the value, so it is offered to the next
    // waiter or buffered instead of being lost.
    pub fn push(&self, mut value: T) {
        let mut state = self.state.lock();
        if state.failure.is_some() || state.ended {
            return;
        }
        while let Some(waiter) = state.waiters.pop_front() {
            match waiter.send(Ok(Some(value))) {
                Ok(()) => return,
                Err(Ok(Some(returned))) => value = returned,
                Err(Ok(None) | Err(_)) => unreachable!("push sends a value result"),
            }
        }
        state.values.push_back(value);
    }

    // Original:
    //   packages/agent-core-v2/src/_base/asyncEventQueue.ts
    //   AsyncEventQueue.end()
    pub fn end(&self) {
        let mut state = self.state.lock();
        if state.failure.is_some() || state.ended {
            return;
        }
        state.ended = true;
        for waiter in state.waiters.drain(..) {
            let _ = waiter.send(Ok(None));
        }
    }

    // Original:
    //   packages/agent-core-v2/src/_base/asyncEventQueue.ts
    //   AsyncEventQueue.fail()
    pub fn fail(&self, error: E)
    where
        E: Clone,
    {
        let mut state = self.state.lock();
        if state.failure.is_some() || state.ended {
            return;
        }
        state.failure = Some(error.clone());
        if !state.values.is_empty() {
            return;
        }
        for waiter in state.waiters.drain(..) {
            let _ = waiter.send(Err(error.clone()));
        }
    }

    // Original:
    //   packages/agent-core-v2/src/_base/asyncEventQueue.ts
    //   AsyncEventQueue.next()
    pub async fn next(&self) -> Result<Option<T>, E>
    where
        E: Clone,
    {
        let receiver = {
            let mut state = self.state.lock();
            if let Some(value) = state.values.pop_front() {
                return Ok(Some(value));
            }
            if let Some(error) = &state.failure {
                return Err(error.clone());
            }
            if state.ended {
                return Ok(None);
            }
            let (sender, receiver) = oneshot::channel();
            state.waiters.push_back(sender);
            receiver
        };

        match receiver.await {
            Ok(result) => result,
            Err(_) => unreachable!("next() borrows the queue until its waiter resolves"),
        }
    }

    /// Rust async-iteration adapter for the source `Symbol.asyncIterator`.
    /// Provider failure is yielded once as `Err(E)` and then terminates the
    /// stream, matching an async iterator whose `next()` rejects.
    pub fn into_stream(self: Arc<Self>) -> impl Stream<Item = Result<T, E>>
    where
        T: 'static,
        E: Clone + 'static,
    {
        stream::unfold(Some(self), |queue| async move {
            let queue = queue?;
            match queue.next().await {
                Ok(Some(value)) => Some((Ok(value), Some(queue))),
                Ok(None) => None,
                Err(error) => Some((Err(error), None)),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::StreamExt;

    #[tokio::test]
    async fn buffered_values_are_delivered_in_push_order() {
        let queue = AsyncEventQueue::<_, String>::new();
        queue.push(1);
        queue.push(2);
        assert_eq!(queue.next().await, Ok(Some(1)));
        assert_eq!(queue.next().await, Ok(Some(2)));
    }

    #[tokio::test]
    async fn pending_waiter_receives_the_next_value_directly() {
        let queue = Arc::new(AsyncEventQueue::<_, String>::new());
        let waiting_queue = Arc::clone(&queue);
        let waiting = tokio::spawn(async move { waiting_queue.next().await });
        tokio::task::yield_now().await;
        queue.push("value");
        assert_eq!(waiting.await.unwrap(), Ok(Some("value")));
    }

    #[tokio::test]
    async fn end_resolves_waiters_and_ignores_later_operations() {
        let queue = Arc::new(AsyncEventQueue::<i32, String>::new());
        let waiting_queue = Arc::clone(&queue);
        let waiting = tokio::spawn(async move { waiting_queue.next().await });
        tokio::task::yield_now().await;
        queue.end();
        queue.end();
        queue.push(1);
        queue.fail("late".to_owned());

        assert_eq!(waiting.await.unwrap(), Ok(None));
        assert_eq!(queue.next().await, Ok(None));
    }

    #[tokio::test]
    async fn failure_is_reported_only_after_buffered_values_are_drained() {
        let queue = AsyncEventQueue::new();
        queue.push("first");
        queue.push("second");
        queue.fail("boom");

        assert_eq!(queue.next().await, Ok(Some("first")));
        assert_eq!(queue.next().await, Ok(Some("second")));
        assert_eq!(queue.next().await, Err("boom"));
    }

    #[tokio::test]
    async fn fail_rejects_all_pending_waiters() {
        let queue = Arc::new(AsyncEventQueue::<i32, _>::new());
        let first_queue = Arc::clone(&queue);
        let second_queue = Arc::clone(&queue);
        let first = tokio::spawn(async move { first_queue.next().await });
        let second = tokio::spawn(async move { second_queue.next().await });
        tokio::task::yield_now().await;
        queue.fail("boom");

        assert_eq!(first.await.unwrap(), Err("boom"));
        assert_eq!(second.await.unwrap(), Err("boom"));
    }

    #[tokio::test]
    async fn a_cancelled_waiter_does_not_consume_a_later_value() {
        let queue = Arc::new(AsyncEventQueue::<i32, String>::new());
        let cancelled_queue = Arc::clone(&queue);
        let cancelled = tokio::spawn(async move { cancelled_queue.next().await });
        tokio::task::yield_now().await;
        cancelled.abort();
        let _ = cancelled.await;

        queue.push(42);

        assert_eq!(queue.next().await, Ok(Some(42)));
    }

    #[tokio::test]
    async fn stream_adapter_yields_buffer_then_one_failure_and_terminates() {
        let queue = Arc::new(AsyncEventQueue::new());
        queue.push(1);
        queue.push(2);
        queue.fail("boom");

        let values = queue.into_stream().collect::<Vec<_>>().await;

        assert_eq!(values, [Ok(1), Ok(2), Err("boom")]);
    }
}
