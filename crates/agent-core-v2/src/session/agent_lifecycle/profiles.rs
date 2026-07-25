//! Built-in main and task-agent profile contributions.
//!
//! Original: `session/agentLifecycle/profile/profiles.ts`.

use std::sync::{Arc, Once};

use crate::{
    app::agent_profile_catalog::contract::AgentPromptPrefix,
    app::agent_profile_catalog::{
        AgentProfile, AgentProfileSummaryPolicy, TASK_AGENT_ROLE_PREFIX, register_agent_profile,
        render_system_prompt, skill_active_for,
    },
    session::session_fs::collect_git_context,
};

const EXPLORE_ROLE: &str = include_str!("profile_explore_overlay.md");
const SUMMARY_CONTINUATION_PROMPT: &str = include_str!("profile_summary_continuation.md");

const AGENT_TOOLS: &[&str] = &[
    "Read",
    "Write",
    "Edit",
    "Grep",
    "Glob",
    "Bash",
    "TaskList",
    "TaskOutput",
    "TaskStop",
    "CronCreate",
    "CronList",
    "CronDelete",
    "ReadMediaFile",
    "TodoList",
    "Skill",
    "WebSearch",
    "Agent",
    "AgentSwarm",
    "FetchURL",
    "AskUserQuestion",
    "EnterPlanMode",
    "ExitPlanMode",
    "CreateGoal",
    "GetGoal",
    "SetGoalBudget",
    "UpdateGoal",
    "mcp__*",
];
const CODER_TOOLS: &[&str] = &[
    "Agent",
    "AgentSwarm",
    "Bash",
    "CronCreate",
    "CronDelete",
    "CronList",
    "Edit",
    "EnterPlanMode",
    "ExitPlanMode",
    "Glob",
    "Grep",
    "Read",
    "ReadMediaFile",
    "Skill",
    "TaskList",
    "TaskOutput",
    "TaskStop",
    "TodoList",
    "WebSearch",
    "FetchURL",
    "Write",
    "mcp__*",
];
const EXPLORE_TOOLS: &[&str] = &[
    "Bash",
    "Read",
    "ReadMediaFile",
    "Glob",
    "Grep",
    "WebSearch",
    "FetchURL",
];

/// Register source built-ins once, mirroring the source module's import-time
/// side effect without reordering user contributions on later catalog builds.
pub fn register_builtin_agent_lifecycle_profiles() {
    static REGISTER: Once = Once::new();
    REGISTER.call_once(|| {
        register_agent_profile(profile("agent", Some("Default Kimi Code agent"), None, AGENT_TOOLS, "", None, None));
        let coder_role = format!("{TASK_AGENT_ROLE_PREFIX}\n\nYour final message is the entire handoff — the parent sees nothing else from your run. Make it technically complete: what you changed and why, the path of every file you touched, how you verified the change (tests or commands run, with results), and anything left undone or worth follow-up. A final message of only a sentence or two is treated as too brief and sent back to you for expansion, costing an extra turn.");
        register_agent_profile(profile("coder", Some("General software engineering agent — the only subagent type with file-editing tools; use it for any delegated task that must modify code."), Some("Use this agent for non-trivial software engineering work that may require reading files, editing code, running commands, and returning a compact but technically complete summary to the parent agent."), CODER_TOOLS, &coder_role, Some(summary_policy()), None));
        register_agent_profile(profile(
            "explore",
            Some("Fast codebase exploration with prompt-enforced read-only behavior."),
            Some("Fast agent specialized for exploring codebases. Use this when you need to quickly find files by patterns (e.g. \"src/**/*.yaml\"), search code for keywords (e.g. \"database connection\"), or answer questions about the codebase (e.g. \"how does the auth module work?\"). When calling this agent, specify the desired thoroughness level: \"quick\" for basic searches, \"medium\" for moderate exploration, or \"thorough\" for comprehensive analysis across multiple locations and naming conventions. Use this agent for any read-only exploration that will clearly require more than 3 search queries. Prefer launching multiple explore agents concurrently when investigating independent questions."),
            EXPLORE_TOOLS,
            EXPLORE_ROLE,
            Some(summary_policy()),
            Some(Arc::new(|context| {
                Box::pin(async move {
                    Ok(collect_git_context(
                        context.runner.0.as_ref(),
                        &context.cwd,
                        context.log.as_deref(),
                    )
                    .await)
                })
            })),
        ));
    });
}

fn profile(
    name: &str,
    description: Option<&str>,
    when_to_use: Option<&str>,
    tools: &[&str],
    role: &str,
    summary_policy: Option<AgentProfileSummaryPolicy>,
    prompt_prefix: Option<AgentPromptPrefix>,
) -> AgentProfile {
    let tools = tools
        .iter()
        .map(|tool| (*tool).to_owned())
        .collect::<Vec<_>>();
    let skill_active = skill_active_for(&tools);
    let role = role.to_owned();
    AgentProfile {
        name: name.into(),
        description: description.map(Into::into),
        when_to_use: when_to_use.map(Into::into),
        is_override: None,
        tools: Some(tools),
        disallowed_tools: None,
        subagents: None,
        system_prompt: Arc::new(move |context| {
            render_system_prompt(
                &role,
                context,
                crate::app::agent_profile_catalog::SkillActiveOptions { skill_active },
            )
        }),
        prompt_prefix,
        summary_policy,
    }
}

fn summary_policy() -> AgentProfileSummaryPolicy {
    AgentProfileSummaryPolicy {
        min_chars: 200,
        continuation_prompt: SUMMARY_CONTINUATION_PROMPT.into(),
        retries: 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::agent_profile_catalog::{
        AgentProfileCatalogContract, AgentProfileCatalogService,
    };
    #[test]
    fn registers_default_and_task_profiles_once() {
        register_builtin_agent_lifecycle_profiles();
        let catalog = AgentProfileCatalogService::new();
        assert_eq!(catalog.get_default().unwrap().name, "agent");
        assert_eq!(
            catalog
                .get("coder")
                .unwrap()
                .summary_policy
                .as_ref()
                .unwrap()
                .min_chars,
            200
        );
        assert_eq!(
            catalog.get("explore").unwrap().tools.as_ref().unwrap(),
            &EXPLORE_TOOLS
                .iter()
                .map(|tool| (*tool).to_owned())
                .collect::<Vec<_>>()
        );
    }
}
