//! Config section and effective-overlay registry implementation.
//!
//! Original: `packages/agent-core-v2/src/app/config/configService.ts`,
//! `ConfigRegistry`.

use std::sync::{Arc, LazyLock, Mutex};

use indexmap::IndexMap;
use serde_json::Value;

use crate::_base::{
    di::{
        descriptors::SyncDescriptor,
        errors::DiError,
        scope::{InstantiationType, LifecycleScope, register_scoped_service},
    },
    event::{Emitter, Event},
};

use super::{
    contract::{
        CONFIG_REGISTRY_SERVICE_ID, ConfigEffectiveOverlay, ConfigMerge,
        ConfigOverlayRegisteredEvent, ConfigRegistryContract, ConfigRegistryError,
        ConfigRegistryHandle, ConfigSchema, ConfigSection, ConfigSectionRegisteredEvent,
        RegisterSectionOptions,
    },
    overlay_contributions::get_config_overlay_contributions,
    pure::{deep_equal, deep_merge},
    section_contributions::get_config_section_contributions,
};

static DEFAULT_MERGE: LazyLock<ConfigMerge> = LazyLock::new(|| Arc::new(deep_merge) as ConfigMerge);

#[derive(Default)]
struct RegistryState {
    sections: IndexMap<String, ConfigSection>,
    overlays: Vec<Arc<dyn ConfigEffectiveOverlay>>,
}

pub struct ConfigRegistry {
    state: Mutex<RegistryState>,
    on_did_register_section: Arc<Emitter<ConfigSectionRegisteredEvent>>,
    on_did_register_overlay: Arc<Emitter<ConfigOverlayRegisteredEvent>>,
}

impl ConfigRegistry {
    // Original: ConfigRegistry.constructor().
    pub fn new() -> Result<Self, ConfigRegistryError> {
        let registry = Self {
            state: Mutex::new(RegistryState::default()),
            on_did_register_section: Arc::new(Emitter::new()),
            on_did_register_overlay: Arc::new(Emitter::new()),
        };
        for contribution in get_config_section_contributions() {
            registry.register_section(
                &contribution.domain,
                contribution.schema,
                contribution.options,
            )?;
        }
        for overlay in get_config_overlay_contributions() {
            registry.register_effective_overlay(overlay);
        }
        Ok(registry)
    }
}

impl ConfigRegistryContract for ConfigRegistry {
    fn on_did_register_section(&self) -> Event<ConfigSectionRegisteredEvent> {
        self.on_did_register_section.event()
    }

    fn on_did_register_overlay(&self) -> Event<ConfigOverlayRegisteredEvent> {
        self.on_did_register_overlay.event()
    }

    // Original: ConfigRegistry.registerSection().
    fn register_section(
        &self,
        domain: &str,
        schema: ConfigSchema,
        options: RegisterSectionOptions,
    ) -> Result<(), ConfigRegistryError> {
        let merge = options
            .merge
            .clone()
            .unwrap_or_else(|| Arc::clone(&DEFAULT_MERGE));
        let mut state = self.state.lock().unwrap();
        if let Some(existing) = state.sections.get(domain) {
            if same_section(existing, &schema, &options, &merge) {
                return Ok(());
            }
            return Err(ConfigRegistryError::AlreadyRegistered(domain.into()));
        }
        state.sections.insert(
            domain.into(),
            ConfigSection {
                domain: domain.into(),
                schema,
                default_value: options.default_value,
                merge,
                scope: options.scope.unwrap_or_default(),
                env: options.env,
                strip_env: options.strip_env,
                from_toml: options.from_toml,
                to_toml: options.to_toml,
            },
        );
        drop(state);
        self.on_did_register_section
            .fire(&ConfigSectionRegisteredEvent {
                domain: domain.into(),
            });
        Ok(())
    }

    // Original: ConfigRegistry.getSection().
    fn get_section(&self, domain: &str) -> Option<ConfigSection> {
        self.state.lock().unwrap().sections.get(domain).cloned()
    }

    // Original: ConfigRegistry.listSections().
    fn list_sections(&self) -> Vec<ConfigSection> {
        self.state
            .lock()
            .unwrap()
            .sections
            .values()
            .cloned()
            .collect()
    }

    // Original: ConfigRegistry.registerEffectiveOverlay().
    fn register_effective_overlay(&self, overlay: Arc<dyn ConfigEffectiveOverlay>) {
        self.state
            .lock()
            .unwrap()
            .overlays
            .push(Arc::clone(&overlay));
        self.on_did_register_overlay
            .fire(&ConfigOverlayRegisteredEvent { overlay });
    }

    // Original: ConfigRegistry.listEffectiveOverlays().
    fn list_effective_overlays(&self) -> Vec<Arc<dyn ConfigEffectiveOverlay>> {
        self.state.lock().unwrap().overlays.clone()
    }

    // Original: ConfigRegistry.validate().
    fn validate(&self, domain: &str, value: &Value) -> Result<Value, ConfigRegistryError> {
        let schema = self
            .state
            .lock()
            .unwrap()
            .sections
            .get(domain)
            .map(|section| section.schema.clone());
        match schema {
            None => Ok(value.clone()),
            Some(schema) => schema
                .parse(value)
                .map_err(|source| ConfigRegistryError::Invalid {
                    domain: domain.into(),
                    source,
                }),
        }
    }

    // Original: ConfigRegistry.merge().
    fn merge(&self, domain: &str, base: Option<&Value>, patch: Option<&Value>) -> Option<Value> {
        let merge = self
            .state
            .lock()
            .unwrap()
            .sections
            .get(domain)
            .map(|section| Arc::clone(&section.merge))
            .unwrap_or_else(|| Arc::clone(&DEFAULT_MERGE));
        merge(base, patch)
    }

    // Original: ConfigRegistry.defaultValue().
    fn default_value(&self, domain: &str) -> Option<Value> {
        self.state
            .lock()
            .unwrap()
            .sections
            .get(domain)
            .and_then(|section| section.default_value.clone())
    }
}

fn same_section(
    existing: &ConfigSection,
    schema: &ConfigSchema,
    options: &RegisterSectionOptions,
    merge: &ConfigMerge,
) -> bool {
    existing.schema == *schema
        && Arc::ptr_eq(&existing.merge, merge)
        && existing.scope == options.scope.unwrap_or_default()
        && same_optional_arc(&existing.env, &options.env)
        && same_optional_arc(&existing.strip_env, &options.strip_env)
        && same_optional_arc(&existing.from_toml, &options.from_toml)
        && same_optional_arc(&existing.to_toml, &options.to_toml)
        && match (&existing.default_value, &options.default_value) {
            (Some(left), Some(right)) => deep_equal(left, right),
            (None, None) => true,
            _ => false,
        }
}

fn same_optional_arc<T: ?Sized>(left: &Option<Arc<T>>, right: &Option<Arc<T>>) -> bool {
    match (left, right) {
        (Some(left), Some(right)) => Arc::ptr_eq(left, right),
        (None, None) => true,
        _ => false,
    }
}

// Original: registerScopedService(... ConfigRegistry ...).
pub fn register_config_registry() {
    register_scoped_service(
        LifecycleScope::App,
        CONFIG_REGISTRY_SERVICE_ID,
        SyncDescriptor::new(|_| {
            let registry =
                ConfigRegistry::new().map_err(|error| DiError::Factory(error.to_string()))?;
            let registry: Arc<dyn ConfigRegistryContract> = Arc::new(registry);
            Ok(ConfigRegistryHandle(registry))
        }),
        InstantiationType::Eager,
        "config",
    );
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use serde_json::json;

    use super::*;

    fn boolean_schema() -> ConfigSchema {
        ConfigSchema::new(|value| {
            value.is_boolean().then(|| value.clone()).ok_or_else(|| {
                super::super::contract::ConfigValidationError::new("expected boolean")
            })
        })
    }

    #[test]
    fn registers_validates_lists_and_emits_in_order() {
        let registry = ConfigRegistry::new().unwrap();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let listener_seen = Arc::clone(&seen);
        let _listener = registry.on_did_register_section().subscribe(move |event| {
            listener_seen.lock().unwrap().push(event.domain.clone());
        });
        let schema = boolean_schema();
        registry
            .register_section(
                "example",
                schema.clone(),
                RegisterSectionOptions {
                    default_value: Some(Value::Bool(true)),
                    ..RegisterSectionOptions::default()
                },
            )
            .unwrap();
        assert_eq!(
            registry.validate("example", &Value::Bool(false)).unwrap(),
            false
        );
        assert!(
            registry
                .validate("example", &Value::String("no".into()))
                .is_err()
        );
        assert_eq!(registry.default_value("example"), Some(Value::Bool(true)));
        assert_eq!(*seen.lock().unwrap(), ["example"]);

        registry
            .register_section(
                "example",
                schema,
                RegisterSectionOptions {
                    default_value: Some(Value::Bool(true)),
                    ..RegisterSectionOptions::default()
                },
            )
            .unwrap();
        assert_eq!(*seen.lock().unwrap(), ["example"]);
    }

    #[test]
    fn rejects_conflicting_duplicate_and_uses_section_merge() {
        let registry = ConfigRegistry::new().unwrap();
        registry
            .register_section(
                "example",
                boolean_schema(),
                RegisterSectionOptions::default(),
            )
            .unwrap();
        assert_eq!(
            registry
                .register_section(
                    "example",
                    boolean_schema(),
                    RegisterSectionOptions::default()
                )
                .err(),
            Some(ConfigRegistryError::AlreadyRegistered("example".into()))
        );

        let custom: ConfigMerge = Arc::new(|_, patch| Some(json!({"wrapped": patch.cloned()})));
        registry
            .register_section(
                "custom",
                ConfigSchema::new(|value| Ok(value.clone())),
                RegisterSectionOptions {
                    merge: Some(custom),
                    ..RegisterSectionOptions::default()
                },
            )
            .unwrap();
        assert_eq!(
            registry.merge("custom", None, Some(&json!(1))),
            Some(json!({"wrapped": 1}))
        );
    }
}
