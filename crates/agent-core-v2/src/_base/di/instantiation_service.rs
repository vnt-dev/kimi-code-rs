use parking_lot::Mutex;
use std::cell::RefCell;
use std::sync::{
    Arc, Weak,
    atomic::{AtomicBool, Ordering},
};

use super::{
    descriptors::SyncDescriptor,
    errors::DiError,
    instantiation::{
        ErasedServiceIdentifier, INSTANTIATION_SERVICE_ID, ServiceIdentifier, ServicesAccessor,
        ServicesAccessorExt,
    },
    lifecycle::{Disposable, DisposableHandle, DisposeResult, dispose_all},
    service_collection::{ServiceCollection, ServiceEntry, ServiceValue},
};

thread_local! {
    static RESOLUTION_STACK: RefCell<Vec<(usize, ErasedServiceIdentifier)>> = const { RefCell::new(Vec::new()) };
}

struct InstantiationInner {
    services: Mutex<ServiceCollection>,
    parent: Option<Weak<InstantiationInner>>,
    children: Mutex<Vec<Weak<InstantiationInner>>>,
    construction_order: Mutex<Vec<DisposableHandle>>,
    disposed: AtomicBool,
}

#[derive(Clone)]
pub struct InstantiationService {
    inner: Arc<InstantiationInner>,
}

impl Default for InstantiationService {
    fn default() -> Self {
        Self::new(ServiceCollection::new())
    }
}

impl InstantiationService {
    pub fn new(services: ServiceCollection) -> Self {
        let service = Self {
            inner: Arc::new(InstantiationInner {
                services: Mutex::new(services),
                parent: None,
                children: Mutex::new(Vec::new()),
                construction_order: Mutex::new(Vec::new()),
                disposed: AtomicBool::new(false),
            }),
        };
        service.register_self();
        service
    }

    fn register_self(&self) {
        self.inner
            .services
            .lock()
            .set_instance(INSTANTIATION_SERVICE_ID, Arc::new(self.clone()));
    }

    // Original: InstantiationService.invokeFunction(). The borrow prevents accessor escape.
    pub fn invoke_function<R>(
        &self,
        function: impl FnOnce(&dyn ServicesAccessor) -> R,
    ) -> Result<R, DiError> {
        self.assert_not_disposed()?;
        Ok(function(self))
    }

    pub fn create_instance<T>(&self, descriptor: SyncDescriptor<T>) -> Result<Arc<T>, DiError>
    where
        T: Send + Sync + 'static,
    {
        self.assert_not_disposed()?;
        descriptor.erase().instantiate(self).and_then(|created| {
            created
                .value
                .downcast::<T>()
                .map_err(|_| DiError::Factory("descriptor returned an incompatible type".into()))
        })
    }

    pub fn create_child(&self, services: ServiceCollection) -> Result<Self, DiError> {
        self.assert_not_disposed()?;
        let child = Self {
            inner: Arc::new(InstantiationInner {
                services: Mutex::new(services),
                parent: Some(Arc::downgrade(&self.inner)),
                children: Mutex::new(Vec::new()),
                construction_order: Mutex::new(Vec::new()),
                disposed: AtomicBool::new(false),
            }),
        };
        child.register_self();
        self.inner
            .children
            .lock()
            .push(Arc::downgrade(&child.inner));
        Ok(child)
    }

    pub fn get<T>(&self, id: ServiceIdentifier<T>) -> Result<Arc<T>, DiError>
    where
        T: Send + Sync + 'static,
    {
        ServicesAccessorExt::get(self, id)
    }

    fn resolve_entry(
        &self,
        id: ErasedServiceIdentifier,
    ) -> Option<(Arc<InstantiationInner>, ServiceEntry)> {
        let mut current = Some(Arc::clone(&self.inner));
        while let Some(inner) = current {
            let entry = inner.services.lock().get_entry(id).cloned();
            if let Some(entry) = entry {
                return Some((inner, entry));
            }
            current = inner.parent.as_ref().and_then(Weak::upgrade);
        }
        None
    }

    fn instantiate_and_cache(
        &self,
        id: ErasedServiceIdentifier,
        owner: Arc<InstantiationInner>,
        descriptor: super::descriptors::ErasedSyncDescriptor,
    ) -> Result<ServiceValue, DiError> {
        // MIGRATION-TODO:
        // Original: InstantiationService._createServiceInstance() delayed-proxy branch.
        // Temporary behavior: delayed descriptors are constructed when first resolved, rather
        // than returning a JavaScript Proxy that waits for its first property access.
        // Completion condition: introduce a type-safe lazy service facade with early-event wiring.
        let root_key = self.root_key();
        let _guard = ResolutionGuard::push(root_key, id)?;
        let owner_service = Self {
            inner: Arc::clone(&owner),
        };
        let created = descriptor.instantiate(&owner_service)?;
        if owner.disposed.load(Ordering::Acquire) {
            if let Some(disposable) = created.disposable {
                let _ = disposable.dispose();
            }
            return Err(DiError::Disposed);
        }

        let (value, redundant) = {
            let mut services = owner.services.lock();
            if let Some(ServiceEntry::Instance { value, .. }) = services.get_entry(id) {
                (Arc::clone(value), created.disposable)
            } else {
                let value = Arc::clone(&created.value);
                if let Some(disposable) = &created.disposable {
                    owner.construction_order.lock().push(Arc::clone(disposable));
                }
                services.set_erased(
                    id,
                    ServiceEntry::Instance {
                        value: created.value,
                        disposable: created.disposable,
                    },
                );
                (value, None)
            }
        };
        if let Some(redundant) = redundant {
            let _ = redundant.dispose();
        }
        Ok(value)
    }

    fn root_key(&self) -> usize {
        let mut root = Arc::clone(&self.inner);
        while let Some(parent) = root.parent.as_ref().and_then(Weak::upgrade) {
            root = parent;
        }
        Arc::as_ptr(&root) as usize
    }

    fn assert_not_disposed(&self) -> Result<(), DiError> {
        if self.inner.disposed.load(Ordering::Acquire) {
            Err(DiError::Disposed)
        } else {
            Ok(())
        }
    }
}

impl ServicesAccessor for InstantiationService {
    fn get_erased(&self, id: ErasedServiceIdentifier) -> Result<ServiceValue, DiError> {
        self.assert_not_disposed()?;
        let (owner, entry) = self.resolve_entry(id).ok_or(DiError::UnknownService(id))?;
        match entry {
            ServiceEntry::Instance { value, .. } => Ok(value),
            ServiceEntry::Descriptor(descriptor) => {
                self.instantiate_and_cache(id, owner, descriptor)
            }
        }
    }
}

impl Disposable for InstantiationService {
    // Original: InstantiationService.dispose(); children first, then reverse construction order.
    fn dispose(&self) -> DisposeResult {
        if self.inner.disposed.swap(true, Ordering::AcqRel) {
            return Ok(());
        }
        let children = std::mem::take(&mut *self.inner.children.lock())
            .into_iter()
            .filter_map(|child| child.upgrade())
            .map(|inner| Arc::new(Self { inner }) as DisposableHandle)
            .collect::<Vec<_>>();
        let mut own = std::mem::take(&mut *self.inner.construction_order.lock());
        own.reverse();
        dispose_all(children.into_iter().chain(own))
    }
}

struct ResolutionGuard {
    root_key: usize,
    id: ErasedServiceIdentifier,
}

impl ResolutionGuard {
    fn push(root_key: usize, id: ErasedServiceIdentifier) -> Result<Self, DiError> {
        let cycle = RESOLUTION_STACK.with(|stack| {
            let stack = stack.borrow();
            stack
                .iter()
                .position(|entry| *entry == (root_key, id))
                .map(|start| {
                    stack[start..]
                        .iter()
                        .map(|(_, id)| id.to_string())
                        .chain(std::iter::once(id.to_string()))
                        .collect::<Vec<_>>()
                })
        });
        if let Some(path) = cycle {
            return Err(DiError::CyclicDependency(path));
        }
        RESOLUTION_STACK.with(|stack| stack.borrow_mut().push((root_key, id)));
        Ok(Self { root_key, id })
    }
}

impl Drop for ResolutionGuard {
    fn drop(&mut self) {
        RESOLUTION_STACK.with(|stack| {
            let mut stack = stack.borrow_mut();
            if let Some(position) = stack
                .iter()
                .rposition(|entry| *entry == (self.root_key, self.id))
            {
                stack.remove(position);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;
    use crate::_base::di::lifecycle::to_disposable;

    const NUMBER: ServiceIdentifier<usize> = ServiceIdentifier::new("number");
    const TEXT: ServiceIdentifier<String> = ServiceIdentifier::new("text");

    #[test]
    fn lazily_creates_caches_and_resolves_dependencies() {
        let calls = Arc::new(AtomicUsize::new(0));
        let mut services = ServiceCollection::new();
        services.set_descriptor(NUMBER, SyncDescriptor::new(|_| Ok(41)));
        services.set_descriptor(
            TEXT,
            SyncDescriptor::new({
                let calls = Arc::clone(&calls);
                move |accessor| {
                    calls.fetch_add(1, Ordering::Relaxed);
                    let number = accessor.get(NUMBER)?;
                    Ok(format!("value-{}", *number + 1))
                }
            }),
        );
        let instantiation = InstantiationService::new(services);
        assert_eq!(instantiation.get(TEXT).unwrap().as_str(), "value-42");
        assert_eq!(instantiation.get(TEXT).unwrap().as_str(), "value-42");
        assert_eq!(calls.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn child_overrides_do_not_change_parent_owned_dependencies() {
        let mut parent_services = ServiceCollection::new();
        parent_services.set_instance(NUMBER, Arc::new(1));
        parent_services.set_descriptor(
            TEXT,
            SyncDescriptor::new(|accessor| Ok(accessor.get(NUMBER)?.to_string())),
        );
        let parent = InstantiationService::new(parent_services);
        let mut child_services = ServiceCollection::new();
        child_services.set_instance(NUMBER, Arc::new(2));
        let child = parent.create_child(child_services).unwrap();
        assert_eq!(child.get(NUMBER).unwrap().as_ref(), &2);
        assert_eq!(child.get(TEXT).unwrap().as_str(), "1");
    }

    #[test]
    fn reports_dependency_cycle_with_service_path() {
        let mut services = ServiceCollection::new();
        services.set_descriptor(
            NUMBER,
            SyncDescriptor::new(|accessor| {
                let _ = accessor.get(TEXT)?;
                Ok(1)
            }),
        );
        services.set_descriptor(
            TEXT,
            SyncDescriptor::new(|accessor| {
                let _ = accessor.get(NUMBER)?;
                Ok(String::new())
            }),
        );
        let error = InstantiationService::new(services).get(NUMBER).unwrap_err();
        assert_eq!(
            error.to_string(),
            "Cyclic DI dependency detected: number → text → number"
        );
    }

    #[test]
    fn disposes_children_then_owned_services_in_reverse_order() {
        let order = Arc::new(Mutex::new(Vec::new()));
        let descriptor = |name: &'static str, order: Arc<Mutex<Vec<&'static str>>>| {
            SyncDescriptor::new(move |_| Ok(name)).managed(move |_| {
                let order = Arc::clone(&order);
                to_disposable(move || order.lock().push(name))
            })
        };
        let mut parent_services = ServiceCollection::new();
        parent_services.set_descriptor(TEXT.refine(), descriptor("parent", Arc::clone(&order)));
        let parent = InstantiationService::new(parent_services);
        let mut child_services = ServiceCollection::new();
        child_services.set_descriptor(NUMBER.refine(), descriptor("child", Arc::clone(&order)));
        let child = parent.create_child(child_services).unwrap();
        let _ = parent.get(TEXT.refine::<&'static str>()).unwrap();
        let _ = child.get(NUMBER.refine::<&'static str>()).unwrap();
        parent.dispose().unwrap();
        assert_eq!(*order.lock(), vec!["child", "parent"]);
    }
}
