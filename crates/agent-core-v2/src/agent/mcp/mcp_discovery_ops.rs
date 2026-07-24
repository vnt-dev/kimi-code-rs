//! Durable MCP tool-discovery de-duplication state.
//!
//! Original: `agent/mcp/mcpDiscoveryOps.ts`.

use std::sync::LazyLock;

use serde::{Deserialize, Serialize};

use crate::wire::{
    model::{ModelDef, ModelOptions, define_model},
    op::{DefineOpOptions, DefinedOp, Op},
};

use super::McpToolDefinition;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum McpToolCollisionWith {
    SameServer { tool_name: String },
    OtherServer { server_name: String },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpToolCollision {
    pub qualified: String,
    pub tool_name: String,
    pub collides_with: McpToolCollisionWith,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct McpDiscoveryState {
    pub seen: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpToolsDiscoveredPayload {
    pub server_name: String,
    pub hash: String,
    pub tools: Vec<McpToolDefinition>,
    pub enabled_names: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub collisions: Option<Vec<McpToolCollision>>,
}

pub static MCP_DISCOVERY_MODEL: LazyLock<ModelDef<McpDiscoveryState>> = LazyLock::new(|| {
    define_model(
        "mcp.discovery",
        McpDiscoveryState::default,
        ModelOptions::default(),
    )
});

pub static MCP_TOOLS_DISCOVERED: LazyLock<DefinedOp<McpDiscoveryState, McpToolsDiscoveredPayload>> =
    LazyLock::new(|| {
        MCP_DISCOVERY_MODEL
            .define_op(
                "mcp.tools_discovered",
                DefineOpOptions::new(apply_mcp_tools_discovered),
            )
            .expect("mcp.tools_discovered must have one global definition")
    });

// Original: mcpToolsDiscovered.apply().
pub fn apply_mcp_tools_discovered(
    state: McpDiscoveryState,
    payload: &McpToolsDiscoveredPayload,
) -> McpDiscoveryState {
    let key = format!("{}\n{}", payload.server_name, payload.hash);
    if state.seen.contains(&key) {
        return state;
    }
    let mut next = state;
    next.seen.push(key);
    next
}

pub fn mcp_tools_discovered(payload: McpToolsDiscoveredPayload) -> Result<Op, serde_json::Error> {
    MCP_TOOLS_DISCOVERED.create(payload)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn payload(server_name: &str, hash: &str) -> McpToolsDiscoveredPayload {
        McpToolsDiscoveredPayload {
            server_name: server_name.into(),
            hash: hash.into(),
            tools: Vec::new(),
            enabled_names: Vec::new(),
            collisions: None,
        }
    }

    #[test]
    fn retains_first_discovery_key_and_uses_source_wire_names() {
        let first =
            apply_mcp_tools_discovered(McpDiscoveryState::default(), &payload("server", "a"));
        let duplicate = apply_mcp_tools_discovered(first.clone(), &payload("server", "a"));
        let next = apply_mcp_tools_discovered(first, &payload("server", "b"));
        assert_eq!(duplicate.seen, ["server\na"]);
        assert_eq!(next.seen, ["server\na", "server\nb"]);
        let collision = McpToolCollision {
            qualified: "mcp__server__tool".into(),
            tool_name: "tool".into(),
            collides_with: McpToolCollisionWith::OtherServer {
                server_name: "other".into(),
            },
        };
        assert_eq!(
            serde_json::to_value(collision).unwrap(),
            serde_json::json!({
                "qualified": "mcp__server__tool",
                "toolName": "tool",
                "collidesWith": {"kind": "other_server", "serverName": "other"}
            })
        );
    }
}
