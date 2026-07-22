use std::{
    error::Error,
    fmt,
    sync::{Arc, Mutex},
};

pub type DisposableHandle = Arc<dyn Disposable>;
pub type DisposeResult = Result<(), DisposeError>;

pub trait Disposable: Send + Sync {
    fn dispose(&self) -> DisposeResult;
}

#[derive(Debug)]
pub struct DisposeError {
    message: String,
    errors: Vec<Box<dyn Error + Send + Sync>>,
}

impl DisposeError {
    pub fn single(error: impl Error + Send + Sync + 'static) -> Self {
        Self {
            message: error.to_string(),
            errors: vec![Box::new(error)],
        }
    }

    pub fn aggregate(errors: Vec<Box<dyn Error + Send + Sync>>) -> Self {
        let message = if errors.len() == 1 {
            errors[0].to_string()
        } else {
            "Encountered errors while disposing of store".into()
        };
        Self { message, errors }
    }

    pub fn errors(&self) -> &[Box<dyn Error + Send + Sync>] {
        &self.errors
    }
}

impl fmt::Display for DisposeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for DisposeError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        self.errors.first().map(|error| error.as_ref() as _)
    }
}

struct FunctionDisposable {
    inner: Mutex<FunctionDisposableState>,
}

struct FunctionDisposableState {
    disposed: bool,
    function: Option<Box<dyn FnOnce() -> DisposeResult + Send>>,
}

impl Disposable for FunctionDisposable {
    fn dispose(&self) -> DisposeResult {
        let function = {
            let mut inner = self.inner.lock().unwrap();
            if inner.disposed {
                return Ok(());
            }
            inner.disposed = true;
            inner.function.take()
        };
        function.map_or(Ok(()), |function| function())
    }
}

// Original: packages/agent-core-v2/src/_base/di/lifecycle.ts, toDisposable().
pub fn to_disposable(function: impl FnOnce() + Send + 'static) -> DisposableHandle {
    to_fallible_disposable(move || {
        function();
        Ok(())
    })
}

pub fn to_fallible_disposable(
    function: impl FnOnce() -> DisposeResult + Send + 'static,
) -> DisposableHandle {
    Arc::new(FunctionDisposable {
        inner: Mutex::new(FunctionDisposableState {
            disposed: false,
            function: Some(Box::new(function)),
        }),
    })
}

pub fn disposable_none() -> DisposableHandle {
    to_disposable(|| {})
}

pub fn dispose_all(disposables: impl IntoIterator<Item = DisposableHandle>) -> DisposeResult {
    let mut errors = Vec::new();
    for disposable in disposables {
        if let Err(error) = disposable.dispose() {
            errors.extend(error.errors);
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(DisposeError::aggregate(errors))
    }
}

pub fn combined_disposable(disposables: Vec<DisposableHandle>) -> DisposableHandle {
    to_fallible_disposable(move || dispose_all(disposables))
}

#[derive(Default)]
pub struct DisposableStore {
    inner: Mutex<DisposableStoreState>,
}

#[derive(Default)]
struct DisposableStoreState {
    disposables: Vec<DisposableHandle>,
    disposed: bool,
}

impl DisposableStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add(&self, disposable: DisposableHandle) -> DisposableHandle {
        let should_dispose = {
            let mut inner = self.inner.lock().unwrap();
            if inner.disposed {
                true
            } else {
                if !inner
                    .disposables
                    .iter()
                    .any(|current| Arc::ptr_eq(current, &disposable))
                {
                    inner.disposables.push(Arc::clone(&disposable));
                }
                false
            }
        };
        if should_dispose {
            let _ = disposable.dispose();
        }
        disposable
    }

    pub fn delete(&self, disposable: &DisposableHandle) -> DisposeResult {
        let removed = {
            let mut inner = self.inner.lock().unwrap();
            if inner.disposed {
                return Ok(());
            }
            inner
                .disposables
                .iter()
                .position(|current| Arc::ptr_eq(current, disposable))
                .map(|index| inner.disposables.remove(index))
        };
        removed.map_or(Ok(()), |disposable| disposable.dispose())
    }

    pub fn delete_and_leak(&self, disposable: &DisposableHandle) -> bool {
        let mut inner = self.inner.lock().unwrap();
        if inner.disposed {
            return false;
        }
        let Some(index) = inner
            .disposables
            .iter()
            .position(|current| Arc::ptr_eq(current, disposable))
        else {
            return false;
        };
        inner.disposables.remove(index);
        true
    }

    pub fn clear(&self) -> DisposeResult {
        let disposables = std::mem::take(&mut self.inner.lock().unwrap().disposables);
        dispose_all(disposables)
    }

    pub fn is_disposed(&self) -> bool {
        self.inner.lock().unwrap().disposed
    }
}

impl Disposable for DisposableStore {
    fn dispose(&self) -> DisposeResult {
        {
            let mut inner = self.inner.lock().unwrap();
            if inner.disposed {
                return Ok(());
            }
            inner.disposed = true;
        }
        self.clear()
    }
}

impl Drop for DisposableStore {
    fn drop(&mut self) {
        let _ = self.dispose();
    }
}

pub struct MutableDisposable {
    inner: Mutex<MutableDisposableState>,
}

#[derive(Default)]
struct MutableDisposableState {
    value: Option<DisposableHandle>,
    disposed: bool,
}

impl Default for MutableDisposable {
    fn default() -> Self {
        Self {
            inner: Mutex::new(MutableDisposableState::default()),
        }
    }
}

impl MutableDisposable {
    pub fn value(&self) -> Option<DisposableHandle> {
        let inner = self.inner.lock().unwrap();
        (!inner.disposed).then(|| inner.value.clone()).flatten()
    }

    pub fn set(&self, value: Option<DisposableHandle>) -> DisposeResult {
        let previous = {
            let mut inner = self.inner.lock().unwrap();
            if inner.disposed {
                drop(inner);
                return value.map_or(Ok(()), |value| value.dispose());
            }
            if matches!((&inner.value, &value), (Some(left), Some(right)) if Arc::ptr_eq(left, right))
            {
                return Ok(());
            }
            std::mem::replace(&mut inner.value, value)
        };
        previous.map_or(Ok(()), |value| value.dispose())
    }

    pub fn clear(&self) -> DisposeResult {
        self.set(None)
    }

    pub fn clear_and_leak(&self) -> Option<DisposableHandle> {
        let mut inner = self.inner.lock().unwrap();
        if inner.disposed {
            None
        } else {
            inner.value.take()
        }
    }
}

impl Disposable for MutableDisposable {
    fn dispose(&self) -> DisposeResult {
        let previous = {
            let mut inner = self.inner.lock().unwrap();
            if inner.disposed {
                return Ok(());
            }
            inner.disposed = true;
            inner.value.take()
        };
        previous.map_or(Ok(()), |value| value.dispose())
    }
}

pub struct RefCountedDisposable {
    inner: Mutex<RefCountedState>,
}

struct RefCountedState {
    count: usize,
    disposable: Option<DisposableHandle>,
}

impl RefCountedDisposable {
    pub fn new(disposable: DisposableHandle) -> Self {
        Self {
            inner: Mutex::new(RefCountedState {
                count: 1,
                disposable: Some(disposable),
            }),
        }
    }

    pub fn acquire(&self) {
        self.inner.lock().unwrap().count += 1;
    }

    pub fn release(&self) -> DisposeResult {
        let disposable = {
            let mut inner = self.inner.lock().unwrap();
            if inner.count == 0 {
                return Ok(());
            }
            inner.count -= 1;
            (inner.count == 0)
                .then(|| inner.disposable.take())
                .flatten()
        };
        disposable.map_or(Ok(()), |disposable| disposable.dispose())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    #[test]
    fn function_disposable_is_idempotent_and_store_disposes_in_insertion_order() {
        let order = Arc::new(Mutex::new(Vec::new()));
        let store = DisposableStore::new();
        for label in ["first", "second"] {
            let order = Arc::clone(&order);
            store.add(to_disposable(move || order.lock().unwrap().push(label)));
        }
        store.dispose().unwrap();
        store.dispose().unwrap();
        assert_eq!(*order.lock().unwrap(), vec!["first", "second"]);
    }

    #[test]
    fn adding_after_disposal_disposes_immediately_and_mutable_replaces() {
        let count = Arc::new(AtomicUsize::new(0));
        let store = DisposableStore::new();
        store.dispose().unwrap();
        let count_on_drop = Arc::clone(&count);
        store.add(to_disposable(move || {
            count_on_drop.fetch_add(1, Ordering::Relaxed);
        }));

        let mutable = MutableDisposable::default();
        let first = Arc::clone(&count);
        mutable
            .set(Some(to_disposable(move || {
                first.fetch_add(1, Ordering::Relaxed);
            })))
            .unwrap();
        mutable.set(Some(disposable_none())).unwrap();
        mutable.dispose().unwrap();
        assert_eq!(count.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn combined_disposal_attempts_every_child_and_aggregates_errors() {
        let visited = Arc::new(AtomicUsize::new(0));
        let disposables = (0..2)
            .map(|index| {
                let visited = Arc::clone(&visited);
                to_fallible_disposable(move || {
                    visited.fetch_add(1, Ordering::Relaxed);
                    Err(DisposeError::single(std::io::Error::other(format!(
                        "failure-{index}"
                    ))))
                })
            })
            .collect();
        let error = combined_disposable(disposables).dispose().unwrap_err();
        assert_eq!(visited.load(Ordering::Relaxed), 2);
        assert_eq!(error.errors().len(), 2);
    }
}
