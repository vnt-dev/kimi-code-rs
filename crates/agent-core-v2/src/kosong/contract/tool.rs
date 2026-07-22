use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

// Original:
//   packages/agent-core-v2/src/kosong/contract/tool.ts
//   Tool
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Tool {
    pub name: String,
    pub description: String,
    pub parameters: Map<String, Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deferred: Option<bool>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_preserves_provider_agnostic_wire_shape() {
        let tool = Tool {
            name: "read".to_owned(),
            description: "Read a file".to_owned(),
            parameters: serde_json::from_value(serde_json::json!({
                "type": "object",
                "properties": {"path": {"type": "string"}},
            }))
            .unwrap(),
            deferred: Some(true),
        };
        assert_eq!(
            serde_json::to_value(tool).unwrap(),
            serde_json::json!({
                "name": "read",
                "description": "Read a file",
                "parameters": {
                    "type": "object",
                    "properties": {"path": {"type": "string"}},
                },
                "deferred": true,
            })
        );
    }
}
