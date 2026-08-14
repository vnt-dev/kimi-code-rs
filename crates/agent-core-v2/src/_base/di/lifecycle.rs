use std::{
    collections::HashMap,
    error::Error,
    fmt,
    hash::Hash,
};
use std::sync::{Arc};
use parking_lot::Mutex;

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
            let mut inner = self.inner.lock();
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
            let mut inner = self.inner.lock();
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
            let mut inner = self.inner.lock();
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
        let mut inner = self.inner.lock();
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
        let disposables = std::mem::take(&mut self.inner.lock().disposables);
        dispose_all(disposables)
    }

    pub fn is_disposed(&self) -> bool {
        self.inner.lock().disposed
    }
}

impl Disposable for DisposableStore {
    fn dispose(&self) -> DisposeResult {
        {
            let mut inner = self.inner.lock();
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
        let inner = self.inner.lock();
        (!inner.disposed).then(|| inner.value.clone()).flatten()
    }

    pub fn set(&self, value: Option<DisposableHandle>) -> DisposeResult {
        let previous = {
            let mut inner = self.inner.lock();
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
        let mut inner = self.inner.lock();
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
            let mut inner = self.inner.lock();
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

pub struct MandatoryMutableDisposable {
    value: MutableDisposable,
}

impl MandatoryMutableDisposable {
    pub fn new(initial_value: DisposableHandle) -> Self {
        let value = MutableDisposable::default();
        // A fresh MutableDisposable cannot reject the initial value.
        let _ = value.set(Some(initial_value));
        Self { value }
    }

    pub fn value(&self) -> Option<DisposableHandle> {
        self.value.value()
    }

    pub fn set(&self, value: DisposableHandle) -> DisposeResult {
        self.value.set(Some(value))
    }
}

impl Disposable for MandatoryMutableDisposable {
    fn dispose(&self) -> DisposeResult {
        self.value.dispose()
    }
}

pub struct DisposableMap<K> {
    inner: Mutex<DisposableMapState<K>>,
}

struct DisposableMapState<K> {
    values: HashMap<K, DisposableHandle>,
    disposed: bool,
}

impl<K> Default for DisposableMap<K> {
    fn default() -> Self {
        Self {
            inner: Mutex::new(DisposableMapState {
                values: HashMap::new(),
                disposed: false,
            }),
        }
    }
}

impl<K> DisposableMap<K>
where
    K: Eq + Hash,
{
    pub fn len(&self) -> usize {
        self.inner.lock().values.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn has(&self, key: &K) -> bool {
        self.inner.lock().values.contains_key(key)
    }

    pub fn get(&self, key: &K) -> Option<DisposableHandle> {
        self.inner.lock().values.get(key).cloned()
    }

    pub fn set(
        &self,
        key: K,
        value: DisposableHandle,
        skip_dispose_on_overwrite: bool,
    ) -> DisposeResult {
        let previous = {
            let mut inner = self.inner.lock();
            if inner.disposed {
                // Source deliberately warns and leaks values added after disposal.
                return Ok(());
            }
            let previous = inner.values.insert(key, Arc::clone(&value));
            if skip_dispose_on_overwrite
                || previous
                    .as_ref()
                    .is_some_and(|previous| Arc::ptr_eq(previous, &value))
            {
                None
            } else {
                previous
            }
        };
        previous.map_or(Ok(()), |previous| previous.dispose())
    }

    pub fn delete_and_dispose(&self, key: &K) -> DisposeResult {
        self.inner
            .lock()
            .values
            .remove(key)
            .map_or(Ok(()), |value| value.dispose())
    }

    pub fn delete_and_leak(&self, key: &K) -> Option<DisposableHandle> {
        self.inner.lock().values.remove(key)
    }

    pub fn clear_and_dispose_all(&self) -> DisposeResult {
        let values = std::mem::take(&mut self.inner.lock().values).into_values();
        dispose_all(values)
    }
}

impl<K> Disposable for DisposableMap<K>
where
    K: Eq + Hash + Send,
{
    fn dispose(&self) -> DisposeResult {
        {
            let mut inner = self.inner.lock();
            if inner.disposed {
                return Ok(());
            }
            inner.disposed = true;
        }
        self.clear_and_dispose_all()
    }
}

#[derive(Default)]
pub struct DisposableSet {
    inner: Mutex<DisposableSetState>,
}

#[derive(Default)]
struct DisposableSetState {
    values: Vec<DisposableHandle>,
    disposed: bool,
}

impl DisposableSet {
    pub fn len(&self) -> usize {
        self.inner.lock().values.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn add(&self, value: DisposableHandle) {
        let mut inner = self.inner.lock();
        if inner.disposed {
            return;
        }
        if !inner
            .values
            .iter()
            .any(|current| Arc::ptr_eq(current, &value))
        {
            inner.values.push(value);
        }
    }

    pub fn delete_and_dispose(&self, value: &DisposableHandle) -> DisposeResult {
        let removed = {
            let mut inner = self.inner.lock();
            inner
                .values
                .iter()
                .position(|current| Arc::ptr_eq(current, value))
                .map(|index| inner.values.remove(index))
        };
        removed.map_or(Ok(()), |value| value.dispose())
    }

    pub fn delete_and_leak(&self, value: &DisposableHandle) -> Option<DisposableHandle> {
        let mut inner = self.inner.lock();
        let index = inner
            .values
            .iter()
            .position(|current| Arc::ptr_eq(current, value))?;
        Some(inner.values.remove(index))
    }

    pub fn clear_and_dispose_all(&self) -> DisposeResult {
        dispose_all(std::mem::take(&mut self.inner.lock().values))
    }
}

impl Disposable for DisposableSet {
    fn dispose(&self) -> DisposeResult {
        {
            let mut inner = self.inner.lock();
            if inner.disposed {
                return Ok(());
            }
            inner.disposed = true;
        }
        self.clear_and_dispose_all()
    }
}

type CreateReference<T> = dyn Fn(&str) -> Result<T, Box<dyn Error + Send + Sync>> + Send + Sync;
type DestroyReference<T> = dyn Fn(&str, &T) -> DisposeResult + Send + Sync;

struct ReferenceEntry<T> {
    object: Arc<T>,
    count: usize,
}

struct ReferenceState<T> {
    values: Mutex<HashMap<String, ReferenceEntry<T>>>,
    destroy: Arc<DestroyReference<T>>,
}

pub struct ReferenceCollection<T> {
    state: Arc<ReferenceState<T>>,
    create: Arc<CreateReference<T>>,
}

impl<T> ReferenceCollection<T>
where
    T: Send + Sync + 'static,
{
    pub fn new(
        create: impl Fn(&str) -> Result<T, Box<dyn Error + Send + Sync>> + Send + Sync + 'static,
        destroy: impl Fn(&str, &T) -> DisposeResult + Send + Sync + 'static,
    ) -> Self {
        Self {
            state: Arc::new(ReferenceState {
                values: Mutex::new(HashMap::new()),
                destroy: Arc::new(destroy),
            }),
            create: Arc::new(create),
        }
    }

    // Original: ReferenceCollection.acquire(). Creation occurs once per live key.
    pub fn acquire(
        &self,
        key: impl Into<String>,
    ) -> Result<Reference<T>, Box<dyn Error + Send + Sync>> {
        let key = key.into();
        let object = {
            let mut values = self.state.values.lock();
            if let Some(reference) = values.get_mut(&key) {
                reference.count += 1;
                Arc::clone(&reference.object)
            } else {
                let object = Arc::new((self.create)(&key)?);
                values.insert(
                    key.clone(),
                    ReferenceEntry {
                        object: Arc::clone(&object),
                        count: 1,
                    },
                );
                object
            }
        };
        let state = Arc::downgrade(&self.state);
        let release_key = key;
        let release = to_fallible_disposable(move || {
            let Some(state) = state.upgrade() else {
                return Ok(());
            };
            let removed = {
                let mut values = state.values.lock();
                let Some(reference) = values.get_mut(&release_key) else {
                    return Ok(());
                };
                reference.count -= 1;
                (reference.count == 0)
                    .then(|| values.remove(&release_key))
                    .flatten()
            };
            removed.map_or(Ok(()), |entry| (state.destroy)(&release_key, &entry.object))
        });
        Ok(Reference { object, release })
    }
}

pub struct Reference<T> {
    pub object: Arc<T>,
    release: DisposableHandle,
}

impl<T> Disposable for Reference<T>
where
    T: Send + Sync,
{
    fn dispose(&self) -> DisposeResult {
        self.release.dispose()
    }
}

pub struct ImmortalReference<T> {
    pub object: Arc<T>,
}

impl<T> ImmortalReference<T> {
    pub fn new(object: T) -> Self {
        Self {
            object: Arc::new(object),
        }
    }
}

impl<T> Disposable for ImmortalReference<T>
where
    T: Send + Sync,
{
    fn dispose(&self) -> DisposeResult {
        Ok(())
    }
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
        self.inner.lock().count += 1;
    }

    pub fn release(&self) -> DisposeResult {
        let disposable = {
            let mut inner = self.inner.lock();
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
            store.add(to_disposable(move || order.lock().push(label)));
        }
        store.dispose().unwrap();
        store.dispose().unwrap();
        assert_eq!(*order.lock(), vec!["first", "second"]);
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

    #[test]
    fn disposable_collections_replace_remove_leak_and_clear() {
        let count = Arc::new(AtomicUsize::new(0));
        let make = || {
            let count = Arc::clone(&count);
            to_disposable(move || {
                count.fetch_add(1, Ordering::Relaxed);
            })
        };
        let map = DisposableMap::default();
        map.set("key", make(), false).unwrap();
        map.set("key", make(), false).unwrap();
        assert_eq!(count.load(Ordering::Relaxed), 1);
        let leaked = map.delete_and_leak(&"key").unwrap();
        assert_eq!(count.load(Ordering::Relaxed), 1);
        leaked.dispose().unwrap();

        let set = DisposableSet::default();
        let value = make();
        set.add(Arc::clone(&value));
        set.add(Arc::clone(&value));
        assert_eq!(set.len(), 1);
        set.dispose().unwrap();
        assert_eq!(count.load(Ordering::Relaxed), 3);
    }

    #[test]
    fn reference_collection_creates_once_and_destroys_after_last_release() {
        let creates = Arc::new(AtomicUsize::new(0));
        let destroys = Arc::new(AtomicUsize::new(0));
        let references = ReferenceCollection::new(
            {
                let creates = Arc::clone(&creates);
                move |key| {
                    creates.fetch_add(1, Ordering::Relaxed);
                    Ok(key.to_owned())
                }
            },
            {
                let destroys = Arc::clone(&destroys);
                move |_, _| {
                    destroys.fetch_add(1, Ordering::Relaxed);
                    Ok(())
                }
            },
        );
        let first = references.acquire("shared").unwrap();
        let second = references.acquire("shared").unwrap();
        assert!(Arc::ptr_eq(&first.object, &second.object));
        assert_eq!(creates.load(Ordering::Relaxed), 1);
        first.dispose().unwrap();
        assert_eq!(destroys.load(Ordering::Relaxed), 0);
        second.dispose().unwrap();
        assert_eq!(destroys.load(Ordering::Relaxed), 1);
    }
}
