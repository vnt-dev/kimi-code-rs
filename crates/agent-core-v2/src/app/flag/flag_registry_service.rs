//! In-memory flag-definition registry.
//!
//! Original: `packages/agent-core-v2/src/app/flag/flagRegistryService.ts`.

use std::sync::{Arc, Mutex};

use indexmap::IndexMap;

use crate::_base::di::lifecycle::{
    Disposable, DisposableHandle, DisposableStore, DisposeResult, to_disposable,
};
use crate::_base::di::{
    descriptors::SyncDescriptor,
    errors::DiError,
    scope::{InstantiationType, LifecycleScope, register_scoped_service},
};

use super::flag_registry::{
    FLAG_REGISTRY_SERVICE_ID, FlagDefinitionInput, FlagRegistry, FlagRegistryError,
    FlagRegistryHandle, get_contributed_flags,
};

pub struct FlagRegistryService {
    by_id: Arc<Mutex<IndexMap<String, FlagDefinitionInput>>>,
    registrations: DisposableStore,
}

impl FlagRegistryService {
    fn empty() -> Self {
        Self {
            by_id: Arc::new(Mutex::new(IndexMap::new())),
            registrations: DisposableStore::new(),
        }
    }

    // Original: FlagRegistryService.constructor().
    pub fn new() -> Result<Self, FlagRegistryError> {
        let service = Self::empty();
        for definition in get_contributed_flags() {
            service.add(definition)?;
        }
        Ok(service)
    }

    #[cfg(test)]
    pub(crate) fn empty_for_tests() -> Self {
        Self::empty()
    }

    // Original: FlagRegistryService.add().
    fn add(&self, definition: FlagDefinitionInput) -> Result<(), FlagRegistryError> {
        let mut by_id = self.by_id.lock().unwrap();
        if by_id.contains_key(&definition.id) {
            return Err(FlagRegistryError::AlreadyRegistered(definition.id));
        }
        by_id.insert(definition.id.clone(), definition);
        Ok(())
    }
}

impl FlagRegistry for FlagRegistryService {
    // Original: FlagRegistryService.register().
    fn register(
        &self,
        definition: FlagDefinitionInput,
    ) -> Result<DisposableHandle, FlagRegistryError> {
        self.add(definition.clone())?;
        let by_id = Arc::clone(&self.by_id);
        let id = definition.id;
        Ok(self.registrations.add(to_disposable(move || {
            by_id.lock().unwrap().shift_remove(&id);
        })))
    }

    // Original: FlagRegistryService.get().
    fn get(&self, id: &str) -> Option<FlagDefinitionInput> {
        self.by_id.lock().unwrap().get(id).cloned()
    }

    // Original: FlagRegistryService.list().
    fn list(&self) -> Vec<FlagDefinitionInput> {
        self.by_id.lock().unwrap().values().cloned().collect()
    }
}

impl Disposable for FlagRegistryService {
    fn dispose(&self) -> DisposeResult {
        self.registrations.dispose()
    }
}

// Original: registerScopedService(... FlagRegistryService ...).
pub fn register_flag_registry_service() {
    register_scoped_service(
        LifecycleScope::App,
        FLAG_REGISTRY_SERVICE_ID,
        SyncDescriptor::new(|_| {
            let registry =
                FlagRegistryService::new().map_err(|error| DiError::Factory(error.to_string()))?;
            let registry: Arc<dyn FlagRegistry> = Arc::new(registry);
            Ok(FlagRegistryHandle(registry))
        }),
        InstantiationType::Eager,
        "flag",
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::flag::flag_registry::FlagSurface;

    fn example_flag() -> FlagDefinitionInput {
        FlagDefinitionInput {
            id: "example_flag".into(),
            title: "Example flag".into(),
            description: "Example experimental flag used to exercise the flag registry.".into(),
            env: "KIMI_CODE_EXPERIMENTAL_EXAMPLE_FLAG".into(),
            default: true,
            surface: FlagSurface::Core,
        }
    }

    #[test]
    fn registers_resolves_and_preserves_insertion_order() {
        let registry = FlagRegistryService::empty_for_tests();
        let definition = example_flag();
        registry.register(definition.clone()).unwrap();

        assert_eq!(registry.get("example_flag"), Some(definition));
        assert_eq!(
            registry
                .list()
                .into_iter()
                .filter(|item| item.id == "example_flag")
                .count(),
            1
        );
        assert_eq!(registry.get("does_not_exist"), None);
    }

    #[test]
    fn rejects_duplicate_ids_with_the_original_error() {
        let registry = FlagRegistryService::empty_for_tests();
        registry.register(example_flag()).unwrap();
        assert_eq!(
            registry.register(example_flag()).err(),
            Some(FlagRegistryError::AlreadyRegistered("example_flag".into()))
        );
    }

    #[test]
    fn registration_is_removed_when_its_handle_is_disposed() {
        let registry = FlagRegistryService::empty_for_tests();
        let handle = registry.register(example_flag()).unwrap();
        handle.dispose().unwrap();
        handle.dispose().unwrap();
        assert_eq!(registry.get("example_flag"), None);
    }

    #[test]
    fn disposing_registry_removes_runtime_registrations() {
        let registry = FlagRegistryService::empty_for_tests();
        let _handle = registry.register(example_flag()).unwrap();
        registry.dispose().unwrap();
        assert_eq!(registry.get("example_flag"), None);
    }
}
