//! Stateless wire model definitions and optional blob transformations.
//!
//! Original: `packages/agent-core-v2/src/wire/model.ts`.

use std::{
    any::Any,
    collections::HashMap,
    fmt,
    sync::{
        Arc, LazyLock, RwLock,
        atomic::{AtomicU64, Ordering},
    },
};

use async_trait::async_trait;
use serde_json::Value;

use super::{
    op::{DefineOpOptions, DefinedOp, DuplicateOpError, define_op},
    record::WireRecord,
};

pub type ErasedState = Box<dyn Any + Send + Sync>;

#[async_trait]
pub trait PartsTransformer: Send + Sync {
    async fn transform(&self, parts: Vec<Value>) -> Result<Vec<Value>, String>;
}

#[async_trait]
pub trait ModelBlobCodec<S>: Send + Sync {
    async fn dehydrate(
        &self,
        record: WireRecord,
        transform: &dyn PartsTransformer,
    ) -> Result<WireRecord, String>;

    async fn rehydrate(&self, state: S, transform: &dyn PartsTransformer) -> Result<S, String>;
}

#[async_trait]
pub trait ErasedModelDef: Send + Sync {
    fn id(&self) -> u64;
    fn name(&self) -> &str;
    fn initial_state(&self) -> ErasedState;
    fn has_blob_codec(&self) -> bool;
    async fn dehydrate_record(
        &self,
        record: WireRecord,
        transform: &dyn PartsTransformer,
    ) -> Result<WireRecord, String>;
    async fn rehydrate_state(
        &self,
        state: ErasedState,
        transform: &dyn PartsTransformer,
    ) -> Result<ErasedState, String>;
}

static NEXT_MODEL_ID: AtomicU64 = AtomicU64::new(1);

struct ModelDefInner<S> {
    id: u64,
    name: String,
    initial: Arc<dyn Fn() -> S + Send + Sync>,
    blobs: Option<Arc<dyn ModelBlobCodec<S>>>,
}

pub struct ModelDef<S> {
    inner: Arc<ModelDefInner<S>>,
}

impl<S> Clone for ModelDef<S> {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

impl<S> fmt::Debug for ModelDef<S> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ModelDef")
            .field("id", &self.inner.id)
            .field("name", &self.inner.name)
            .field("has_blobs", &self.inner.blobs.is_some())
            .finish()
    }
}

impl<S> PartialEq for ModelDef<S> {
    fn eq(&self, other: &Self) -> bool {
        self.inner.id == other.inner.id
    }
}

impl<S> Eq for ModelDef<S> {}

impl<S> ModelDef<S>
where
    S: Send + Sync + 'static,
{
    pub fn id(&self) -> u64 {
        self.inner.id
    }

    pub fn name(&self) -> &str {
        &self.inner.name
    }

    pub fn initial(&self) -> S {
        (self.inner.initial)()
    }

    pub fn blobs(&self) -> Option<&Arc<dyn ModelBlobCodec<S>>> {
        self.inner.blobs.as_ref()
    }

    // Original: ModelDef.defineOp(), bound to this model by defineModel().
    pub fn define_op<P>(
        &self,
        op_type: impl Into<String>,
        options: DefineOpOptions<S, P>,
    ) -> Result<DefinedOp<S, P>, DuplicateOpError>
    where
        P: serde::Serialize + Send + Sync + 'static,
    {
        define_op(self.clone(), op_type, options)
    }

    pub fn erased(&self) -> Arc<dyn ErasedModelDef> {
        Arc::new(self.clone())
    }
}

#[async_trait]
impl<S> ErasedModelDef for ModelDef<S>
where
    S: Send + Sync + 'static,
{
    fn id(&self) -> u64 {
        self.id()
    }

    fn name(&self) -> &str {
        self.name()
    }

    fn initial_state(&self) -> ErasedState {
        Box::new(self.initial())
    }

    fn has_blob_codec(&self) -> bool {
        self.inner.blobs.is_some()
    }

    async fn dehydrate_record(
        &self,
        record: WireRecord,
        transform: &dyn PartsTransformer,
    ) -> Result<WireRecord, String> {
        match &self.inner.blobs {
            Some(codec) => codec.dehydrate(record, transform).await,
            None => Ok(record),
        }
    }

    async fn rehydrate_state(
        &self,
        state: ErasedState,
        transform: &dyn PartsTransformer,
    ) -> Result<ErasedState, String> {
        let Some(codec) = &self.inner.blobs else {
            return Ok(state);
        };
        let state = state
            .downcast::<S>()
            .map_err(|_| format!("Model '{}' received an incompatible state", self.name()))?;
        codec
            .rehydrate(*state, transform)
            .await
            .map(|state| Box::new(state) as ErasedState)
    }
}

pub struct ModelOptions<S> {
    pub blobs: Option<Arc<dyn ModelBlobCodec<S>>>,
    pub reducers: Vec<ModelCrossReducer<S>>,
}

impl<S> Default for ModelOptions<S> {
    fn default() -> Self {
        Self {
            blobs: None,
            reducers: Vec::new(),
        }
    }
}

type ErasedReducer =
    Arc<dyn Fn(ErasedState, &dyn Any) -> Result<ErasedState, String> + Send + Sync>;
type TypedReducer<S> = Arc<dyn Fn(S, &dyn Any) -> Result<S, String> + Send + Sync>;

pub struct ModelCrossReducer<S> {
    op_type: String,
    reducer: TypedReducer<S>,
}

impl<S> ModelCrossReducer<S> {
    pub fn typed<P>(
        op_type: impl Into<String>,
        reducer: impl Fn(S, &P) -> S + Send + Sync + 'static,
    ) -> Self
    where
        P: Send + Sync + 'static,
    {
        let op_type = op_type.into();
        let op_type_for_error = op_type.clone();
        Self {
            op_type,
            reducer: Arc::new(move |state, payload| {
                let payload = payload.downcast_ref::<P>().ok_or_else(|| {
                    format!("Cross reducer for '{op_type_for_error}' received incompatible payload")
                })?;
                Ok(reducer(state, payload))
            }),
        }
    }
}

#[derive(Clone)]
pub struct ErasedCrossReducerEntry {
    pub model: Arc<dyn ErasedModelDef>,
    reducer: ErasedReducer,
}

impl ErasedCrossReducerEntry {
    pub fn apply(&self, state: ErasedState, payload: &dyn Any) -> Result<ErasedState, String> {
        (self.reducer)(state, payload)
    }
}

static MODEL_CROSS_REDUCERS: LazyLock<RwLock<HashMap<String, Vec<ErasedCrossReducerEntry>>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

pub fn model_cross_reducers(op_type: &str) -> Vec<ErasedCrossReducerEntry> {
    MODEL_CROSS_REDUCERS
        .read()
        .unwrap()
        .get(op_type)
        .cloned()
        .unwrap_or_default()
}

// Original: defineModel().
pub fn define_model<S>(
    name: impl Into<String>,
    initial: impl Fn() -> S + Send + Sync + 'static,
    options: ModelOptions<S>,
) -> ModelDef<S>
where
    S: Send + Sync + 'static,
{
    let model = ModelDef {
        inner: Arc::new(ModelDefInner {
            id: NEXT_MODEL_ID.fetch_add(1, Ordering::Relaxed),
            name: name.into(),
            initial: Arc::new(initial),
            blobs: options.blobs,
        }),
    };
    if !options.reducers.is_empty() {
        let erased_model = model.erased();
        let mut registry = MODEL_CROSS_REDUCERS.write().unwrap();
        for cross in options.reducers {
            let reducer = cross.reducer;
            let model_name = model.name().to_owned();
            let erased: ErasedReducer = Arc::new(move |state, payload| {
                let state = state.downcast::<S>().map_err(|_| {
                    format!("Cross reducer received incompatible state for '{model_name}'")
                })?;
                reducer(*state, payload).map(|state| Box::new(state) as ErasedState)
            });
            registry
                .entry(cross.op_type)
                .or_default()
                .push(ErasedCrossReducerEntry {
                    model: Arc::clone(&erased_model),
                    reducer: erased,
                });
        }
    }
    model
}

#[cfg(test)]
mod tests {
    use super::*;

    struct IdentityTransformer;

    #[async_trait]
    impl PartsTransformer for IdentityTransformer {
        async fn transform(&self, parts: Vec<Value>) -> Result<Vec<Value>, String> {
            Ok(parts)
        }
    }

    struct CounterCodec;

    #[async_trait]
    impl ModelBlobCodec<u64> for CounterCodec {
        async fn dehydrate(
            &self,
            mut record: WireRecord,
            _transform: &dyn PartsTransformer,
        ) -> Result<WireRecord, String> {
            record.insert("dehydrated".into(), Value::Bool(true));
            Ok(record)
        }

        async fn rehydrate(
            &self,
            state: u64,
            _transform: &dyn PartsTransformer,
        ) -> Result<u64, String> {
            Ok(state + 1)
        }
    }

    #[test]
    fn model_identity_initial_state_and_erasure_are_stable() {
        let model = define_model("counter", || 1_u64, ModelOptions::default());
        let clone = model.clone();
        assert_eq!(model, clone);
        assert_eq!(model.name(), "counter");
        assert_eq!(model.initial(), 1);
        let erased = model.erased();
        assert_eq!(erased.id(), model.id());
        assert_eq!(*erased.initial_state().downcast::<u64>().unwrap(), 1);
    }

    #[test]
    fn registers_and_applies_typed_cross_model_reducers_in_order() {
        #[derive(Debug)]
        struct Payload(u64);

        let op_type = format!(
            "test.cross.{}",
            NEXT_MODEL_ID.fetch_add(1, Ordering::Relaxed)
        );
        let model = define_model(
            "derived-counter",
            || 1_u64,
            ModelOptions {
                blobs: None,
                reducers: vec![ModelCrossReducer::typed(
                    &op_type,
                    |state, payload: &Payload| state + payload.0,
                )],
            },
        );
        let reducers = model_cross_reducers(&op_type);
        assert_eq!(reducers.len(), 1);
        assert_eq!(reducers[0].model.id(), model.id());
        let state = reducers[0]
            .apply(Box::new(model.initial()), &Payload(4))
            .unwrap();
        assert_eq!(*state.downcast::<u64>().unwrap(), 5);
    }

    #[tokio::test]
    async fn erased_blob_codec_preserves_both_transformation_directions() {
        let model = define_model(
            "blobs",
            || 1_u64,
            ModelOptions {
                blobs: Some(Arc::new(CounterCodec)),
                ..ModelOptions::default()
            },
        );
        let erased = model.erased();
        assert!(erased.has_blob_codec());
        let record = erased
            .dehydrate_record(WireRecord::new(), &IdentityTransformer)
            .await
            .unwrap();
        assert_eq!(record.get("dehydrated"), Some(&Value::Bool(true)));
        let state = erased
            .rehydrate_state(Box::new(1_u64), &IdentityTransformer)
            .await
            .unwrap();
        assert_eq!(*state.downcast::<u64>().unwrap(), 2);
    }
}
