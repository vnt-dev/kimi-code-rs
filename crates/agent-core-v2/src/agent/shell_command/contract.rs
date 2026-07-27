//! Agent-scoped shell command contract.
//!
//! Original: `packages/agent-core-v2/src/agent/shellCommand/shellCommand.ts`.

use std::{error::Error, ops::Deref, sync::Arc};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::_base::di::instantiation::ServiceIdentifier;

pub type ShellCommandServiceError = Box<dyn Error + Send + Sync>;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunShellCommandInput {
    pub command: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command_id: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunShellCommandResult {
    pub stdout: String,
    pub stderr: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_error: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backgrounded: Option<bool>,
}

#[async_trait]
pub trait AgentShellCommandServiceContract: Send + Sync {
    async fn run(
        &self,
        input: RunShellCommandInput,
    ) -> Result<RunShellCommandResult, ShellCommandServiceError>;
    fn cancel(&self, command_id: &str);
}

#[derive(Clone)]
pub struct AgentShellCommandServiceHandle(pub Arc<dyn AgentShellCommandServiceContract>);

impl Deref for AgentShellCommandServiceHandle {
    type Target = dyn AgentShellCommandServiceContract;

    fn deref(&self) -> &Self::Target {
        self.0.as_ref()
    }
}

pub const AGENT_SHELL_COMMAND_SERVICE_ID: ServiceIdentifier<AgentShellCommandServiceHandle> =
    ServiceIdentifier::new("agentShellCommandService");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_identity_and_wire_shape_match_source() {
        assert_eq!(
            AGENT_SHELL_COMMAND_SERVICE_ID.to_string(),
            "agentShellCommandService"
        );
        assert_eq!(
            serde_json::to_value(RunShellCommandInput {
                command: "pwd".into(),
                command_id: Some("command-1".into()),
            })
            .unwrap(),
            serde_json::json!({"command": "pwd", "commandId": "command-1"})
        );
    }
}
