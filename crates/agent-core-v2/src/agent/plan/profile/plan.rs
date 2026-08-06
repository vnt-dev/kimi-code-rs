//! Built-in read-only planning subagent profile.
//!
//! Original: `agent/plan/profile/plan.ts`.

use std::sync::{Arc, Once};

use crate::app::agent_profile_catalog::{
    AgentProfile, SkillActiveOptions, TASK_AGENT_ROLE_PREFIX, register_agent_profile,
    render_system_prompt, skill_active_for,
};

const PLAN_TOOLS: &[&str] = &[
    "Read",
    "ReadMediaFile",
    "Glob",
    "Grep",
    "WebSearch",
    "FetchURL",
];

const PLAN_ROLE: &str = "Before designing your implementation plan, consider whether you fully understand the codebase areas \
relevant to the task. If not, recommend the parent agent to use the explore agent \
(subagent_type=\"explore\") to investigate key questions first. In your response, clearly state:\n\
1. What you already know from the information provided\n\
2. What questions remain unanswered that would benefit from explore agent investigation\n\
3. Your implementation plan (either preliminary if questions remain, or final if sufficient context exists)\n\n\
You are a read-only planning agent: you can read and search files (Read, Glob, Grep, ReadMediaFile) \
and consult the web (WebSearch, FetchURL), but you have no shell and no file-editing tools. \
Where the general instructions tell you to make changes with tools, that does not apply to you — \
do not attempt to run commands or modify files. Your deliverable is the plan itself, returned as \
your final message.";

fn plan_profile() -> AgentProfile {
    let tools = PLAN_TOOLS
        .iter()
        .map(|tool| (*tool).to_owned())
        .collect::<Vec<_>>();
    let skill_active = skill_active_for(&tools);
    let role = format!("{TASK_AGENT_ROLE_PREFIX}\n\n{PLAN_ROLE}");
    AgentProfile {
        name: "plan".into(),
        description: Some("Read-only implementation planning and architecture design.".into()),
        when_to_use: Some(
            "Use this agent when the parent agent needs a step-by-step implementation plan, key file identification, and architectural trade-off analysis before code changes are made.".into(),
        ),
        is_override: None,
        tools: Some(tools),
        disallowed_tools: None,
        subagents: None,
        model: None,
        system_prompt: Arc::new(move |context| {
            render_system_prompt(
                &role,
                context,
                SkillActiveOptions { skill_active },
            )
        }),
        prompt_prefix: None,
        summary_policy: None,
    }
}

/// Registers the source `plan` profile once, mirroring its import-time side
/// effect without duplicating the contribution when runtimes are rebuilt.
pub fn register_plan_agent_profile() {
    static REGISTER: Once = Once::new();
    REGISTER.call_once(|| register_agent_profile(plan_profile()));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::agent_profile_catalog::AgentProfileContext;

    #[test]
    fn profile_matches_the_source_toolset_and_read_only_role() {
        let profile = plan_profile();
        let expected_tools = PLAN_TOOLS
            .iter()
            .map(|tool| (*tool).to_owned())
            .collect::<Vec<_>>();
        assert_eq!(profile.name, "plan");
        assert_eq!(profile.tools.as_deref(), Some(expected_tools.as_slice()));
        let prompt = profile.render_system_prompt(&AgentProfileContext::default());
        assert!(prompt.contains(TASK_AGENT_ROLE_PREFIX));
        assert!(prompt.contains("read-only planning agent"));
        assert!(prompt.contains("no shell and no file-editing tools"));
    }
}
