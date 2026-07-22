use std::sync::{Arc, Mutex, OnceLock, Weak};

use indexmap::IndexMap;

use super::{
    descriptors::{ErasedSyncDescriptor, SyncDescriptor},
    errors::DiError,
    instantiation::ServiceIdentifier,
    instantiation_service::InstantiationService,
    lifecycle::{Disposable, DisposeResult, dispose_all},
    service_collection::{ServiceCollection, ServiceEntry},
};

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
#[repr(u8)]
pub enum LifecycleScope {
    App = 0,
    Session = 1,
    Agent = 2,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum InstantiationType {
    #[default]
    Eager,
    Delayed,
}

#[derive(Clone)]
pub struct ScopedEntry {
    pub scope: LifecycleScope,
    pub id: super::instantiation::ErasedServiceIdentifier,
    pub descriptor: ErasedSyncDescriptor,
    pub domain: String,
}

fn scoped_registry() -> &'static Mutex<Vec<ScopedEntry>> {
    static REGISTRY: OnceLock<Mutex<Vec<ScopedEntry>>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(Vec::new()))
}

// Original registerScopedService(); Rust factories replace constructor decorators.
pub fn register_scoped_service<T>(
    scope: LifecycleScope,
    id: ServiceIdentifier<T>,
    descriptor: SyncDescriptor<T>,
    instantiation_type: InstantiationType,
    domain: impl Into<String>,
) where
    T: Send + Sync + 'static,
{
    let descriptor = match instantiation_type {
        InstantiationType::Eager => descriptor,
        InstantiationType::Delayed => descriptor.delayed(),
    };
    scoped_registry().lock().unwrap().push(ScopedEntry {
        scope,
        id: id.erase(),
        descriptor: descriptor.erase(),
        domain: domain.into(),
    });
}

pub fn get_scoped_service_descriptors(scope: LifecycleScope) -> Vec<ScopedEntry> {
    scoped_registry()
        .lock()
        .unwrap()
        .iter()
        .filter(|entry| entry.scope == scope)
        .cloned()
        .collect()
}

pub fn clear_scoped_registry_for_tests() {
    scoped_registry().lock().unwrap().clear();
}

#[derive(Clone, Default)]
pub struct ScopeOptions {
    pub id: Option<String>,
    pub extra: ServiceCollection,
}

struct ScopeInner {
    id: String,
    kind: LifecycleScope,
    instantiation: InstantiationService,
    parent: Option<Weak<ScopeInner>>,
    children: Mutex<IndexMap<String, Scope>>,
    disposed: std::sync::atomic::AtomicBool,
}

#[derive(Clone)]
pub struct Scope {
    inner: Arc<ScopeInner>,
}

#[derive(Clone)]
pub struct ScopeHandle {
    scope: Scope,
}

impl ScopeHandle {
    pub fn id(&self) -> &str {
        self.scope.id()
    }

    pub fn kind(&self) -> LifecycleScope {
        self.scope.kind()
    }

    pub fn get<T>(&self, id: ServiceIdentifier<T>) -> Result<Arc<T>, DiError>
    where
        T: Send + Sync + 'static,
    {
        self.scope.get(id)
    }
}

impl Disposable for ScopeHandle {
    fn dispose(&self) -> DisposeResult {
        self.scope.dispose()
    }
}

impl Scope {
    pub fn create_app(options: ScopeOptions) -> Self {
        let collection = build_collection(LifecycleScope::App, options.extra);
        Self {
            inner: Arc::new(ScopeInner {
                id: options.id.unwrap_or_else(|| "app".into()),
                kind: LifecycleScope::App,
                instantiation: InstantiationService::new(collection),
                parent: None,
                children: Mutex::new(IndexMap::new()),
                disposed: std::sync::atomic::AtomicBool::new(false),
            }),
        }
    }

    pub fn id(&self) -> &str {
        &self.inner.id
    }

    pub fn kind(&self) -> LifecycleScope {
        self.inner.kind
    }

    pub fn get<T>(&self, id: ServiceIdentifier<T>) -> Result<Arc<T>, DiError>
    where
        T: Send + Sync + 'static,
    {
        self.assert_not_disposed()?;
        self.inner.instantiation.get(id)
    }

    // Original: Scope.createChild(). Scope kinds must strictly descend.
    pub fn create_child(
        &self,
        kind: LifecycleScope,
        id: impl Into<String>,
        options: ScopeOptions,
    ) -> Result<Self, DiError> {
        self.assert_not_disposed()?;
        let id = id.into();
        if kind <= self.inner.kind {
            return Err(DiError::Factory(format!(
                "child scope kind {kind:?}({}) must be greater than parent kind {:?}({})",
                kind as u8, self.inner.kind, self.inner.kind as u8
            )));
        }
        let mut children = self.inner.children.lock().unwrap();
        if children.contains_key(&id) {
            return Err(DiError::Factory(format!(
                "Scope '{}' already has a child with id '{id}'",
                self.id()
            )));
        }
        let collection = build_collection(kind, options.extra);
        let child = Self {
            inner: Arc::new(ScopeInner {
                id: id.clone(),
                kind,
                instantiation: self.inner.instantiation.create_child(collection)?,
                parent: Some(Arc::downgrade(&self.inner)),
                children: Mutex::new(IndexMap::new()),
                disposed: std::sync::atomic::AtomicBool::new(false),
            }),
        };
        children.insert(id, child.clone());
        Ok(child)
    }

    pub fn to_handle(&self) -> ScopeHandle {
        ScopeHandle {
            scope: self.clone(),
        }
    }

    pub fn child_ids(&self) -> Vec<String> {
        self.inner
            .children
            .lock()
            .unwrap()
            .keys()
            .cloned()
            .collect()
    }

    fn assert_not_disposed(&self) -> Result<(), DiError> {
        if self
            .inner
            .disposed
            .load(std::sync::atomic::Ordering::Acquire)
        {
            Err(DiError::Factory(format!(
                "Scope '{}' has been disposed",
                self.id()
            )))
        } else {
            Ok(())
        }
    }
}

impl Disposable for Scope {
    // Original: Scope.dispose(); descendants are disposed before this scope's services.
    fn dispose(&self) -> DisposeResult {
        if self
            .inner
            .disposed
            .swap(true, std::sync::atomic::Ordering::AcqRel)
        {
            return Ok(());
        }
        let children = std::mem::take(&mut *self.inner.children.lock().unwrap())
            .into_values()
            .map(|child| Arc::new(child) as super::lifecycle::DisposableHandle)
            .collect::<Vec<_>>();
        let child_result = dispose_all(children);
        let own_result = self.inner.instantiation.dispose();
        if let Some(parent) = self.inner.parent.as_ref().and_then(Weak::upgrade) {
            parent.children.lock().unwrap().shift_remove(self.id());
        }
        child_result.and(own_result)
    }
}

fn build_collection(kind: LifecycleScope, extra: ServiceCollection) -> ServiceCollection {
    let mut collection = ServiceCollection::new();
    for entry in get_scoped_service_descriptors(kind) {
        collection.set_erased(entry.id, ServiceEntry::Descriptor(entry.descriptor));
    }
    for (id, entry) in extra.iter() {
        collection.set_erased(id, entry.clone());
    }
    collection
}

#[cfg(test)]
mod tests {
    use super::*;

    static TEST_LOCK: Mutex<()> = Mutex::new(());
    const APP_NAME: ServiceIdentifier<String> = ServiceIdentifier::new("appName");
    const SESSION_NAME: ServiceIdentifier<String> = ServiceIdentifier::new("sessionName");

    #[test]
    fn registry_builds_scopes_and_children_inherit_parent_services() {
        let _guard = TEST_LOCK.lock().unwrap();
        clear_scoped_registry_for_tests();
        register_scoped_service(
            LifecycleScope::App,
            APP_NAME,
            SyncDescriptor::new(|_| Ok("app".into())),
            InstantiationType::Eager,
            "app",
        );
        register_scoped_service(
            LifecycleScope::Session,
            SESSION_NAME,
            SyncDescriptor::new(|_| Ok("session".into())),
            InstantiationType::Delayed,
            "session",
        );
        let app = Scope::create_app(ScopeOptions::default());
        let session = app
            .create_child(LifecycleScope::Session, "one", ScopeOptions::default())
            .unwrap();
        assert_eq!(session.get(APP_NAME).unwrap().as_str(), "app");
        assert_eq!(session.get(SESSION_NAME).unwrap().as_str(), "session");
        assert_eq!(app.child_ids(), vec!["one"]);
        session.dispose().unwrap();
        assert!(app.child_ids().is_empty());
        clear_scoped_registry_for_tests();
    }

    #[test]
    fn rejects_duplicate_and_non_descending_children() {
        let _guard = TEST_LOCK.lock().unwrap();
        clear_scoped_registry_for_tests();
        let app = Scope::create_app(ScopeOptions::default());
        let _session = app
            .create_child(LifecycleScope::Session, "one", ScopeOptions::default())
            .unwrap();
        assert!(
            app.create_child(LifecycleScope::Session, "one", ScopeOptions::default())
                .is_err()
        );
        assert!(
            app.create_child(LifecycleScope::App, "bad", ScopeOptions::default())
                .is_err()
        );
    }
}
