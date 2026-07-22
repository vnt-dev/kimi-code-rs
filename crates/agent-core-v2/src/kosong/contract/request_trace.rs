use serde::{Deserialize, Serialize};

// Original:
//   packages/agent-core-v2/src/kosong/contract/requestTrace.ts
//   LLMRequestTrace
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct LlmRequestTrace {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trace_id: Option<String>,
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
            serde_json::to_value(LlmRequestTrace {
                trace_id: Some("trace-123".to_owned()),
            })
            .unwrap(),
            serde_json::json!({"traceId": "trace-123"})
        );
    }
}
