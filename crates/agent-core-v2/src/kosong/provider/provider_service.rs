//! Config-backed provider service.
//!
//! Original: `packages/agent-core-v2/src/kosong/provider/providerService.ts`.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{Map, Value};

use crate::{
    _base::{
        di::{
            descriptors::SyncDescriptor,
            instantiation::ServicesAccessorExt,
            lifecycle::{Disposable, DisposableStore, DisposeResult},
            scope::{InstantiationType, LifecycleScope, register_scoped_service},
        },
        event::{Emitter, Event},
    },
    app::config::{CONFIG_SERVICE_ID, ConfigServiceHandle, ConfigTarget, diff_records},
};

use super::{
    config::{
        DEFAULT_PROVIDER_SECTION, PROVIDERS_SECTION, ProviderConfig, ProvidersChangedEvent,
        ProvidersSection,
    },
    config_section::register_provider_config_section,
    contract::{
        PROVIDER_SERVICE_ID, ProviderServiceContract, ProviderServiceHandle, ProviderServiceResult,
    },
};

pub struct ProviderService {
    config: ConfigServiceHandle,
    on_did_change_providers: Arc<Emitter<ProvidersChangedEvent>>,
    disposables: DisposableStore,
}

impl ProviderService {
    // Original: ProviderService.constructor().
    pub fn new(config: ConfigServiceHandle) -> Self {
        let emitter = Arc::new(Emitter::new());
        let disposables = DisposableStore::new();
        disposables.add(Arc::clone(&emitter) as Arc<dyn Disposable>);
        let event_emitter = Arc::clone(&emitter);
        disposables.add(
            config
                .on_did_change_configuration()
                .subscribe(move |event| {
                    if event.domain != PROVIDERS_SECTION {
                        return;
                    }
                    let diff = diff_records(
                        event.previous_value.as_ref().and_then(Value::as_object),
                        event.value.as_ref().and_then(Value::as_object),
                    );
                    event_emitter.fire(&ProvidersChangedEvent {
                        added: diff.added,
                        removed: diff.removed,
                        changed: diff.changed,
                    });
                }),
        );
        Self {
            config,
            on_did_change_providers: emitter,
            disposables,
        }
    }
}

#[async_trait]
impl ProviderServiceContract for ProviderService {
    async fn ready(&self) -> ProviderServiceResult<()> {
        self.config.ready().await?;
        Ok(())
    }

    fn on_did_change_providers(&self) -> Event<ProvidersChangedEvent> {
        self.on_did_change_providers.event()
    }

    // Original: ProviderService.get().
    fn get(&self, name: &str) -> Option<ProviderConfig> {
        self.list().shift_remove(name)
    }

    // Original: ProviderService.list().
    fn list(&self) -> ProvidersSection {
        self.config
            .get(PROVIDERS_SECTION)
            .and_then(|value| serde_json::from_value(value).ok())
            .unwrap_or_default()
    }

    // Original: ProviderService.set().
    async fn set(&self, name: &str, config: ProviderConfig) -> ProviderServiceResult<()> {
        let patch = Value::Object(Map::from_iter([(
            name.to_owned(),
            serde_json::to_value(config)?,
        )]));
        self.config
            .set(PROVIDERS_SECTION, Some(patch), ConfigTarget::User)
            .await?;
        Ok(())
    }

    // Original: ProviderService.delete().
    async fn delete(&self, name: &str) -> ProviderServiceResult<()> {
        let mut current = self.list();
        if current.shift_remove(name).is_none() {
            return Ok(());
        }
        self.config
            .replace(
                PROVIDERS_SECTION,
                Some(serde_json::to_value(current)?),
                ConfigTarget::User,
            )
            .await?;
        if self
            .config
            .get(DEFAULT_PROVIDER_SECTION)
            .as_ref()
            .and_then(Value::as_str)
            == Some(name)
        {
            self.config
                .set(DEFAULT_PROVIDER_SECTION, None, ConfigTarget::User)
                .await?;
        }
        Ok(())
    }
}

impl Disposable for ProviderService {
    fn dispose(&self) -> DisposeResult {
        self.disposables.dispose()
    }
}

pub fn register_provider_service() {
    register_provider_config_section();
    register_scoped_service(
        LifecycleScope::App,
        PROVIDER_SERVICE_ID,
        SyncDescriptor::new(|accessor| {
            let config = accessor.get(CONFIG_SERVICE_ID)?;
            let service: Arc<dyn ProviderServiceContract> =
                Arc::new(ProviderService::new((*config).clone()));
            Ok(ProviderServiceHandle(service))
        })
        .disposable(),
        InstantiationType::Eager,
        "provider",
    );
}

#[cfg(test)]
mod tests {
    use parking_lot::Mutex;
    use std::sync::Arc;

    use serde_json::json;

    use crate::app::config::{
        ConfigChangeSource, ConfigChangedEvent, ConfigDiagnostic, ConfigInspectValue,
        ConfigServiceContract, ConfigServiceError, ResolvedConfig,
    };

    use super::*;

    struct StubConfigService {
        values: Mutex<ResolvedConfig>,
        emitter: Emitter<ConfigChangedEvent>,
    }

    impl StubConfigService {
        fn new(values: ResolvedConfig) -> Self {
            Self {
                values: Mutex::new(values),
                emitter: Emitter::new(),
            }
        }

        fn change(&self, domain: &str, value: Option<Value>) {
            let previous_value = self.values.lock().get(domain).cloned();
            let mut values = self.values.lock();
            match &value {
                Some(value) => {
                    values.insert(domain.to_owned(), value.clone());
                }
                None => {
                    values.shift_remove(domain);
                }
            }
            drop(values);
            self.emitter.fire(&ConfigChangedEvent {
                domain: domain.to_owned(),
                source: ConfigChangeSource::Set,
                value,
                previous_value,
            });
        }
    }

    #[async_trait]
    impl ConfigServiceContract for StubConfigService {
        async fn ready(&self) -> Result<(), ConfigServiceError> {
            Ok(())
        }
        fn on_did_change_configuration(&self) -> Event<ConfigChangedEvent> {
            self.emitter.event()
        }
        fn on_did_section_change(&self) -> Event<ConfigChangedEvent> {
            self.emitter.event()
        }
        fn get(&self, domain: &str) -> Option<Value> {
            self.values.lock().get(domain).cloned()
        }
        fn inspect(&self, _domain: &str) -> ConfigInspectValue {
            ConfigInspectValue::default()
        }
        fn get_all(&self) -> ResolvedConfig {
            self.values.lock().clone()
        }
        async fn set(
            &self,
            domain: &str,
            patch: Option<Value>,
            _target: ConfigTarget,
        ) -> Result<(), ConfigServiceError> {
            if domain == PROVIDERS_SECTION {
                let mut current = self
                    .get(domain)
                    .and_then(|value| value.as_object().cloned())
                    .unwrap_or_default();
                if let Some(patch) = patch.and_then(|value| value.as_object().cloned()) {
                    current.extend(patch);
                }
                self.change(domain, Some(Value::Object(current)));
            } else {
                self.change(domain, patch);
            }
            Ok(())
        }
        async fn replace(
            &self,
            domain: &str,
            value: Option<Value>,
            _target: ConfigTarget,
        ) -> Result<(), ConfigServiceError> {
            self.change(domain, value);
            Ok(())
        }
        async fn reload(&self) -> Result<(), ConfigServiceError> {
            Ok(())
        }
        fn diagnostics(&self) -> Vec<ConfigDiagnostic> {
            Vec::new()
        }
    }

    impl Disposable for StubConfigService {
        fn dispose(&self) -> DisposeResult {
            self.emitter.dispose()
        }
    }

    fn provider(base_url: &str) -> ProviderConfig {
        ProviderConfig {
            base_url: Some(base_url.to_owned()),
            ..ProviderConfig::default()
        }
    }

    #[tokio::test]
    async fn reads_sets_deletes_and_emits_record_diffs() {
        let stub = Arc::new(StubConfigService::new(Map::from_iter([
            (
                PROVIDERS_SECTION.to_owned(),
                json!({"old": {"baseUrl": "old"}}),
            ),
            (DEFAULT_PROVIDER_SECTION.to_owned(), json!("old")),
        ])));
        let config: Arc<dyn ConfigServiceContract> = stub.clone();
        let service = ProviderService::new(ConfigServiceHandle(config));
        let seen = Arc::new(Mutex::new(Vec::new()));
        let seen_listener = Arc::clone(&seen);
        let _subscription = service.on_did_change_providers().subscribe(move |event| {
            seen_listener.lock().push(event.clone());
        });

        service.ready().await.unwrap();
        assert_eq!(service.get("old").unwrap().base_url.as_deref(), Some("old"));
        service.set("new", provider("new")).await.unwrap();
        service.delete("old").await.unwrap();
        assert_eq!(service.list().keys().cloned().collect::<Vec<_>>(), ["new"]);
        assert!(stub.get(DEFAULT_PROVIDER_SECTION).is_none());
        let seen = seen.lock();
        assert_eq!(seen[0].added, ["new"]);
        assert_eq!(seen[1].removed, ["old"]);
    }
}
