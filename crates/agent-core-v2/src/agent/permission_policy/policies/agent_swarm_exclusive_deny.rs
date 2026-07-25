//! Enforces the source batch-exclusivity rule for `AgentSwarm` calls.
//!
//! Original: `agent/permissionPolicy/policies/agent-swarm-exclusive-deny.ts`.

use futures_util::FutureExt;

use crate::agent::{
    permission_policy::{
        PermissionDecisionReason, PermissionPolicy, PermissionPolicyFuture, PermissionPolicyResult,
        PermissionReasonValue,
    },
    tool_executor::ResolvedToolExecutionHookContext,
};

#[derive(Default)]
pub struct AgentSwarmExclusiveDenyPermissionPolicy;

pub fn multiple_agent_swarm_denied_message(has_other_tool_calls: bool) -> String {
    let suffix = if has_other_tool_calls {
        " AgentSwarm also must not be combined with other tools in the same response."
    } else {
        ""
    };
    format!(
        "AgentSwarm must be called one swarm at a time. Multiple AgentSwarm calls are not forbidden, but issue them sequentially: call one AgentSwarm, wait for its result, then call the next; or merge the work into a single AgentSwarm when one swarm can cover it.{suffix}"
    )
}

pub const fn mixed_agent_swarm_denied_message() -> &'static str {
    "AgentSwarm must be the only tool call in a model response. Retry with a single AgentSwarm call by itself, then call any other tools after it returns."
}

// Original: AgentSwarmExclusiveDenyPermissionPolicyService.evaluate().
pub fn agent_swarm_exclusive_denial(
    context: &ResolvedToolExecutionHookContext,
) -> Option<PermissionPolicyResult> {
    let agent_swarm_count = context
        .tool_calls
        .iter()
        .filter(|call| call.name == "AgentSwarm")
        .count();
    if agent_swarm_count == 0 || (agent_swarm_count == 1 && context.tool_calls.len() == 1) {
        return None;
    }
    let message = if agent_swarm_count > 1 {
        multiple_agent_swarm_denied_message(context.tool_calls.len() > agent_swarm_count)
    } else {
        mixed_agent_swarm_denied_message().into()
    };
    let mut reason = PermissionDecisionReason::new();
    reason.insert(
        "agent_swarm_tool_calls".into(),
        PermissionReasonValue::Number(agent_swarm_count as f64),
    );
    reason.insert(
        "tool_calls".into(),
        PermissionReasonValue::Number(context.tool_calls.len() as f64),
    );
    Some(PermissionPolicyResult::Deny {
        reason: Some(reason),
        message: Some(message),
    })
}

impl PermissionPolicy for AgentSwarmExclusiveDenyPermissionPolicy {
    fn name(&self) -> &str {
        "agent-swarm-exclusive-deny"
    }
    fn evaluate<'a>(
        &'a self,
        context: &'a ResolvedToolExecutionHookContext,
    ) -> PermissionPolicyFuture<'a> {
        async move { agent_swarm_exclusive_denial(context) }.boxed()
    }
}
