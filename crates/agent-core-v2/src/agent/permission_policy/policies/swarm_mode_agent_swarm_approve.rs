//! Approves the `AgentSwarm` tool while swarm mode is active.
//!
//! Original: `agent/permissionPolicy/policies/swarm-mode-agent-swarm-approve.ts`.

use std::sync::Arc;

use futures_util::FutureExt;

use crate::agent::{
    permission_policy::{PermissionPolicy, PermissionPolicyFuture, PermissionPolicyResult},
    swarm::AgentSwarmServiceContract,
    tool_executor::ResolvedToolExecutionHookContext,
};

pub struct SwarmModeAgentSwarmApprovePermissionPolicy {
    swarm: Arc<dyn AgentSwarmServiceContract>,
}
impl SwarmModeAgentSwarmApprovePermissionPolicy {
    pub fn new(swarm: Arc<dyn AgentSwarmServiceContract>) -> Self {
        Self { swarm }
    }
}

impl PermissionPolicy for SwarmModeAgentSwarmApprovePermissionPolicy {
    fn name(&self) -> &str {
        "swarm-mode-agent-swarm-approve"
    }
    // Original: SwarmModeAgentSwarmApprovePermissionPolicyService.evaluate().
    fn evaluate<'a>(
        &'a self,
        context: &'a ResolvedToolExecutionHookContext,
    ) -> PermissionPolicyFuture<'a> {
        async move {
            (context.tool_call.name == "AgentSwarm" && self.swarm.is_active()).then_some(
                PermissionPolicyResult::Approve {
                    reason: None,
                    execution_metadata: None,
                },
            )
        }
        .boxed()
    }
}
