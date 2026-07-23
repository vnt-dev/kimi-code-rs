//! Stateless wire model definitions and optional blob transformations.
//!
//! Original: `packages/agent-core-v2/src/wire/model.ts`.

use std::{
    any::Any,
    fmt,
    sync::{
        Arc,
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

#[derive(Default)]
pub struct ModelOptions<S> {
    pub blobs: Option<Arc<dyn ModelBlobCodec<S>>>,
}

// Original: defineModel().
// MIGRATION-TODO:
// Original feature: opts.reducers registers cross-model reducers by foreign Op type.
// Temporary behavior: ModelOptions currently accepts the blob codec only.
// Completion condition: add the erased cross-reducer registry with its first domain consumer.
pub fn define_model<S>(
    name: impl Into<String>,
    initial: impl Fn() -> S + Send + Sync + 'static,
    options: ModelOptions<S>,
) -> ModelDef<S>
where
    S: Send + Sync + 'static,
{
    ModelDef {
        inner: Arc::new(ModelDefInner {
            id: NEXT_MODEL_ID.fetch_add(1, Ordering::Relaxed),
            name: name.into(),
            initial: Arc::new(initial),
            blobs: options.blobs,
        }),
    }
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

    #[tokio::test]
    async fn erased_blob_codec_preserves_both_transformation_directions() {
        let model = define_model(
            "blobs",
            || 1_u64,
            ModelOptions {
                blobs: Some(Arc::new(CounterCodec)),
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
