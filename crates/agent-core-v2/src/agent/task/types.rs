//! Extensible Agent-managed task contracts.
//!
//! Original: `packages/agent-core-v2/src/agent/task/types.ts`.
//!
//! Rust adaptation: TypeScript declaration merging is represented by a stable
//! base plus flattened JSON details, so independent task modules can add kinds
//! without introducing a dependency cycle back into this contract module.

use std::error::Error;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::_base::utils::abort::AbortSignal;

pub type AgentTaskError = Box<dyn Error + Send + Sync>;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentTaskStatus {
    Running,
    Completed,
    Failed,
    TimedOut,
    Killed,
    Lost,
}

impl AgentTaskStatus {
    pub const fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Failed | Self::TimedOut | Self::Killed | Self::Lost
        )
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentTaskSettlementStatus {
    Completed,
    Failed,
    TimedOut,
    Killed,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentTaskSettlement {
    pub status: AgentTaskSettlementStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentTaskInfoBase {
    pub task_id: String,
    pub description: String,
    pub status: AgentTaskStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detached: Option<bool>,
    pub started_at: i64,
    pub ended_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub terminal_notification_suppressed: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct AgentTaskInfo {
    #[serde(flatten)]
    pub base: AgentTaskInfoBase,
    pub kind: String,
    #[serde(flatten)]
    pub details: Map<String, Value>,
}

#[async_trait]
pub trait AgentTaskSink: Send + Sync {
    fn signal(&self) -> AbortSignal;
    fn append_output(&self, chunk: &str);
    async fn settle(&self, settlement: AgentTaskSettlement) -> Result<bool, AgentTaskError>;
}

#[async_trait]
pub trait AgentTask: Send + Sync {
    fn id_prefix(&self) -> &str;
    fn kind(&self) -> &str;
    fn description(&self) -> &str;
    fn timeout_ms(&self) -> Option<u64> {
        None
    }
    async fn start(&self, sink: &dyn AgentTaskSink) -> Result<(), AgentTaskError>;
    fn on_detach(&self) {}
    async fn force_stop(&self) -> Result<(), AgentTaskError> {
        Ok(())
    }
    fn to_info(&self, base: AgentTaskInfoBase) -> AgentTaskInfo;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn statuses_and_extensible_info_preserve_external_shapes() {
        assert!(AgentTaskStatus::Completed.is_terminal());
        assert!(!AgentTaskStatus::Running.is_terminal());
        let info = AgentTaskInfo {
            base: AgentTaskInfoBase {
                task_id: "bash-1".into(),
                description: "command".into(),
                status: AgentTaskStatus::Running,
                detached: Some(true),
                started_at: 10,
                ended_at: None,
                stop_reason: None,
                terminal_notification_suppressed: None,
                timeout_ms: Some(1000),
            },
            kind: "process".into(),
            details: Map::from_iter([
                ("command".into(), Value::String("pwd".into())),
                ("pid".into(), Value::from(42)),
            ]),
        };
        let value = serde_json::to_value(info).unwrap();
        assert_eq!(value["taskId"], "bash-1");
        assert_eq!(value["status"], "running");
        assert_eq!(value["kind"], "process");
        assert_eq!(value["command"], "pwd");
        assert!(value["endedAt"].is_null());
    }
}
