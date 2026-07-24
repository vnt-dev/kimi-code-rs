//! Pure input expansion and XML rendering for the AgentSwarm tool.
//!
//! Original: `agent/swarm/tools/agent-swarm.ts`, `createAgentSwarmSpecs()`
//! and rendering helpers.

use std::collections::HashMap;
use std::future::Future;

pub const DEFAULT_SUBAGENT_TYPE: &str = "coder";
pub const PROMPT_TEMPLATE_PLACEHOLDER: &str = "{{item}}";
pub const MAX_AGENT_SWARM_SUBAGENTS: usize = 128;

#[derive(Clone, Debug, Default)]
pub struct AgentSwarmInput {
    pub prompt_template: Option<String>,
    pub items: Vec<String>,
    /// Ordered entries preserve JavaScript `Object.entries()` iteration order.
    pub resume_agent_ids: Vec<(String, String)>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AgentSwarmSpec {
    Spawn {
        index: usize,
        item: String,
        prompt: String,
    },
    Resume {
        index: usize,
        agent_id: String,
        item: Option<String>,
        prompt: String,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AgentSwarmStatus {
    Completed,
    Failed,
    Aborted,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AgentSwarmState {
    Started,
    NotStarted,
}

#[derive(Clone, Debug)]
pub struct AgentSwarmResult {
    pub spec: AgentSwarmSpec,
    pub agent_id: Option<String>,
    pub status: AgentSwarmStatus,
    pub state: Option<AgentSwarmState>,
    pub result: Option<String>,
    pub error: Option<String>,
}

/// Expand resume entries first, then item-based spawn entries.
///
/// The item lookup remains async and is awaited sequentially to match the
/// original `for...of` loop and its externally observable lookup ordering.
pub async fn create_agent_swarm_specs<F, Fut>(
    input: &AgentSwarmInput,
    get_resume_item: F,
) -> Result<Vec<AgentSwarmSpec>, String>
where
    F: Fn(&str) -> Fut,
    Fut: Future<Output = Option<String>>,
{
    let resume_entries = input
        .resume_agent_ids
        .iter()
        .map(|(agent_id, prompt)| (agent_id.trim().to_owned(), prompt.trim().to_owned()))
        .collect::<Vec<_>>();
    let items = input
        .items
        .iter()
        .map(|item| item.trim().to_owned())
        .collect::<Vec<_>>();

    if !has_minimum_agent_swarm_inputs(items.len(), resume_entries.len()) {
        return Err(
            "AgentSwarm requires at least 2 items unless resume_agent_ids is provided.".into(),
        );
    }

    if resume_entries.len() + items.len() > MAX_AGENT_SWARM_SUBAGENTS {
        return Err(format!(
            "AgentSwarm supports at most {MAX_AGENT_SWARM_SUBAGENTS} subagents."
        ));
    }

    let prompt_template = input
        .prompt_template
        .as_deref()
        .and_then(normalize_optional_string);
    if !items.is_empty() && prompt_template.is_none() {
        return Err("prompt_template is required when items are provided.".into());
    }
    if let Some(prompt_template) = prompt_template
        && !prompt_template.contains(PROMPT_TEMPLATE_PLACEHOLDER)
    {
        return Err(format!(
            "prompt_template must include the {PROMPT_TEMPLATE_PLACEHOLDER} placeholder."
        ));
    }

    let mut specs = Vec::with_capacity(resume_entries.len() + items.len());
    for (agent_id, prompt) in resume_entries {
        let item = get_resume_item(&agent_id).await;
        specs.push(AgentSwarmSpec::Resume {
            index: specs.len() + 1,
            agent_id,
            item,
            prompt,
        });
    }

    if let Some(prompt_template) = prompt_template {
        let mut seen_prompts = HashMap::new();
        for (item_index, item) in items.into_iter().enumerate() {
            let prompt = prompt_template.replace(PROMPT_TEMPLATE_PLACEHOLDER, &item);
            if let Some(previous_index) = seen_prompts.insert(prompt.clone(), item_index + 1) {
                return Err(format!(
                    "Duplicate subagent prompts from items {previous_index} and {}. AgentSwarm requires distinct subagents.",
                    item_index + 1
                ));
            }
            specs.push(AgentSwarmSpec::Spawn {
                index: specs.len() + 1,
                item,
                prompt,
            });
        }
    }

    Ok(specs)
}

pub fn has_minimum_agent_swarm_inputs(item_count: usize, resume_count: usize) -> bool {
    resume_count > 0 || item_count >= 2
}

pub fn child_description(swarm_description: &str, index: usize, profile_name: &str) -> String {
    format!("{swarm_description} #{index} ({profile_name})")
}

pub fn render_swarm_results(results: &[AgentSwarmResult]) -> String {
    let completed = results
        .iter()
        .filter(|result| result.status == AgentSwarmStatus::Completed)
        .count();
    let failed = results
        .iter()
        .filter(|result| result.status == AgentSwarmStatus::Failed)
        .count();
    let aborted = results
        .iter()
        .filter(|result| result.status == AgentSwarmStatus::Aborted)
        .count();

    let mut lines = vec![
        "<agent_swarm_result>".to_owned(),
        format!(
            "<summary>{}</summary>",
            render_swarm_summary(completed, failed, aborted)
        ),
    ];

    let should_render_resume_hint = results
        .iter()
        .any(|result| result.status != AgentSwarmStatus::Completed)
        && results.iter().any(|result| result.agent_id.is_some());
    if should_render_resume_hint {
        lines.push("<resume_hint>Call AgentSwarm with resume_agent_ids using the agent_id values in this result to continue unfinished work.</resume_hint>".to_owned());
    }

    for result in results {
        let agent_id = result
            .agent_id
            .as_ref()
            .map_or_else(String::new, |agent_id| format!(" agent_id=\"{agent_id}\""));
        let mode = matches!(result.spec, AgentSwarmSpec::Resume { .. })
            .then_some(" mode=\"resume\"")
            .unwrap_or_default();
        let item = spec_item(&result.spec).map_or_else(String::new, |item| {
            format!(" item=\"{}\"", escape_xml_attribute(item))
        });
        let state = result.state.map_or_else(String::new, |state| {
            format!(" state=\"{}\"", agent_swarm_state_name(state))
        });
        let status = agent_swarm_status_name(result.status);
        let body = if result.status == AgentSwarmStatus::Completed {
            result.result.as_deref().unwrap_or_default()
        } else {
            result.error.as_deref().unwrap_or("unknown error")
        };
        lines.push(format!(
            "<subagent{mode}{agent_id}{item}{state} outcome=\"{status}\">{body}</subagent>"
        ));
    }

    lines.push("</agent_swarm_result>".to_owned());
    lines.join("\n")
}

pub fn render_swarm_summary(completed: usize, failed: usize, aborted: usize) -> String {
    let mut parts = Vec::with_capacity(3);
    if completed > 0 {
        parts.push(format!("completed: {completed}"));
    }
    if failed > 0 {
        parts.push(format!("failed: {failed}"));
    }
    if aborted > 0 {
        parts.push(format!("aborted: {aborted}"));
    }
    parts.join(", ")
}

pub fn escape_xml_attribute(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn normalize_optional_string(value: &str) -> Option<&str> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then_some(trimmed)
}

fn spec_item(spec: &AgentSwarmSpec) -> Option<&str> {
    match spec {
        AgentSwarmSpec::Spawn { item, .. } => Some(item),
        AgentSwarmSpec::Resume { item, .. } => item.as_deref(),
    }
}

fn agent_swarm_status_name(status: AgentSwarmStatus) -> &'static str {
    match status {
        AgentSwarmStatus::Completed => "completed",
        AgentSwarmStatus::Failed => "failed",
        AgentSwarmStatus::Aborted => "aborted",
    }
}

fn agent_swarm_state_name(state: AgentSwarmState) -> &'static str {
    match state {
        AgentSwarmState::Started => "started",
        AgentSwarmState::NotStarted => "not_started",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn expands_resumes_before_spawns_and_preserves_lookup_order() {
        let input = AgentSwarmInput {
            prompt_template: Some("Investigate {{item}}; {{item}} again".into()),
            items: vec!["new work".into(), "other work".into()],
            resume_agent_ids: vec![
                (" agent-one ".into(), " resume first ".into()),
                ("agent-two".into(), "resume second".into()),
            ],
        };
        let lookups = std::sync::Mutex::new(Vec::new());
        let specs = create_agent_swarm_specs(&input, |agent_id| {
            lookups.lock().unwrap().push(agent_id.to_owned());
            std::future::ready(Some(format!("item for {agent_id}")))
        })
        .await
        .unwrap();

        assert_eq!(*lookups.lock().unwrap(), ["agent-one", "agent-two"]);
        assert_eq!(
            specs,
            vec![
                AgentSwarmSpec::Resume {
                    index: 1,
                    agent_id: "agent-one".into(),
                    item: Some("item for agent-one".into()),
                    prompt: "resume first".into(),
                },
                AgentSwarmSpec::Resume {
                    index: 2,
                    agent_id: "agent-two".into(),
                    item: Some("item for agent-two".into()),
                    prompt: "resume second".into(),
                },
                AgentSwarmSpec::Spawn {
                    index: 3,
                    item: "new work".into(),
                    prompt: "Investigate new work; new work again".into(),
                },
                AgentSwarmSpec::Spawn {
                    index: 4,
                    item: "other work".into(),
                    prompt: "Investigate other work; other work again".into(),
                },
            ]
        );
    }

    #[tokio::test]
    async fn rejects_invalid_item_input_and_duplicate_prompts() {
        let too_few =
            create_agent_swarm_specs(&AgentSwarmInput::default(), |_| std::future::ready(None))
                .await
                .unwrap_err();
        assert_eq!(
            too_few,
            "AgentSwarm requires at least 2 items unless resume_agent_ids is provided."
        );

        let duplicate = create_agent_swarm_specs(
            &AgentSwarmInput {
                prompt_template: Some("same".into()),
                items: vec!["a".into(), "b".into()],
                ..Default::default()
            },
            |_| std::future::ready(None),
        )
        .await
        .unwrap_err();
        assert_eq!(
            duplicate,
            "prompt_template must include the {{item}} placeholder."
        );

        let duplicate = create_agent_swarm_specs(
            &AgentSwarmInput {
                prompt_template: Some("{{item}}".into()),
                items: vec![" same ".into(), "same".into()],
                ..Default::default()
            },
            |_| std::future::ready(None),
        )
        .await
        .unwrap_err();
        assert_eq!(
            duplicate,
            "Duplicate subagent prompts from items 1 and 2. AgentSwarm requires distinct subagents."
        );
    }

    #[test]
    fn renders_result_xml_with_resume_hint_and_escaped_item() {
        let output = render_swarm_results(&[
            AgentSwarmResult {
                spec: AgentSwarmSpec::Spawn {
                    index: 1,
                    item: "a & <b>\"".into(),
                    prompt: "unused".into(),
                },
                agent_id: Some("agent-a".into()),
                status: AgentSwarmStatus::Completed,
                state: Some(AgentSwarmState::Started),
                result: Some("done".into()),
                error: None,
            },
            AgentSwarmResult {
                spec: AgentSwarmSpec::Resume {
                    index: 2,
                    agent_id: "agent-b".into(),
                    item: None,
                    prompt: "unused".into(),
                },
                agent_id: Some("agent-b".into()),
                status: AgentSwarmStatus::Failed,
                state: Some(AgentSwarmState::NotStarted),
                result: None,
                error: None,
            },
        ]);

        assert_eq!(
            output,
            "<agent_swarm_result>\n<summary>completed: 1, failed: 1</summary>\n<resume_hint>Call AgentSwarm with resume_agent_ids using the agent_id values in this result to continue unfinished work.</resume_hint>\n<subagent agent_id=\"agent-a\" item=\"a &amp; &lt;b&gt;&quot;\" state=\"started\" outcome=\"completed\">done</subagent>\n<subagent mode=\"resume\" agent_id=\"agent-b\" state=\"not_started\" outcome=\"failed\">unknown error</subagent>\n</agent_swarm_result>"
        );
    }
}
