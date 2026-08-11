use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

pub type AgentLlmRequestLogFields = Map<String, Value>;

// Original:
//   packages/agent-core-v2/src/agent/llmRequester/llmRequester.ts
//   AgentLLMRequestSource
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum AgentLlmRequestSource {
    Turn {
        #[serde(rename = "turnId")]
        turn_id: crate::agent::TurnId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        step: Option<crate::agent::StepId>,
        #[serde(rename = "logFields", default, skip_serializing_if = "Option::is_none")]
        log_fields: Option<AgentLlmRequestLogFields>,
    },
    Operation {
        #[serde(rename = "turnId", default, skip_serializing_if = "Option::is_none")]
        turn_id: Option<crate::agent::TurnId>,
        #[serde(
            rename = "requestKind",
            default,
            skip_serializing_if = "Option::is_none"
        )]
        request_kind: Option<String>,
        #[serde(rename = "logFields", default, skip_serializing_if = "Option::is_none")]
        log_fields: Option<AgentLlmRequestLogFields>,
    },
}

impl AgentLlmRequestSource {
    pub fn turn_id(&self) -> Option<crate::agent::TurnId> {
        match self {
            Self::Turn { turn_id, .. } => Some(*turn_id),
            Self::Operation { turn_id, .. } => *turn_id,
        }
    }

    pub fn is_turn(&self) -> bool {
        matches!(self, Self::Turn { .. })
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn discriminated_union_preserves_camel_case_fields_and_optional_values() {
        let turn = AgentLlmRequestSource::Turn {
            turn_id: crate::agent::TurnId::new(7),
            step: Some(crate::agent::StepId::new(2)),
            log_fields: Some(Map::from_iter([("projection".into(), json!("strict"))])),
        };
        assert_eq!(
            serde_json::to_value(&turn).unwrap(),
            json!({
                "type": "turn", "turnId": 7, "step": 2,
                "logFields": {"projection": "strict"}
            })
        );
        assert!(turn.is_turn());
        assert_eq!(turn.turn_id(), Some(crate::agent::TurnId::new(7)));

        let operation = AgentLlmRequestSource::Operation {
            turn_id: None,
            request_kind: Some("compaction".into()),
            log_fields: None,
        };
        assert_eq!(
            serde_json::to_value(&operation).unwrap(),
            json!({"type": "operation", "requestKind": "compaction"})
        );
        assert!(!operation.is_turn());
        assert_eq!(operation.turn_id(), None);
    }
}
