use std::collections::BTreeMap;
use std::sync::{Arc, Weak, atomic::{AtomicBool, AtomicU64, Ordering}};
use parking_lot::Mutex;

use futures_util::future::{BoxFuture, join_all};
use tokio::sync::Mutex as AsyncMutex;
use tokio_util::sync::CancellationToken;

use super::{
    di::lifecycle::{
        Disposable, DisposableHandle, DisposableStore, combined_disposable, disposable_none,
        to_disposable,
    },
    errors::unexpected_error::{on_unexpected_error, safely_call_listener},
    lifecycle::lifecycle_machine::BoxError,
};

pub type Listener<T> = Arc<dyn Fn(&T) + Send + Sync>;

pub struct Event<T> {
    subscribe: Arc<dyn Fn(Listener<T>) -> DisposableHandle + Send + Sync>,
}

impl<T> Clone for Event<T> {
    fn clone(&self) -> Self {
        Self {
            subscribe: Arc::clone(&self.subscribe),
        }
    }
}

impl<T> Event<T> {
    pub(crate) fn from_subscribe(
        subscribe: impl Fn(Listener<T>) -> DisposableHandle + Send + Sync + 'static,
    ) -> Self {
        Self {
            subscribe: Arc::new(subscribe),
        }
    }

    pub fn subscribe(&self, listener: impl Fn(&T) + Send + Sync + 'static) -> DisposableHandle {
        (self.subscribe)(Arc::new(listener))
    }

    pub fn none() -> Self
    where
        T: 'static,
    {
        Self {
            subscribe: Arc::new(|_| disposable_none()),
        }
    }

    pub fn once(&self) -> Self
    where
        T: Send + Sync + 'static,
    {
        let source = self.clone();
        Self {
            subscribe: Arc::new(move |listener| {
                let subscription = Arc::new(Mutex::new(None::<DisposableHandle>));
                let subscription_for_listener = Arc::clone(&subscription);
                let handle = source.subscribe(move |value| {
                    if let Some(subscription) = subscription_for_listener.lock().take() {
                        let _ = subscription.dispose();
                        listener(value);
                    }
                });
                *subscription.lock() = Some(Arc::clone(&handle));
                handle
            }),
        }
    }

    pub fn map<O>(&self, map: impl Fn(&T) -> O + Send + Sync + 'static) -> Event<O>
    where
        T: 'static,
        O: 'static,
    {
        let source = self.clone();
        let map = Arc::new(map);
        Event {
            subscribe: Arc::new(move |listener| {
                let map = Arc::clone(&map);
                source.subscribe(move |value| listener(&map(value)))
            }),
        }
    }

    pub fn filter(&self, predicate: impl Fn(&T) -> bool + Send + Sync + 'static) -> Self
    where
        T: 'static,
    {
        let source = self.clone();
        let predicate = Arc::new(predicate);
        Self {
            subscribe: Arc::new(move |listener| {
                let predicate = Arc::clone(&predicate);
                source.subscribe(move |value| {
                    if predicate(value) {
                        listener(value);
                    }
                })
            }),
        }
    }

    pub fn any(events: Vec<Self>) -> Self
    where
        T: 'static,
    {
        Self {
            subscribe: Arc::new(move |listener| {
                combined_disposable(
                    events
                        .iter()
                        .map(|event| {
                            let listener = Arc::clone(&listener);
                            event.subscribe(move |value| listener(value))
                        })
                        .collect(),
                )
            }),
        }
    }

    pub fn subscribe_in(
        &self,
        store: &DisposableStore,
        listener: impl Fn(&T) + Send + Sync + 'static,
    ) -> DisposableHandle {
        store.add(self.subscribe(listener))
    }
}

struct EmitterInner<T> {
    listeners: Mutex<BTreeMap<u64, Listener<T>>>,
    next_id: AtomicU64,
    disposed: AtomicBool,
}

pub struct Emitter<T> {
    inner: Arc<EmitterInner<T>>,
}

impl<T: Send + Sync + 'static> Default for Emitter<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: Send + Sync + 'static> Emitter<T> {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(EmitterInner {
                listeners: Mutex::new(BTreeMap::new()),
                next_id: AtomicU64::new(0),
                disposed: AtomicBool::new(false),
            }),
        }
    }

    pub fn event(&self) -> Event<T> {
        let weak = Arc::downgrade(&self.inner);
        Event {
            subscribe: Arc::new(move |listener| subscribe(&weak, listener)),
        }
    }

    // Original: packages/agent-core-v2/src/_base/event.ts, Emitter.fire().
    pub fn fire(&self, value: &T) {
        if self.inner.disposed.load(Ordering::Acquire) {
            return;
        }
        let listeners = self
            .inner
            .listeners
            .lock()
            .values()
            .cloned()
            .collect::<Vec<_>>();
        for listener in listeners {
            safely_call_listener(|| listener(value));
        }
    }

    pub fn is_disposed(&self) -> bool {
        self.inner.disposed.load(Ordering::Acquire)
    }
}

impl<T> Disposable for Emitter<T>
where
    T: Send + Sync + 'static,
{
    fn dispose(&self) -> super::di::lifecycle::DisposeResult {
        if !self.inner.disposed.swap(true, Ordering::AcqRel) {
            self.inner.listeners.lock().clear();
        }
        Ok(())
    }
}

fn subscribe<T: Send + Sync + 'static>(
    weak: &Weak<EmitterInner<T>>,
    listener: Listener<T>,
) -> DisposableHandle {
    let Some(inner) = weak.upgrade() else {
        return disposable_none();
    };
    if inner.disposed.load(Ordering::Acquire) {
        return disposable_none();
    }
    let id = inner.next_id.fetch_add(1, Ordering::Relaxed);
    inner.listeners.lock().insert(id, listener);
    let weak = Arc::downgrade(&inner);
    to_disposable(move || {
        if let Some(inner) = weak.upgrade()
            && !inner.disposed.load(Ordering::Acquire)
        {
            inner.listeners.lock().remove(&id);
        }
    })
}

pub enum Veto {
    Immediate(bool),
    Future(BoxFuture<'static, Result<bool, BoxError>>),
}

pub async fn handle_vetos(vetos: Vec<Veto>, on_error: impl Fn(&BoxError)) -> bool {
    if vetos
        .iter()
        .any(|veto| matches!(veto, Veto::Immediate(true)))
    {
        return true;
    }
    let futures = vetos.into_iter().filter_map(|veto| match veto {
        Veto::Immediate(_) => None,
        Veto::Future(future) => Some(future),
    });
    let mut result = false;
    for settled in join_all(futures).await {
        match settled {
            Ok(true) => result = true,
            Ok(false) => {}
            Err(error) => {
                on_error(&error);
                result = true;
            }
        }
    }
    result
}

pub type WaitUntilFuture = BoxFuture<'static, Result<(), BoxError>>;

pub struct AsyncEvent<T> {
    pub data: T,
    pub signal: CancellationToken,
    pending: Arc<Mutex<Option<Vec<WaitUntilFuture>>>>,
}

impl<T> AsyncEvent<T> {
    pub fn wait_until(&self, future: WaitUntilFuture) -> Result<(), &'static str> {
        let mut pending = self.pending.lock();
        match pending.as_mut() {
            Some(pending) => {
                pending.push(future);
                Ok(())
            }
            None => Err("waitUntil can NOT be called asynchronously"),
        }
    }
}

pub struct AsyncEmitter<T> {
    emitter: Emitter<AsyncEvent<T>>,
    delivery: AsyncMutex<()>,
}

impl<T> Default for AsyncEmitter<T>
where
    T: Clone + Send + Sync + 'static,
{
    fn default() -> Self {
        Self::new()
    }
}

impl<T> AsyncEmitter<T>
where
    T: Clone + Send + Sync + 'static,
{
    pub fn new() -> Self {
        Self {
            emitter: Emitter::new(),
            delivery: AsyncMutex::new(()),
        }
    }

    pub fn event(&self) -> Event<AsyncEvent<T>> {
        self.emitter.event()
    }

    // Original: AsyncEmitter.fireAsync(). Deliveries and waitUntil work are sequential.
    pub async fn fire_async(&self, data: T, signal: CancellationToken) {
        let _delivery = self.delivery.lock().await;
        if self.emitter.is_disposed() || signal.is_cancelled() {
            return;
        }
        let listeners = self
            .emitter
            .inner
            .listeners
            .lock()
            .values()
            .cloned()
            .collect::<Vec<_>>();
        for listener in listeners {
            if signal.is_cancelled() {
                break;
            }
            let pending = Arc::new(Mutex::new(Some(Vec::new())));
            let event = AsyncEvent {
                data: data.clone(),
                signal: signal.clone(),
                pending: Arc::clone(&pending),
            };
            safely_call_listener(|| listener(&event));
            let futures = pending.lock().take().unwrap_or_default();
            for result in join_all(futures).await {
                if let Err(error) = result {
                    on_unexpected_error(error.as_ref());
                }
            }
        }
    }
}

impl<T> Disposable for AsyncEmitter<T>
where
    T: Clone + Send + Sync + 'static,
{
    fn dispose(&self) -> super::di::lifecycle::DisposeResult {
        self.emitter.dispose()
    }
}

#[cfg(test)]
mod tests {
    use parking_lot::Mutex as StdMutex;

    use super::*;

    #[test]
    fn emitter_snapshot_preserves_order_removal_and_late_subscription() {
        let emitter = Arc::new(Emitter::new());
        let seen = Arc::new(StdMutex::new(Vec::new()));
        let subscription = Arc::new(StdMutex::new(None::<DisposableHandle>));
        let emitter_for_listener = Arc::clone(&emitter);
        let seen_first = Arc::clone(&seen);
        let subscription_first = Arc::clone(&subscription);
        let handle = emitter.event().subscribe(move |_| {
            seen_first.lock().push("first");
            if let Some(handle) = subscription_first.lock().take() {
                handle.dispose().unwrap();
            }
            let seen_late = Arc::clone(&seen_first);
            emitter_for_listener
                .event()
                .subscribe(move |_| seen_late.lock().push("late"));
        });
        *subscription.lock() = Some(handle);
        let seen_second = Arc::clone(&seen);
        emitter
            .event()
            .subscribe(move |_| seen_second.lock().push("second"));

        emitter.fire(&1);
        emitter.fire(&2);
        assert_eq!(
            *seen.lock(),
            vec!["first", "second", "second", "late"]
        );
    }

    #[test]
    fn once_map_filter_and_any_preserve_combinator_behavior() {
        let first = Emitter::new();
        let second = Emitter::new();
        let seen = Arc::new(StdMutex::new(Vec::new()));
        let output = Event::any(vec![first.event(), second.event()])
            .filter(|value| *value % 2 == 0)
            .map(|value| value * 2)
            .once();
        let captured = Arc::clone(&seen);
        output.subscribe(move |value| captured.lock().push(*value));
        first.fire(&1);
        second.fire(&2);
        first.fire(&4);
        assert_eq!(*seen.lock(), vec![4]);
    }

    #[tokio::test]
    async fn async_emitter_waits_for_each_listener_before_the_next() {
        let emitter = AsyncEmitter::new();
        let order = Arc::new(StdMutex::new(Vec::new()));
        let first = Arc::clone(&order);
        emitter.event().subscribe(move |event| {
            first.lock().push("listener-1");
            let first = Arc::clone(&first);
            event
                .wait_until(Box::pin(async move {
                    tokio::task::yield_now().await;
                    first.lock().push("wait-1");
                    Ok(())
                }))
                .unwrap();
        });
        let second = Arc::clone(&order);
        emitter
            .event()
            .subscribe(move |_| second.lock().push("listener-2"));
        emitter.fire_async((), CancellationToken::new()).await;
        assert_eq!(
            *order.lock(),
            vec!["listener-1", "wait-1", "listener-2"]
        );
    }
}
