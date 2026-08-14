//! Live provenance for one logical LLM request.
//!
//! Original: `packages/agent-core-v2/src/kosong/contract/requestTrace.ts`.

use parking_lot::Mutex;
use std::fmt;
use std::sync::Arc;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

#[derive(Clone, Default)]
pub struct LlmRequestTrace {
    trace_id: Arc<Mutex<Option<String>>>,
}

impl LlmRequestTrace {
    pub fn new(trace_id: Option<String>) -> Self {
        Self {
            trace_id: Arc::new(Mutex::new(trace_id)),
        }
    }

    pub fn trace_id(&self) -> Option<String> {
        self.trace_id.lock().clone()
    }

    pub(crate) fn set_trace_id(&self, trace_id: Option<String>) {
        *self.trace_id.lock() = trace_id;
    }
}

impl fmt::Debug for LlmRequestTrace {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LlmRequestTrace")
            .field("trace_id", &self.trace_id())
            .finish()
    }
}

impl PartialEq for LlmRequestTrace {
    fn eq(&self, other: &Self) -> bool {
        self.trace_id() == other.trace_id()
    }
}

impl Eq for LlmRequestTrace {}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct SerializedLlmRequestTrace {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    trace_id: Option<String>,
}

impl Serialize for LlmRequestTrace {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        SerializedLlmRequestTrace {
            trace_id: self.trace_id(),
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for LlmRequestTrace {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = SerializedLlmRequestTrace::deserialize(deserializer)?;
        Ok(Self::new(value.trace_id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pending_and_resolved_trace_shapes_match_the_contract() {
        assert_eq!(
            serde_json::to_value(LlmRequestTrace::default()).unwrap(),
            serde_json::json!({})
        );
        assert_eq!(
            serde_json::to_value(LlmRequestTrace::new(Some("trace-123".to_owned()))).unwrap(),
            serde_json::json!({"traceId": "trace-123"})
        );
    }

    #[test]
    fn clones_observe_live_trace_updates() {
        let trace = LlmRequestTrace::default();
        let observer = trace.clone();
        trace.set_trace_id(Some("trace-live".into()));
        assert_eq!(observer.trace_id().as_deref(), Some("trace-live"));

        let restored: LlmRequestTrace =
            serde_json::from_value(serde_json::json!({"traceId": "trace-restored"})).unwrap();
        assert_eq!(restored.trace_id().as_deref(), Some("trace-restored"));
    }
}
