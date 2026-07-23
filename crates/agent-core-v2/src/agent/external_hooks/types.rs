//! External hook event and result models.
//!
//! Original: `packages/agent-core-v2/src/agent/externalHooks/types.ts`.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::kosong::contract::message::ContentPart;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
pub enum HookEventType {
    PreToolUse,
    PostToolUse,
    PostToolUseFailure,
    PermissionRequest,
    PermissionResult,
    UserPromptSubmit,
    Stop,
    StopFailure,
    Interrupt,
    SessionStart,
    SessionEnd,
    SubagentStart,
    SubagentStop,
    PreCompact,
    PostCompact,
    Notification,
}

impl HookEventType {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::PreToolUse => "PreToolUse",
            Self::PostToolUse => "PostToolUse",
            Self::PostToolUseFailure => "PostToolUseFailure",
            Self::PermissionRequest => "PermissionRequest",
            Self::PermissionResult => "PermissionResult",
            Self::UserPromptSubmit => "UserPromptSubmit",
            Self::Stop => "Stop",
            Self::StopFailure => "StopFailure",
            Self::Interrupt => "Interrupt",
            Self::SessionStart => "SessionStart",
            Self::SessionEnd => "SessionEnd",
            Self::SubagentStart => "SubagentStart",
            Self::SubagentStop => "SubagentStop",
            Self::PreCompact => "PreCompact",
            Self::PostCompact => "PostCompact",
            Self::Notification => "Notification",
        }
    }
}

pub const HOOK_EVENT_TYPES: [HookEventType; 16] = [
    HookEventType::PreToolUse,
    HookEventType::PostToolUse,
    HookEventType::PostToolUseFailure,
    HookEventType::PermissionRequest,
    HookEventType::PermissionResult,
    HookEventType::UserPromptSubmit,
    HookEventType::Stop,
    HookEventType::StopFailure,
    HookEventType::Interrupt,
    HookEventType::SessionStart,
    HookEventType::SessionEnd,
    HookEventType::SubagentStart,
    HookEventType::SubagentStop,
    HookEventType::PreCompact,
    HookEventType::PostCompact,
    HookEventType::Notification,
];

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HookDef {
    pub event: HookEventType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub matcher: Option<String>,
    pub command: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env: Option<HashMap<String, String>>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HookAction {
    Allow,
    Block,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HookResult {
    pub action: HookAction,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stdout: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stderr: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timed_out: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub structured_output: Option<bool>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct HookBlockDecision {
    pub block: bool,
    pub reason: String,
}

impl HookBlockDecision {
    pub fn new(reason: impl Into<String>) -> Self {
        Self {
            block: true,
            reason: reason.into(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum HookMatcherValue {
    String(String),
    Content(Vec<ContentPart>),
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn event_registry_preserves_all_original_wire_names_in_order() {
        assert_eq!(HOOK_EVENT_TYPES.len(), 16);
        assert_eq!(
            serde_json::to_value(HOOK_EVENT_TYPES).unwrap(),
            json!([
                "PreToolUse",
                "PostToolUse",
                "PostToolUseFailure",
                "PermissionRequest",
                "PermissionResult",
                "UserPromptSubmit",
                "Stop",
                "StopFailure",
                "Interrupt",
                "SessionStart",
                "SessionEnd",
                "SubagentStart",
                "SubagentStop",
                "PreCompact",
                "PostCompact",
                "Notification"
            ])
        );
    }

    #[test]
    fn results_and_matchers_preserve_camel_case_and_union_shapes() {
        let result = HookResult {
            action: HookAction::Block,
            message: None,
            reason: Some("denied".into()),
            stdout: None,
            stderr: None,
            exit_code: Some(2),
            timed_out: Some(false),
            structured_output: Some(true),
        };
        assert_eq!(
            serde_json::to_value(result).unwrap(),
            json!({
                "action": "block", "reason": "denied", "exitCode": 2,
                "timedOut": false, "structuredOutput": true
            })
        );
        assert!(HookBlockDecision::new("stop").block);
        assert_eq!(
            serde_json::to_value(HookMatcherValue::String("tool".into())).unwrap(),
            "tool"
        );
    }
}
