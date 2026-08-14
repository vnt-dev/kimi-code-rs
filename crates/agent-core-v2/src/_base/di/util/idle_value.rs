//! Process-global idle-time lazy value.
//!
//! Original: `packages/agent-core-v2/src/_base/di/util/idleValue.ts`,
//! `GlobalIdleValue`.

use parking_lot::{Condvar, Mutex};
use std::sync::Arc;
use std::{
    fmt,
    panic::{AssertUnwindSafe, catch_unwind},
    thread,
};

type Executor<T> = Box<dyn FnOnce() -> T + Send + 'static>;

struct State<T> {
    executor: Option<Executor<T>>,
    value: Option<Arc<T>>,
    failure: Option<String>,
    initialized: bool,
    started: bool,
    cancelled: bool,
}

/// Rust's typed equivalent of the source getter throwing an executor error.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IdleValueError {
    message: String,
}

impl fmt::Display for IdleValueError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for IdleValueError {}

/// Schedules a no-argument computation to run once at the next idle turn.
///
/// Browsers expose `requestIdleCallback`; Node falls back to a zero-delay
/// timer.  Rust has neither runtime-independent primitive, so the fallback is
/// a dedicated thread that yields once before claiming the computation.  A
/// caller of [`Self::value`] claims and runs it immediately, exactly as the
/// source getter cancels its idle handle and invokes the executor.
pub struct GlobalIdleValue<T> {
    state: Arc<(Mutex<State<T>>, Condvar)>,
}

impl<T> GlobalIdleValue<T>
where
    T: Send + Sync + 'static,
{
    pub fn new(executor: impl FnOnce() -> T + Send + 'static) -> Self {
        let state = Arc::new((
            Mutex::new(State {
                executor: Some(Box::new(executor)),
                value: None,
                failure: None,
                initialized: false,
                started: false,
                cancelled: false,
            }),
            Condvar::new(),
        ));
        let scheduled = Arc::clone(&state);
        thread::spawn(move || {
            // Mirrors Node's `setTimeout(callback)` fallback: do not execute
            // inline with construction, so immediate consumers retain the
            // chance to force eager execution through `value()`.
            thread::yield_now();
            claim_and_run(&scheduled, false);
        });
        Self { state }
    }

    /// Original: `GlobalIdleValue.dispose()`. Cancels only the scheduled idle
    /// callback; a later `value()` call still evaluates the original executor.
    pub fn dispose(&self) {
        let (state, _) = &*self.state;
        let mut state = state.lock();
        if !state.started && !state.initialized {
            state.cancelled = true;
        }
    }

    /// Original: `GlobalIdleValue.value`.
    ///
    /// The JavaScript getter throws stored executor errors. Rust surfaces them
    /// as a typed `Result` and shares the value through `Arc` so callers do not
    /// receive an invalid long-lived borrow from a synchronized lazy cell.
    pub fn value(&self) -> Result<Arc<T>, IdleValueError> {
        claim_and_run(&self.state, true);
        let (lock, ready) = &*self.state;
        let mut state = lock.lock();
        while !state.initialized {
            ready.wait(&mut state);
        }
        match (&state.value, &state.failure) {
            (Some(value), None) => Ok(Arc::clone(value)),
            (_, Some(message)) => Err(IdleValueError {
                message: message.clone(),
            }),
            _ => unreachable!("an initialized GlobalIdleValue has a result"),
        }
    }

    /// Original: `GlobalIdleValue.isInitialized`.
    pub fn is_initialized(&self) -> bool {
        self.state.0.lock().initialized
    }
}

fn claim_and_run<T>(state: &Arc<(Mutex<State<T>>, Condvar)>, force: bool)
where
    T: Send + Sync + 'static,
{
    let executor = {
        let (lock, _) = &**state;
        let mut state = lock.lock();
        if state.initialized || state.started || (!force && state.cancelled) {
            return;
        }
        state.started = true;
        state.executor.take()
    };
    let Some(executor) = executor else {
        return;
    };
    let result = catch_unwind(AssertUnwindSafe(executor));
    let (lock, ready) = &**state;
    let mut state = lock.lock();
    match result {
        Ok(value) => state.value = Some(Arc::new(value)),
        Err(payload) => {
            state.failure = Some(payload.downcast_ref::<&str>().map_or_else(
                || {
                    payload
                        .downcast_ref::<String>()
                        .cloned()
                        .unwrap_or_else(|| "Lazy value initialization failed".into())
                },
                |message| (*message).into(),
            ));
        }
    }
    state.initialized = true;
    ready.notify_all();
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    #[test]
    fn value_forces_single_initialization_and_shares_the_result() {
        let calls = Arc::new(AtomicUsize::new(0));
        let executor_calls = Arc::clone(&calls);
        let value = GlobalIdleValue::new(move || {
            executor_calls.fetch_add(1, Ordering::SeqCst);
            "ready".to_owned()
        });

        assert_eq!(&*value.value().unwrap(), "ready");
        assert_eq!(&*value.value().unwrap(), "ready");
        assert!(value.is_initialized());
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn dispose_cancels_idle_work_but_value_still_executes_it() {
        let calls = Arc::new(AtomicUsize::new(0));
        let executor_calls = Arc::clone(&calls);
        let value = GlobalIdleValue::new(move || {
            executor_calls.fetch_add(1, Ordering::SeqCst);
            42
        });
        value.dispose();

        assert_eq!(*value.value().unwrap(), 42);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn executor_panics_are_retained_as_failures() {
        let value = GlobalIdleValue::<()>::new(|| panic!("broken"));
        let error = value.value().unwrap_err();
        assert_eq!(error.to_string(), "broken");
        assert!(value.is_initialized());
    }
}
