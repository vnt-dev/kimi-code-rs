use std::sync::Arc;

use super::{errors::DiError, instantiation::ServicesAccessor, service_collection::ServiceValue};

pub type ServiceFactory =
    dyn Fn(&dyn ServicesAccessor) -> Result<ServiceValue, DiError> + Send + Sync;
type TypedServiceFactory<T> =
    dyn Fn(&dyn ServicesAccessor) -> Result<Arc<T>, DiError> + Send + Sync;

#[derive(Clone)]
pub struct ErasedSyncDescriptor {
    factory: Arc<ServiceFactory>,
    pub supports_delayed_instantiation: bool,
}

impl ErasedSyncDescriptor {
    pub fn instantiate(&self, accessor: &dyn ServicesAccessor) -> Result<ServiceValue, DiError> {
        (self.factory)(accessor)
    }
}

pub struct SyncDescriptor<T> {
    factory: Arc<TypedServiceFactory<T>>,
    pub supports_delayed_instantiation: bool,
}

impl<T> Clone for SyncDescriptor<T> {
    fn clone(&self) -> Self {
        Self {
            factory: Arc::clone(&self.factory),
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
            supports_delayed_instantiation: false,
        }
    }

    pub fn from_arc(
        factory: impl Fn(&dyn ServicesAccessor) -> Result<Arc<T>, DiError> + Send + Sync + 'static,
    ) -> Self {
        Self {
            factory: Arc::new(factory),
            supports_delayed_instantiation: false,
        }
    }

    pub fn delayed(mut self) -> Self {
        self.supports_delayed_instantiation = true;
        self
    }

    pub fn erase(self) -> ErasedSyncDescriptor {
        let factory = self.factory;
        ErasedSyncDescriptor {
            factory: Arc::new(move |accessor| {
                factory(accessor).map(|service| service as ServiceValue)
            }),
            supports_delayed_instantiation: self.supports_delayed_instantiation,
        }
    }
}
