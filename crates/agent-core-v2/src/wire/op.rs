//! Strongly typed Op definitions with an erased global replay registry.
//!
//! Original: `packages/agent-core-v2/src/wire/op.ts`.

use std::{
    any::Any,
    collections::HashMap,
    error::Error,
    fmt,
    sync::{Arc, LazyLock, RwLock},
};

use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Map, Value};

use super::{
    errors::{WIRE_DUPLICATE_OP, WireError},
    model::{ErasedModelDef, ErasedState, ModelDef},
};

pub type ErasedPayload = Box<dyn Any + Send + Sync>;
pub type PayloadParser<P> = Arc<dyn Fn(&Value) -> Result<P, String> + Send + Sync>;
pub type ApplyOp<S, P> = Arc<dyn Fn(S, &P) -> S + Send + Sync>;
pub type OpEvent<S, P> = Arc<dyn Fn(&P, &S) -> Option<Value> + Send + Sync>;

pub struct DefineOpOptions<S, P> {
    pub parse_payload: PayloadParser<P>,
    pub apply: ApplyOp<S, P>,
    pub to_event: Option<OpEvent<S, P>>,
    pub persist: Option<bool>,
}

impl<S, P> DefineOpOptions<S, P>
where
    P: DeserializeOwned,
{
    pub fn new(apply: impl Fn(S, &P) -> S + Send + Sync + 'static) -> Self {
        Self {
            parse_payload: Arc::new(|value| {
                serde_json::from_value(value.clone()).map_err(|error| error.to_string())
            }),
            apply: Arc::new(apply),
            to_event: None,
            persist: None,
        }
    }
}

pub trait ErasedOpDescriptor: Send + Sync {
    fn op_type(&self) -> &str;
    fn model(&self) -> Arc<dyn ErasedModelDef>;
    fn persist(&self) -> Option<bool>;
    fn parse_payload(&self, payload: &Value) -> Result<ErasedPayload, String>;
    fn apply(&self, state: ErasedState, payload: &dyn Any) -> Result<ErasedState, OpTypeError>;
    fn to_event(&self, payload: &dyn Any, state: &dyn Any) -> Result<Option<Value>, OpTypeError>;
}

pub struct OpDescriptor<S, P> {
    op_type: String,
    model: ModelDef<S>,
    parse_payload: PayloadParser<P>,
    apply: ApplyOp<S, P>,
    to_event: Option<OpEvent<S, P>>,
    persist: Option<bool>,
}

impl<S, P> OpDescriptor<S, P> {
    pub fn op_type(&self) -> &str {
        &self.op_type
    }

    pub fn model_def(&self) -> &ModelDef<S> {
        &self.model
    }

    pub fn persist_value(&self) -> Option<bool> {
        self.persist
    }
}

impl<S, P> ErasedOpDescriptor for OpDescriptor<S, P>
where
    S: Send + Sync + 'static,
    P: Send + Sync + 'static,
{
    fn op_type(&self) -> &str {
        &self.op_type
    }

    fn model(&self) -> Arc<dyn ErasedModelDef> {
        self.model.erased()
    }

    fn persist(&self) -> Option<bool> {
        self.persist
    }

    fn parse_payload(&self, payload: &Value) -> Result<ErasedPayload, String> {
        (self.parse_payload)(payload).map(|payload| Box::new(payload) as ErasedPayload)
    }

    fn apply(&self, state: ErasedState, payload: &dyn Any) -> Result<ErasedState, OpTypeError> {
        let state = state.downcast::<S>().map_err(|_| OpTypeError::State {
            op_type: self.op_type.clone(),
            model: self.model.name().into(),
        })?;
        let payload = payload
            .downcast_ref::<P>()
            .ok_or_else(|| OpTypeError::Payload {
                op_type: self.op_type.clone(),
            })?;
        Ok(Box::new((self.apply)(*state, payload)))
    }

    fn to_event(&self, payload: &dyn Any, state: &dyn Any) -> Result<Option<Value>, OpTypeError> {
        let Some(to_event) = &self.to_event else {
            return Ok(None);
        };
        let payload = payload
            .downcast_ref::<P>()
            .ok_or_else(|| OpTypeError::Payload {
                op_type: self.op_type.clone(),
            })?;
        let state = state
            .downcast_ref::<S>()
            .ok_or_else(|| OpTypeError::State {
                op_type: self.op_type.clone(),
                model: self.model.name().into(),
            })?;
        Ok(to_event(payload, state))
    }
}

#[derive(Debug, thiserror::Error)]
pub enum OpTypeError {
    #[error("Op '{op_type}' received an incompatible model state for '{model}'")]
    State { op_type: String, model: String },
    #[error("Op '{op_type}' received an incompatible payload")]
    Payload { op_type: String },
}

pub struct DefinedOp<S, P> {
    descriptor: Arc<OpDescriptor<S, P>>,
}

impl<S, P> Clone for DefinedOp<S, P> {
    fn clone(&self) -> Self {
        Self {
            descriptor: Arc::clone(&self.descriptor),
        }
    }
}

impl<S, P> DefinedOp<S, P>
where
    S: Send + Sync + 'static,
    P: Serialize + Send + Sync + 'static,
{
    pub fn op_type(&self) -> &str {
        self.descriptor.op_type()
    }

    pub fn descriptor(&self) -> &Arc<OpDescriptor<S, P>> {
        &self.descriptor
    }

    pub fn create(&self, payload: P) -> Result<Op, serde_json::Error> {
        let payload_value = serde_json::to_value(&payload)?;
        let descriptor: Arc<dyn ErasedOpDescriptor> = self.descriptor.clone();
        Ok(Op {
            op_type: self.op_type().into(),
            payload: Box::new(payload),
            payload_value,
            descriptor,
        })
    }
}

pub struct Op {
    pub op_type: String,
    payload: ErasedPayload,
    pub payload_value: Value,
    pub descriptor: Arc<dyn ErasedOpDescriptor>,
}

impl Op {
    pub fn payload(&self) -> &dyn Any {
        self.payload.as_ref()
    }

    pub fn from_wire(
        descriptor: Arc<dyn ErasedOpDescriptor>,
        payload_value: Value,
    ) -> Result<Self, String> {
        let payload = descriptor.parse_payload(&payload_value)?;
        Ok(Self {
            op_type: descriptor.op_type().into(),
            payload,
            payload_value,
            descriptor,
        })
    }
}

static OP_REGISTRY: LazyLock<RwLock<HashMap<String, Arc<dyn ErasedOpDescriptor>>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

pub fn registered_op(op_type: &str) -> Option<Arc<dyn ErasedOpDescriptor>> {
    OP_REGISTRY.read().unwrap().get(op_type).cloned()
}

// Original: defineOp(). Registration and duplicate detection are atomic under
// the registry write lock, preserving the fail-fast global namespace.
pub fn define_op<S, P>(
    model: ModelDef<S>,
    op_type: impl Into<String>,
    options: DefineOpOptions<S, P>,
) -> Result<DefinedOp<S, P>, DuplicateOpError>
where
    S: Send + Sync + 'static,
    P: Serialize + Send + Sync + 'static,
{
    let op_type = op_type.into();
    let descriptor = Arc::new(OpDescriptor {
        op_type: op_type.clone(),
        model,
        parse_payload: options.parse_payload,
        apply: options.apply,
        to_event: options.to_event,
        persist: options.persist,
    });
    let erased: Arc<dyn ErasedOpDescriptor> = descriptor.clone();
    let mut registry = OP_REGISTRY.write().unwrap();
    if registry.contains_key(&op_type) {
        return Err(DuplicateOpError::new(op_type));
    }
    registry.insert(op_type, erased);
    Ok(DefinedOp { descriptor })
}

#[derive(Debug)]
pub struct DuplicateOpError {
    pub op_type: String,
    inner: Box<WireError>,
}

impl DuplicateOpError {
    fn new(op_type: String) -> Self {
        let details = Map::from_iter([("type".into(), Value::String(op_type.clone()))]);
        let inner = WireError::with_options(
            WIRE_DUPLICATE_OP,
            format!("Duplicate Op type registered: '{op_type}'"),
            crate::_base::errors::errors::Error2Options {
                details: Some(details),
                name: Some("DuplicateOpError".into()),
                ..crate::_base::errors::errors::Error2Options::default()
            },
        );
        Self {
            op_type,
            inner: Box::new(inner),
        }
    }

    pub fn error(&self) -> &WireError {
        &self.inner
    }
}

impl fmt::Display for DuplicateOpError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.inner.fmt(formatter)
    }
}

impl Error for DuplicateOpError {}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use serde::{Deserialize, Serialize};

    use super::*;
    use crate::wire::{
        model::{ModelOptions, define_model},
        record::op_to_wire_record_at,
    };

    static NEXT_TEST_OP: AtomicU64 = AtomicU64::new(1);

    fn unique(name: &str) -> String {
        format!(
            "test.{name}.{}",
            NEXT_TEST_OP.fetch_add(1, Ordering::Relaxed)
        )
    }

    #[derive(Debug, Deserialize, Serialize)]
    struct AddPayload {
        amount: u64,
    }

    #[test]
    fn typed_definition_creates_erased_ops_and_applies_replayed_payloads() {
        let model = define_model("counter", || 1_u64, ModelOptions::default());
        let op_type = unique("add");
        let mut options =
            DefineOpOptions::new(|state, payload: &AddPayload| state + payload.amount);
        options.to_event = Some(Arc::new(|payload, state| {
            Some(serde_json::json!({"amount": payload.amount, "state": state}))
        }));
        let add = model.define_op(&op_type, options).unwrap();
        let live = add.create(AddPayload { amount: 2 }).unwrap();
        assert_eq!(live.op_type, op_type);
        assert_eq!(live.payload_value, serde_json::json!({"amount": 2}));
        assert_eq!(
            Value::Object(op_to_wire_record_at(&live, 9)),
            serde_json::json!({"type": op_type, "amount": 2, "time": 9})
        );

        let descriptor = registered_op(&op_type).unwrap();
        let replay = Op::from_wire(descriptor.clone(), serde_json::json!({"amount": 4})).unwrap();
        let event = descriptor
            .to_event(replay.payload(), &5_u64)
            .unwrap()
            .unwrap();
        assert_eq!(event, serde_json::json!({"amount": 4, "state": 5}));
        let state = descriptor.apply(Box::new(1_u64), replay.payload()).unwrap();
        assert_eq!(*state.downcast::<u64>().unwrap(), 5);
    }

    #[test]
    fn duplicate_registration_fails_without_overwriting_first_descriptor() {
        let model = define_model("duplicate", || 0_u64, ModelOptions::default());
        let op_type = unique("duplicate");
        model
            .define_op(&op_type, DefineOpOptions::new(|state, _: &()| state + 1))
            .unwrap();
        let error = model
            .define_op(&op_type, DefineOpOptions::new(|state, _: &()| state + 2))
            .err()
            .unwrap();
        assert_eq!(error.op_type, op_type);
        assert_eq!(error.error().code(), WIRE_DUPLICATE_OP);
        assert!(error.to_string().contains("Duplicate Op type registered"));
    }

    #[test]
    fn replay_parser_rejects_malformed_payload_before_apply() {
        let model = define_model("validated", String::new, ModelOptions::default());
        let op_type = unique("validated");
        model
            .define_op(
                &op_type,
                DefineOpOptions::new(|mut state: String, payload: &AddPayload| {
                    state.push_str(&payload.amount.to_string());
                    state
                }),
            )
            .unwrap();
        let descriptor = registered_op(&op_type).unwrap();
        assert!(Op::from_wire(descriptor, serde_json::json!({"amount": "bad"})).is_err());
    }
}
