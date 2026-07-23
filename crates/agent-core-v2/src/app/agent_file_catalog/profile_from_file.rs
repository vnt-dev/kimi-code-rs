//! Agent-file definition to executable profile conversion.
//!
//! Original: `packages/agent-core-v2/src/app/agentFileCatalog/agentProfileFromFile.ts`.

use std::sync::Arc;

use crate::app::agent_profile_catalog::{
    AgentProfile, AgentProfileContext, AgentSystemPrompt, SkillActiveOptions,
    render_prompt_template,
};

use super::types::{AgentFileDefinition, AgentFileSource};

// Original: agentProfileFromFile(). The base prompt remains a captured lazy
// function and is evaluated only if the file template references it.
pub fn agent_profile_from_file(
    definition: AgentFileDefinition,
    base_prompt: AgentSystemPrompt,
) -> AgentProfile {
    let skill_active = definition
        .tools
        .as_ref()
        .is_none_or(|tools| tools.iter().any(|tool| tool == "Skill"))
        && !definition
            .disallowed_tools
            .as_ref()
            .is_some_and(|tools| tools.iter().any(|tool| tool == "Skill"));
    let prompt = definition.prompt.clone();
    let system_prompt = Arc::new(move |context: &AgentProfileContext| {
        render_prompt_template(
            &prompt,
            context,
            SkillActiveOptions { skill_active },
            Some(base_prompt.as_ref()),
        )
    });

    AgentProfile {
        name: definition.name,
        description: Some(definition.description),
        when_to_use: definition.when_to_use,
        is_override: Some(definition.is_override || definition.source == AgentFileSource::Explicit),
        tools: definition.tools,
        disallowed_tools: definition.disallowed_tools,
        subagents: definition.subagents,
        system_prompt,
        prompt_prefix: None,
        summary_policy: None,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    fn definition(source: AgentFileSource) -> AgentFileDefinition {
        AgentFileDefinition {
            name: "review".into(),
            description: "Review code".into(),
            when_to_use: Some("before merging".into()),
            is_override: false,
            tools: None,
            disallowed_tools: None,
            subagents: Some(vec!["explore".into()]),
            prompt: "${base_prompt}${skills_section}".into(),
            path: "/agents/review.md".into(),
            source,
        }
    }

    #[test]
    fn explicit_files_override_and_profile_fields_pass_through() {
        let profile = agent_profile_from_file(
            definition(AgentFileSource::Explicit),
            Arc::new(|_| "BASE".into()),
        );
        assert_eq!(profile.name, "review");
        assert_eq!(profile.description.as_deref(), Some("Review code"));
        assert_eq!(profile.when_to_use.as_deref(), Some("before merging"));
        assert_eq!(profile.is_override, Some(true));
        assert_eq!(profile.subagents, Some(vec!["explore".into()]));
    }

    #[test]
    fn tool_policy_controls_skill_section_and_base_prompt_stays_lazy() {
        let calls = Arc::new(AtomicUsize::new(0));
        let calls_for_prompt = Arc::clone(&calls);
        let base: AgentSystemPrompt = Arc::new(move |_| {
            calls_for_prompt.fetch_add(1, Ordering::Relaxed);
            "BASE".into()
        });
        let mut denied_definition = definition(AgentFileSource::Project);
        denied_definition.prompt = "${skills_section}".into();
        denied_definition.disallowed_tools = Some(vec!["Skill".into()]);
        let profile = agent_profile_from_file(denied_definition, Arc::clone(&base));
        let context = AgentProfileContext {
            skills: Some("- review".into()),
            now: Some("now".into()),
            ..AgentProfileContext::default()
        };
        assert_eq!(profile.render_system_prompt(&context), "");
        assert_eq!(calls.load(Ordering::Relaxed), 0);

        let allowed =
            agent_profile_from_file(definition(AgentFileSource::Project), Arc::clone(&base));
        let rendered = allowed.render_system_prompt(&context);
        assert!(rendered.starts_with("BASE"));
        assert!(rendered.contains("# Skills"));
        assert_eq!(calls.load(Ordering::Relaxed), 1);
    }
}
