//! Config-backed model service.
//!
//! Original: `packages/agent-core-v2/src/kosong/model/modelService.ts`.

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
    config_section::register_models_config_section,
    contract::{
        MODEL_SERVICE_ID, MODELS_SECTION, ModelRecord, ModelServiceContract, ModelServiceHandle,
        ModelServiceResult, ModelsChangedEvent, ModelsSection,
    },
    env_overlay::register_kimi_model_env_overlay,
};

pub struct ModelService {
    config: ConfigServiceHandle,
    on_did_change_models: Arc<Emitter<ModelsChangedEvent>>,
    disposables: DisposableStore,
}

impl ModelService {
    // Original: ModelService.constructor().
    pub fn new(config: ConfigServiceHandle) -> Self {
        let emitter = Arc::new(Emitter::new());
        let disposables = DisposableStore::new();
        disposables.add(Arc::clone(&emitter) as Arc<dyn Disposable>);
        let event_emitter = Arc::clone(&emitter);
        disposables.add(
            config
                .on_did_change_configuration()
                .subscribe(move |event| {
                    if event.domain != MODELS_SECTION {
                        return;
                    }
                    let diff = diff_records(
                        event.previous_value.as_ref().and_then(Value::as_object),
                        event.value.as_ref().and_then(Value::as_object),
                    );
                    event_emitter.fire(&ModelsChangedEvent {
                        added: diff.added,
                        removed: diff.removed,
                        changed: diff.changed,
                    });
                }),
        );
        Self {
            config,
            on_did_change_models: emitter,
            disposables,
        }
    }
}

#[async_trait]
impl ModelServiceContract for ModelService {
    fn on_did_change_models(&self) -> Event<ModelsChangedEvent> {
        self.on_did_change_models.event()
    }

    // Original: ModelService.get().
    fn get(&self, id: &str) -> Option<ModelRecord> {
        self.list().shift_remove(id)
    }

    // Original: ModelService.list().
    fn list(&self) -> ModelsSection {
        self.config
            .get(MODELS_SECTION)
            .and_then(|value| serde_json::from_value(value).ok())
            .unwrap_or_default()
    }

    // Original: ModelService.set(). ConfigService.set merges the one-record
    // patch into the current models section.
    async fn set(&self, id: &str, model: ModelRecord) -> ModelServiceResult<()> {
        let patch = Value::Object(Map::from_iter([(
            id.to_owned(),
            serde_json::to_value(model)?,
        )]));
        self.config
            .set(MODELS_SECTION, Some(patch), ConfigTarget::User)
            .await?;
        Ok(())
    }

    // Original: ModelService.delete(). A missing id intentionally causes no
    // write and therefore emits no config or model change event.
    async fn delete(&self, id: &str) -> ModelServiceResult<()> {
        let mut current = self.list();
        if current.shift_remove(id).is_none() {
            return Ok(());
        }
        self.config
            .replace(
                MODELS_SECTION,
                Some(serde_json::to_value(current)?),
                ConfigTarget::User,
            )
            .await?;
        Ok(())
    }
}

impl Disposable for ModelService {
    fn dispose(&self) -> DisposeResult {
        self.disposables.dispose()
    }
}

// Original: modelService.ts module setup. Rust composes this explicitly so
// loading a module cannot mutate global service state.
pub fn register_model_service() {
    register_models_config_section();
    register_kimi_model_env_overlay();
    register_scoped_service(
        LifecycleScope::App,
        MODEL_SERVICE_ID,
        SyncDescriptor::new(|accessor| {
            let config = accessor.get(CONFIG_SERVICE_ID)?;
            let service: Arc<dyn ModelServiceContract> =
                Arc::new(ModelService::new((*config).clone()));
            Ok(ModelServiceHandle(service))
        })
        .disposable(),
        InstantiationType::Eager,
        "model",
    );
}
