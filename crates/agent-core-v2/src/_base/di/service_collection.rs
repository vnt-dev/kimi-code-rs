use std::{any::Any, sync::Arc};

use indexmap::IndexMap;

use super::{
    descriptors::{ErasedSyncDescriptor, SyncDescriptor},
    errors::DiError,
    instantiation::{ErasedServiceIdentifier, ServiceIdentifier},
};

pub type ServiceValue = Arc<dyn Any + Send + Sync>;

#[derive(Clone)]
pub enum ServiceEntry {
    Instance(ServiceValue),
    Descriptor(ErasedSyncDescriptor),
}

#[derive(Clone, Default)]
pub struct ServiceCollection {
    entries: IndexMap<ErasedServiceIdentifier, ServiceEntry>,
}

impl ServiceCollection {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_instance<T>(
        &mut self,
        id: ServiceIdentifier<T>,
        instance: Arc<T>,
    ) -> Option<ServiceEntry>
    where
        T: Send + Sync + 'static,
    {
        self.entries
            .insert(id.erase(), ServiceEntry::Instance(instance))
    }

    pub fn set_descriptor<T>(
        &mut self,
        id: ServiceIdentifier<T>,
        descriptor: SyncDescriptor<T>,
    ) -> Option<ServiceEntry>
    where
        T: Send + Sync + 'static,
    {
        self.entries
            .insert(id.erase(), ServiceEntry::Descriptor(descriptor.erase()))
    }

    pub fn set_erased(
        &mut self,
        id: ErasedServiceIdentifier,
        entry: ServiceEntry,
    ) -> Option<ServiceEntry> {
        self.entries.insert(id, entry)
    }

    pub fn has<T: ?Sized>(&self, id: ServiceIdentifier<T>) -> bool {
        self.entries.contains_key(&id.erase())
    }

    pub fn get<T>(&self, id: ServiceIdentifier<T>) -> Result<Option<Arc<T>>, DiError>
    where
        T: Send + Sync + 'static,
    {
        match self.entries.get(&id.erase()) {
            Some(ServiceEntry::Instance(value)) => Arc::clone(value)
                .downcast::<T>()
                .map(Some)
                .map_err(|_| DiError::TypeMismatch(id.erase())),
            Some(ServiceEntry::Descriptor(_)) | None => Ok(None),
        }
    }

    pub fn get_entry(&self, id: ErasedServiceIdentifier) -> Option<&ServiceEntry> {
        self.entries.get(&id)
    }

    pub fn iter(&self) -> impl Iterator<Item = (ErasedServiceIdentifier, &ServiceEntry)> {
        self.entries.iter().map(|(id, value)| (*id, value))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collection_replaces_entries_and_downcasts_typed_instances() {
        let id = ServiceIdentifier::<String>::new("name");
        let mut collection = ServiceCollection::new();
        assert!(
            collection
                .set_instance(id, Arc::new("first".to_owned()))
                .is_none()
        );
        let previous = collection.set_instance(id, Arc::new("second".to_owned()));
        assert!(matches!(previous, Some(ServiceEntry::Instance(_))));
        assert_eq!(collection.get(id).unwrap().unwrap().as_str(), "second");
        assert!(collection.has(id));
    }

    #[test]
    fn descriptors_remain_lazy_collection_entries() {
        let id = ServiceIdentifier::<usize>::new("count");
        let mut collection = ServiceCollection::new();
        collection.set_descriptor(id, SyncDescriptor::new(|_| Ok(42)));
        assert!(collection.get(id).unwrap().is_none());
        assert!(matches!(
            collection.get_entry(id.erase()),
            Some(ServiceEntry::Descriptor(_))
        ));
    }
}
