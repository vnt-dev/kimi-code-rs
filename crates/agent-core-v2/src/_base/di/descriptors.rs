use std::sync::Arc;

use super::{
    errors::DiError,
    instantiation::ServicesAccessor,
    lifecycle::{Disposable, DisposableHandle},
    service_collection::ServiceValue,
};

pub type ServiceFactory =
    dyn Fn(&dyn ServicesAccessor) -> Result<InstantiatedService, DiError> + Send + Sync;
type TypedServiceFactory<T> =
    dyn Fn(&dyn ServicesAccessor) -> Result<Arc<T>, DiError> + Send + Sync;

#[derive(Clone)]
pub struct ErasedSyncDescriptor {
    factory: Arc<ServiceFactory>,
    pub supports_delayed_instantiation: bool,
}

impl ErasedSyncDescriptor {
    pub fn instantiate(
        &self,
        accessor: &dyn ServicesAccessor,
    ) -> Result<InstantiatedService, DiError> {
        (self.factory)(accessor)
    }
}

pub struct InstantiatedService {
    pub value: ServiceValue,
    pub disposable: Option<DisposableHandle>,
}

pub struct SyncDescriptor<T> {
    factory: Arc<TypedServiceFactory<T>>,
    disposer: Option<Arc<dyn Fn(Arc<T>) -> DisposableHandle + Send + Sync>>,
    pub supports_delayed_instantiation: bool,
}

impl<T> Clone for SyncDescriptor<T> {
    fn clone(&self) -> Self {
        Self {
            factory: Arc::clone(&self.factory),
            disposer: self.disposer.clone(),
            supports_delayed_instantiation: self.supports_delayed_instantiation,
        }
    }
}

impl<T> SyncDescriptor<T>
where
    T: Send + Sync + 'static,
{
    // Original SyncDescriptor constructor/staticArguments are represented by a capturing factory.
    pub fn new(
        factory: impl Fn(&dyn ServicesAccessor) -> Result<T, DiError> + Send + Sync + 'static,
    ) -> Self {
        Self {
            factory: Arc::new(move |accessor| factory(accessor).map(Arc::new)),
            disposer: None,
            supports_delayed_instantiation: false,
        }
    }

    pub fn from_arc(
        factory: impl Fn(&dyn ServicesAccessor) -> Result<Arc<T>, DiError> + Send + Sync + 'static,
    ) -> Self {
        Self {
            factory: Arc::new(factory),
            disposer: None,
            supports_delayed_instantiation: false,
        }
    }

    pub fn delayed(mut self) -> Self {
        self.supports_delayed_instantiation = true;
        self
    }

    pub fn managed(
        mut self,
        disposer: impl Fn(Arc<T>) -> DisposableHandle + Send + Sync + 'static,
    ) -> Self {
        self.disposer = Some(Arc::new(disposer));
        self
    }

    pub fn disposable(self) -> Self
    where
        T: Disposable,
    {
        self.managed(|service| service)
    }

    pub fn erase(self) -> ErasedSyncDescriptor {
        let factory = self.factory;
        let disposer = self.disposer;
        ErasedSyncDescriptor {
            factory: Arc::new(move |accessor| {
                factory(accessor).map(|service| InstantiatedService {
                    disposable: disposer
                        .as_ref()
                        .map(|dispose| dispose(Arc::clone(&service))),
                    value: service,
                })
            }),
            supports_delayed_instantiation: self.supports_delayed_instantiation,
        }
    }
}
