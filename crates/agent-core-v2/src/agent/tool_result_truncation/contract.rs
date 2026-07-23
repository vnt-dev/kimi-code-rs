use std::{ops::Deref, sync::Arc};

use async_trait::async_trait;

use crate::{
    _base::di::instantiation::ServiceIdentifier, tool::tool_contract::ExecutableToolResult,
};

pub struct ToolResultTruncationInput {
    pub tool_name: String,
    pub tool_call_id: String,
    pub result: ExecutableToolResult,
}

#[async_trait]
pub trait AgentToolResultTruncationServiceContract: Send + Sync {
    async fn truncate_for_model(&self, input: ToolResultTruncationInput) -> ExecutableToolResult;
}

#[derive(Clone)]
pub struct AgentToolResultTruncationServiceHandle(
    pub Arc<dyn AgentToolResultTruncationServiceContract>,
);

impl Deref for AgentToolResultTruncationServiceHandle {
    type Target = dyn AgentToolResultTruncationServiceContract;

    fn deref(&self) -> &Self::Target {
        self.0.as_ref()
    }
}

// Original:
//   packages/agent-core-v2/src/agent/toolResultTruncation/toolResultTruncation.ts
//   IAgentToolResultTruncationService
pub const AGENT_TOOL_RESULT_TRUNCATION_SERVICE_ID: ServiceIdentifier<
    AgentToolResultTruncationServiceHandle,
> = ServiceIdentifier::new("agentToolResultTruncationService");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_identifier_matches_source() {
        assert_eq!(
            AGENT_TOOL_RESULT_TRUNCATION_SERVICE_ID.to_string(),
            "agentToolResultTruncationService"
        );
    }
}
