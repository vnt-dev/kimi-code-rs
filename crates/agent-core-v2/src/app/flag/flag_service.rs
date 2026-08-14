//! Experimental flag resolution service.
//!
//! Original: `packages/agent-core-v2/src/app/flag/flagService.ts`.

use parking_lot::Mutex;
use std::sync::Arc;

use indexmap::IndexMap;

use crate::{
    _base::{
        di::{
            descriptors::SyncDescriptor,
            instantiation::ServicesAccessorExt,
            lifecycle::{Disposable, DisposableStore, DisposeResult},
            scope::{InstantiationType, LifecycleScope, register_scoped_service},
        },
        utils::env::parse_boolean_env,
    },
    app::{
        bootstrap::{BOOTSTRAP_SERVICE_ID, BootstrapServiceHandle},
        config::{CONFIG_SERVICE_ID, ConfigServiceHandle},
    },
};

use super::{
    contract::{
        EXPERIMENTAL_SECTION, ExperimentalFeatureState, ExperimentalFlagConfig,
        ExperimentalFlagMap, ExperimentalFlagSource, FLAG_SERVICE_ID, FlagServiceContract,
        FlagServiceHandle, register_experimental_config_section,
    },
    flag_registry::{
        FLAG_REGISTRY_SERVICE_ID, FlagDefinitionInput, FlagId, FlagRegistry, FlagRegistryHandle,
    },
};

pub const MASTER_ENV: &str = "KIMI_CODE_EXPERIMENTAL_FLAG";

pub struct FlagService {
    bootstrap: BootstrapServiceHandle,
    config: ConfigServiceHandle,
    registry: Arc<dyn FlagRegistry>,
    config_overrides: Mutex<ExperimentalFlagConfig>,
    disposables: DisposableStore,
}

impl FlagService {
    // Original: FlagService.constructor(). Returning Arc is the Rust adaptation
    // that lets the config listener retain only a Weak service reference.
    pub fn new(
        bootstrap: BootstrapServiceHandle,
        config: ConfigServiceHandle,
        registry: FlagRegistryHandle,
    ) -> Arc<Self> {
        let service = Arc::new(Self {
            config_overrides: Mutex::new(read_config(&config)),
            bootstrap,
            config: config.clone(),
            registry: Arc::clone(&registry.0),
            disposables: DisposableStore::new(),
        });
        let weak = Arc::downgrade(&service);
        let subscription = config
            .on_did_change_configuration()
            .subscribe(move |event| {
                if event.domain == EXPERIMENTAL_SECTION
                    && let Some(service) = weak.upgrade()
                {
                    *service.config_overrides.lock() = read_config(&service.config);
                }
            });
        service.disposables.add(subscription);
        service
    }

    // Original: FlagService.state().
    fn state(
        definition: FlagDefinitionInput,
        enabled: bool,
        source: ExperimentalFlagSource,
        config_value: Option<bool>,
    ) -> ExperimentalFeatureState {
        ExperimentalFeatureState {
            id: definition.id,
            title: definition.title,
            description: definition.description,
            surface: definition.surface,
            env: definition.env,
            default_enabled: definition.default,
            enabled,
            source,
            config_value,
        }
    }
}

impl FlagServiceContract for FlagService {
    fn registry(&self) -> Arc<dyn FlagRegistry> {
        Arc::clone(&self.registry)
    }

    // Original: FlagService.enabled().
    fn enabled(&self, id: &str) -> bool {
        self.explain(id).is_some_and(|state| state.enabled)
    }

    // Original: FlagService.snapshot().
    fn snapshot(&self) -> ExperimentalFlagMap {
        self.registry
            .list()
            .into_iter()
            .map(|definition| {
                let id = definition.id;
                let enabled = self.enabled(&id);
                (id, enabled)
            })
            .collect()
    }

    // Original: FlagService.enabledIds().
    fn enabled_ids(&self) -> Vec<FlagId> {
        self.registry
            .list()
            .into_iter()
            .filter_map(|definition| self.enabled(&definition.id).then_some(definition.id))
            .collect()
    }

    // Original: FlagService.explain().
    fn explain(&self, id: &str) -> Option<ExperimentalFeatureState> {
        let definition = self.registry.get(id)?;
        let config_value = self
            .config_overrides
            .lock()
            .get(&definition.id)
            .copied();
        if parse_boolean_env(self.bootstrap.get_env(MASTER_ENV)) == Some(true) {
            return Some(Self::state(
                definition,
                true,
                ExperimentalFlagSource::MasterEnv,
                config_value,
            ));
        }
        if let Some(override_value) = parse_boolean_env(self.bootstrap.get_env(&definition.env)) {
            return Some(Self::state(
                definition,
                override_value,
                ExperimentalFlagSource::Env,
                config_value,
            ));
        }
        if let Some(config_value) = config_value {
            return Some(Self::state(
                definition,
                config_value,
                ExperimentalFlagSource::Config,
                Some(config_value),
            ));
        }
        let default = definition.default;
        Some(Self::state(
            definition,
            default,
            ExperimentalFlagSource::Default,
            None,
        ))
    }

    // Original: FlagService.explainAll().
    fn explain_all(&self) -> Vec<ExperimentalFeatureState> {
        self.registry
            .list()
            .into_iter()
            .filter_map(|definition| self.explain(&definition.id))
            .collect()
    }

    // Original: FlagService.setConfigOverrides().
    fn set_config_overrides(&self, overrides: Option<ExperimentalFlagConfig>) {
        *self.config_overrides.lock() = overrides.unwrap_or_default();
    }
}

impl Disposable for FlagService {
    fn dispose(&self) -> DisposeResult {
        self.disposables.dispose()
    }
}

fn read_config(config: &ConfigServiceHandle) -> ExperimentalFlagConfig {
    config
        .get(EXPERIMENTAL_SECTION)
        .and_then(|value| value.as_object().cloned())
        .map(|object| {
            object
                .into_iter()
                .filter_map(|(id, value)| value.as_bool().map(|enabled| (id, enabled)))
                .collect::<IndexMap<_, _>>()
        })
        .unwrap_or_default()
}

// Original: registerScopedService(... FlagService ...).
pub fn register_flag_service() {
    register_experimental_config_section();
    register_scoped_service(
        LifecycleScope::App,
        FLAG_SERVICE_ID,
        SyncDescriptor::new(|accessor| {
            let bootstrap = accessor.get(BOOTSTRAP_SERVICE_ID)?;
            let config = accessor.get(CONFIG_SERVICE_ID)?;
            let registry = accessor.get(FLAG_REGISTRY_SERVICE_ID)?;
            let service: Arc<dyn FlagServiceContract> =
                FlagService::new((*bootstrap).clone(), (*config).clone(), (*registry).clone());
            Ok(FlagServiceHandle(service))
        })
        .disposable(),
        InstantiationType::Eager,
        "flag",
    );
}

#[cfg(test)]
mod tests {
    use std::{collections::HashMap, path::PathBuf};

    use async_trait::async_trait;
    use serde_json::{Map, Value, json};

    use crate::{
        _base::{
            di::lifecycle::{Disposable, DisposeResult},
            event::{Emitter, Event},
        },
        app::{
            bootstrap::{BootstrapOptions, BootstrapService, BootstrapServiceContract},
            config::{
                ConfigChangeSource, ConfigChangedEvent, ConfigDiagnostic, ConfigInspectValue,
                ConfigSectionChangedEvent, ConfigServiceContract, ConfigServiceError, ConfigTarget,
                ResolvedConfig,
            },
        },
    };

    use super::*;
    use crate::app::flag::{FlagRegistryService, FlagSurface};

    struct StubConfig {
        value: Mutex<Option<Value>>,
        changed: Arc<Emitter<ConfigChangedEvent>>,
    }

    impl StubConfig {
        fn new(value: Option<Value>) -> Arc<Self> {
            Arc::new(Self {
                value: Mutex::new(value),
                changed: Arc::new(Emitter::new()),
            })
        }

        fn set_experimental(&self, value: Option<Value>) {
            let previous_value = self
                .value
                .lock()
                .replace(value.clone().unwrap_or(Value::Null));
            *self.value.lock() = value.clone();
            self.changed.fire(&ConfigChangedEvent {
                domain: EXPERIMENTAL_SECTION.into(),
                source: ConfigChangeSource::Set,
                value,
                previous_value,
            });
        }
    }

    impl Disposable for StubConfig {
        fn dispose(&self) -> DisposeResult {
            self.changed.dispose()
        }
    }

    #[async_trait]
    impl ConfigServiceContract for StubConfig {
        async fn ready(&self) -> Result<(), ConfigServiceError> {
            Ok(())
        }

        fn on_did_change_configuration(&self) -> Event<ConfigChangedEvent> {
            self.changed.event()
        }

        fn on_did_section_change(&self) -> Event<ConfigSectionChangedEvent> {
            Event::none()
        }

        fn get(&self, domain: &str) -> Option<Value> {
            (domain == EXPERIMENTAL_SECTION)
                .then(|| self.value.lock().clone())
                .flatten()
        }

        fn inspect(&self, domain: &str) -> ConfigInspectValue {
            ConfigInspectValue {
                value: self.get(domain),
                ..ConfigInspectValue::default()
            }
        }

        fn get_all(&self) -> ResolvedConfig {
            self.get(EXPERIMENTAL_SECTION)
                .map(|value| Map::from_iter([(EXPERIMENTAL_SECTION.into(), value)]))
                .unwrap_or_default()
        }

        async fn set(
            &self,
            _domain: &str,
            _patch: Option<Value>,
            _target: ConfigTarget,
        ) -> Result<(), ConfigServiceError> {
            Ok(())
        }

        async fn replace(
            &self,
            _domain: &str,
            _value: Option<Value>,
            _target: ConfigTarget,
        ) -> Result<(), ConfigServiceError> {
            Ok(())
        }

        async fn reload(&self) -> Result<(), ConfigServiceError> {
            Ok(())
        }

        fn diagnostics(&self) -> Vec<ConfigDiagnostic> {
            Vec::new()
        }
    }

    fn make_flags(
        env: HashMap<String, String>,
        config_value: Option<Value>,
    ) -> (Arc<FlagService>, Arc<StubConfig>) {
        let bootstrap: Arc<dyn BootstrapServiceContract> =
            Arc::new(BootstrapService::new(BootstrapOptions {
                home_dir: PathBuf::from("/tmp/kimi-home"),
                config_path: PathBuf::from("/tmp/kimi-home/config.toml"),
                os_home_dir: PathBuf::from("/home/test"),
                platform: "linux".into(),
                arch: "x64".into(),
                cwd: PathBuf::from("/tmp"),
                env,
                client_version: "test".into(),
            }));
        let config = StubConfig::new(config_value);
        let config_contract: Arc<dyn ConfigServiceContract> = config.clone();
        let registry = Arc::new(FlagRegistryService::empty_for_tests());
        registry
            .register(FlagDefinitionInput {
                id: "example_flag".into(),
                title: "Example flag".into(),
                description: "Example experimental flag".into(),
                env: "KIMI_CODE_EXPERIMENTAL_EXAMPLE_FLAG".into(),
                default: true,
                surface: FlagSurface::Core,
            })
            .unwrap();
        let registry_contract: Arc<dyn FlagRegistry> = registry;
        (
            FlagService::new(
                BootstrapServiceHandle(bootstrap),
                ConfigServiceHandle(config_contract),
                FlagRegistryHandle(registry_contract),
            ),
            config,
        )
    }

    #[test]
    fn resolves_master_env_feature_env_config_and_default_in_order() {
        let (flags, _) = make_flags(HashMap::new(), None);
        assert_eq!(
            flags.explain("example_flag").unwrap().source,
            ExperimentalFlagSource::Default
        );

        let (flags, _) = make_flags(HashMap::new(), Some(json!({"example_flag": false})));
        let state = flags.explain("example_flag").unwrap();
        assert!(!state.enabled);
        assert_eq!(state.source, ExperimentalFlagSource::Config);

        let (flags, _) = make_flags(
            HashMap::from([("KIMI_CODE_EXPERIMENTAL_EXAMPLE_FLAG".into(), "YES".into())]),
            Some(json!({"example_flag": false})),
        );
        assert_eq!(
            flags.explain("example_flag").unwrap().source,
            ExperimentalFlagSource::Env
        );

        let (flags, _) = make_flags(
            HashMap::from([
                (MASTER_ENV.into(), "1".into()),
                ("KIMI_CODE_EXPERIMENTAL_EXAMPLE_FLAG".into(), "off".into()),
            ]),
            Some(json!({"example_flag": false})),
        );
        let state = flags.explain("example_flag").unwrap();
        assert!(state.enabled);
        assert_eq!(state.source, ExperimentalFlagSource::MasterEnv);
    }

    #[test]
    fn refreshes_config_and_exposes_ordered_views() {
        let (flags, config) = make_flags(HashMap::new(), None);
        assert_eq!(
            flags.snapshot(),
            IndexMap::from([("example_flag".into(), true)])
        );
        assert_eq!(flags.enabled_ids(), ["example_flag"]);
        assert_eq!(flags.explain_all().len(), 1);
        assert!(!flags.enabled("missing"));

        config.set_experimental(Some(json!({"example_flag": false})));
        assert!(!flags.enabled("example_flag"));
        flags.set_config_overrides(None);
        assert!(flags.enabled("example_flag"));
    }
}
