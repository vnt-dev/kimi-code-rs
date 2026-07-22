use serde::{Deserialize, Serialize};

use super::time::IsoDateTime;
use super::validation::non_empty;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskKind {
    Subagent,
    Bash,
    Tool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskStatus {
    Running,
    Completed,
    Failed,
    Cancelled,
}

// Original: task.ts, taskSchema.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Task {
    #[serde(deserialize_with = "non_empty")]
    pub id: String,
    #[serde(deserialize_with = "non_empty")]
    pub session_id: String,
    pub kind: TaskKind,
    pub description: String,
    pub status: TaskStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    pub created_at: IsoDateTime,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_at: Option<IsoDateTime>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<IsoDateTime>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_preview: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_bytes: Option<u64>,
}

pub type BackgroundTaskKind = TaskKind;
pub type BackgroundTaskStatus = TaskStatus;
pub type BackgroundTask = Task;
