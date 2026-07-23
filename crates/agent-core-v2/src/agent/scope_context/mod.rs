//! Seeded per-agent identity and persistence scope.
//!
//! Original: `packages/agent-core-v2/src/agent/scopeContext/scopeContext.ts`.

use crate::_base::di::instantiation::ServiceIdentifier;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentScopeContext {
    pub agent_id: String,
    agent_scope: String,
}

impl AgentScopeContext {
    // Original: IAgentScopeContext.scope().
    pub fn scope(&self, sub_key: Option<&str>) -> String {
        match (self.agent_scope.as_str(), sub_key) {
            (_, None | Some("")) => self.agent_scope.clone(),
            ("", Some(sub_key)) => sub_key.to_owned(),
            (agent_scope, Some(sub_key)) => format!("{agent_scope}/{sub_key}"),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentScopeContextInput {
    pub agent_id: String,
    pub agent_scope: String,
}

// Original: makeAgentScopeContext().
pub fn make_agent_scope_context(input: AgentScopeContextInput) -> AgentScopeContext {
    AgentScopeContext {
        agent_id: input.agent_id,
        agent_scope: input.agent_scope,
    }
}

pub const AGENT_SCOPE_CONTEXT_ID: ServiceIdentifier<AgentScopeContext> =
    ServiceIdentifier::new("agentScopeContext");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn joins_only_nonempty_subkeys_and_preserves_empty_base_behavior() {
        let context = make_agent_scope_context(AgentScopeContextInput {
            agent_id: "main".to_owned(),
            agent_scope: "sessions/w/s/agents/main".to_owned(),
        });
        assert_eq!(context.agent_id, "main");
        assert_eq!(context.scope(None), "sessions/w/s/agents/main");
        assert_eq!(context.scope(Some("")), "sessions/w/s/agents/main");
        assert_eq!(context.scope(Some("cron")), "sessions/w/s/agents/main/cron");

        let root = make_agent_scope_context(AgentScopeContextInput {
            agent_id: "main".to_owned(),
            agent_scope: String::new(),
        });
        assert_eq!(root.scope(Some("cron")), "cron");
        assert_eq!(AGENT_SCOPE_CONTEXT_ID.to_string(), "agentScopeContext");
    }
}
