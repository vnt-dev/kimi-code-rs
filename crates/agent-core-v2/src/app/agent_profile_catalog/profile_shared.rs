//! Shared prompt helpers for built-in and contributed agent profiles.
//!
//! Original: `packages/agent-core-v2/src/app/agentProfileCatalog/profile-shared.ts`.

use std::{collections::HashMap, time::SystemTime};

use chrono::{DateTime, SecondsFormat, Utc};
use serde_json::Value;

use crate::_base::utils::render_prompt::render_prompt;

pub use super::contract::{AgentProfile, AgentProfileContext};

const SYSTEM_PROMPT_TEMPLATE: &str = include_str!("system.md");

pub const TASK_AGENT_ROLE_PREFIX: &str = "You are now running as a subagent. All the `user` messages are sent by the main agent. \
The main agent cannot see your context, it can only see your last message when you finish the task. \
You must treat the parent agent as your caller. Do not directly ask the end user questions. \
If something is unclear, explain the ambiguity in your final summary to the parent agent.";

const WINDOWS_NOTES: &str = "IMPORTANT: You are on Windows. The Bash tool runs through Git Bash, so use Unix shell syntax inside Bash commands — `/dev/null` not `NUL`, and forward slashes in paths. For file operations, always prefer the built-in tools (Read, Write, Edit, Glob, Grep) over Bash commands — they work reliably across all platforms.";

const ADDITIONAL_DIRS_SECTION_PROSE: &str = "The following directories have been added to the workspace. You can read, write, search, and glob files in these directories as part of your workspace scope.";

const SKILLS_SECTION_PROSE: &str = "Skills are reusable, composable capabilities that enhance your abilities. Each skill is either a self-contained directory with a `SKILL.md` file or a standalone `.md` file that contains instructions, examples, and/or reference material.\n\n\
Identify the skills relevant to your current task and read the skill file for its instructions; only read further skill details when needed, to conserve the context window.\n\n\
## Available skills\n\n\
Skills are grouped by scope (`Project`, `User`, `Extra`, `Built-in`) so you can tell where each came from. When the user refers to \"the skill in this project\" or \"the user-scope skill\", use the scope heading to disambiguate. When multiple scopes define a skill with the same name, the more specific scope takes precedence: **Project overrides User overrides Extra overrides Built-in**.";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SkillActiveOptions {
    pub skill_active: bool,
}

// Original: skillActiveFor().
pub fn skill_active_for(tools: &[String]) -> bool {
    tools.iter().any(|tool| tool == "Skill")
}

// Original: subagentAllowlistFor(). Passing the default profile's allowlist is
// the Rust adaptation of the source's narrow catalog argument.
pub fn subagent_allowlist_for<'a>(
    default_subagents: Option<&'a [String]>,
    caller_profile_name: Option<&str>,
    caller_subagents: Option<&'a [String]>,
) -> Option<&'a [String]> {
    if caller_profile_name.is_none() {
        default_subagents
    } else {
        caller_subagents
    }
}

// Original: subagentTypeNotAllowedMessage().
pub fn subagent_type_not_allowed_message(name: &str, allowlist: &[String]) -> String {
    let allowed = if allowlist.is_empty() {
        "none".into()
    } else {
        allowlist.join(", ")
    };
    format!(
        "Subagent type \"{name}\" is not allowed for this agent. Allowed subagent types: {allowed}."
    )
}

// No source equivalent: profiles can pin a `model` (agent-file frontmatter).
// A pinned model wins over the caller's current model when binding a
// subagent; profiles without one keep the inherit-from-caller behavior.
pub fn subagent_model_alias(profile: Option<&AgentProfile>, caller_model: String) -> String {
    profile
        .and_then(|profile| profile.model.clone())
        .unwrap_or(caller_model)
}

// Original: systemPromptVars().
pub fn system_prompt_vars(
    context: &AgentProfileContext,
    options: SkillActiveOptions,
) -> HashMap<String, Value> {
    let shell_name = context.shell_name.as_deref().unwrap_or_default();
    let shell_path = context.shell_path.as_deref().unwrap_or_default();
    let skill_active = context.skill_active.unwrap_or(options.skill_active);
    let skills = if skill_active {
        context.skills.as_deref().unwrap_or_default()
    } else {
        ""
    };
    let additional_dirs_info = context.additional_dirs_info.as_deref().unwrap_or_default();
    let windows_notes = if context.os_kind.as_deref() == Some("Windows") {
        format!("\n\n{WINDOWS_NOTES}\n\n")
    } else {
        String::new()
    };
    let shell = if shell_name.is_empty() {
        String::new()
    } else {
        format!("{shell_name} (`{shell_path}`)")
    };
    let additional_dirs_section = if additional_dirs_info.is_empty() {
        String::new()
    } else {
        format!(
            "\n\n## Additional Directories\n\n{ADDITIONAL_DIRS_SECTION_PROSE}\n\n{additional_dirs_info}\n\n"
        )
    };
    let skills_section = if skills.is_empty() {
        String::new()
    } else {
        format!("\n\n# Skills\n\n{SKILLS_SECTION_PROSE}\n\n{skills}\n\n")
    };

    string_variables([
        ("role_additional", "".into()),
        ("os", context.os_kind.clone().unwrap_or_default()),
        ("windows_notes", windows_notes),
        ("shell", shell),
        ("now", context.now.clone().unwrap_or_else(current_iso_time)),
        ("cwd", context.cwd.clone().unwrap_or_default()),
        (
            "cwd_listing",
            context.cwd_listing.clone().unwrap_or_default(),
        ),
        ("agents_md", context.agents_md.clone().unwrap_or_default()),
        ("additional_dirs_info", additional_dirs_info.into()),
        ("additional_dirs_section", additional_dirs_section),
        ("skills", skills.into()),
        ("skills_section", skills_section),
    ])
}

// Original: renderPromptTemplate(). `base_prompt` remains lazy and is invoked
// only if the user-owned template references `${base_prompt}`.
pub fn render_prompt_template(
    template: &str,
    context: &AgentProfileContext,
    options: SkillActiveOptions,
    base_prompt: Option<&dyn Fn(&AgentProfileContext) -> String>,
) -> String {
    let mut variables = system_prompt_vars(context, options);
    if template.contains("${base_prompt}")
        && let Some(base_prompt) = base_prompt
    {
        variables.insert("base_prompt".into(), Value::String(base_prompt(context)));
    }
    render_prompt(template, &variables)
}

// Original: renderSystemPrompt().
pub fn render_system_prompt(
    role_additional: &str,
    context: &AgentProfileContext,
    options: SkillActiveOptions,
) -> String {
    let mut variables = system_prompt_vars(context, options);
    variables.insert(
        "role_additional".into(),
        Value::String(role_additional.into()),
    );
    render_prompt(SYSTEM_PROMPT_TEMPLATE, &variables)
}

fn string_variables<const N: usize>(values: [(&str, String); N]) -> HashMap<String, Value> {
    values
        .into_iter()
        .map(|(key, value)| (key.into(), Value::String(value)))
        .collect()
}

fn current_iso_time() -> String {
    let now: DateTime<Utc> = SystemTime::now().into();
    now.to_rfc3339_opts(SecondsFormat::Millis, true)
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;
    use crate::_base::utils::hash::sha256_hex;

    #[test]
    fn system_template_is_byte_identical_to_the_source_asset() {
        let digest = sha256_hex(SYSTEM_PROMPT_TEMPLATE.as_bytes());
        assert_eq!(
            digest,
            "ae0a53980f99df339518c3a707b2f0b4af56b1fa3879749f96e245de3c83fd8c"
        );
    }

    #[test]
    fn variables_compose_conditional_sections_and_context_override() {
        let context = AgentProfileContext {
            os_kind: Some("Windows".into()),
            shell_name: Some("bash".into()),
            shell_path: Some("C:/Git/bin/bash.exe".into()),
            additional_dirs_info: Some("- /extra".into()),
            skills: Some("- review".into()),
            skill_active: Some(false),
            now: Some("2026-01-02T03:04:05.006Z".into()),
            ..AgentProfileContext::default()
        };
        let variables = system_prompt_vars(&context, SkillActiveOptions { skill_active: true });
        assert_eq!(variables["shell"], "bash (`C:/Git/bin/bash.exe`)");
        assert!(
            variables["windows_notes"]
                .as_str()
                .unwrap()
                .contains("Git Bash")
        );
        assert!(
            variables["additional_dirs_section"]
                .as_str()
                .unwrap()
                .contains("- /extra")
        );
        assert_eq!(variables["skills"], "");
        assert_eq!(variables["skills_section"], "");
    }

    #[test]
    fn user_template_resolves_base_prompt_lazily_in_one_pass() {
        let calls = AtomicUsize::new(0);
        let base = |_: &AgentProfileContext| {
            calls.fetch_add(1, Ordering::Relaxed);
            "base ${cwd}".into()
        };
        let context = AgentProfileContext {
            cwd: Some("/repo".into()),
            now: Some("now".into()),
            ..AgentProfileContext::default()
        };
        let options = SkillActiveOptions {
            skill_active: false,
        };

        assert_eq!(
            render_prompt_template("cwd=${cwd}", &context, options, Some(&base)),
            "cwd=/repo"
        );
        assert_eq!(calls.load(Ordering::Relaxed), 0);
        assert_eq!(
            render_prompt_template("${base_prompt}", &context, options, Some(&base)),
            "base ${cwd}"
        );
        assert_eq!(calls.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn tool_and_subagent_helpers_preserve_allowlist_messages() {
        assert!(skill_active_for(&["Read".into(), "Skill".into()]));
        assert!(!skill_active_for(&["read".into()]));
        let defaults = vec!["explore".into()];
        let caller = vec!["plan".into()];
        assert_eq!(
            subagent_allowlist_for(Some(&defaults), None, Some(&caller)),
            Some(defaults.as_slice())
        );
        assert_eq!(
            subagent_allowlist_for(Some(&defaults), Some("custom"), Some(&caller)),
            Some(caller.as_slice())
        );
        assert_eq!(
            subagent_type_not_allowed_message("plan", &[]),
            "Subagent type \"plan\" is not allowed for this agent. Allowed subagent types: none."
        );
    }

    #[test]
    fn subagent_model_alias_prefers_profile_pinned_model() {
        use std::sync::Arc;

        let profile = |model: Option<&str>| AgentProfile {
            name: "worker".into(),
            description: None,
            when_to_use: None,
            is_override: None,
            tools: None,
            disallowed_tools: None,
            subagents: None,
            model: model.map(Into::into),
            system_prompt: Arc::new(|_| String::new()),
            prompt_prefix: None,
            summary_policy: None,
        };

        assert_eq!(
            subagent_model_alias(Some(&profile(Some("pinned"))), "caller".into()),
            "pinned"
        );
        assert_eq!(
            subagent_model_alias(Some(&profile(None)), "caller".into()),
            "caller"
        );
        assert_eq!(subagent_model_alias(None, "caller".into()), "caller");
    }
}
