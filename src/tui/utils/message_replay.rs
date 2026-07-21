use std::{
    collections::{HashMap, HashSet},
    sync::OnceLock,
};

use regex::Regex;
use serde_json::{Map, Value};

use crate::{
    sdk::types::{
        BackgroundTaskInfo, BackgroundTaskKind, BackgroundTaskStatus, ContentPart, ContextMessage,
        PromptOrigin, PromptOriginKind, ToolCall,
    },
    tui::types::{
        BackgroundAgentMetadata, SkillActivationTrigger, ToolCallBlockData, TranscriptEntry,
        TranscriptEntryKind, TranscriptRenderMode,
    },
};

use super::{
    media_url::{MediaUrlKind, media_url_part_to_text},
    transcript_id::next_transcript_id,
};

pub const REPLAY_TURN_LIMIT: usize = 10;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ReplayAssistantContent {
    pub thinking: Vec<String>,
    pub text: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ReplayRenderContext {
    pub turn_index: usize,
    pub step_index: usize,
    pub current_turn_id: Option<String>,
    pub assistant: ReplayAssistantContent,
    pub tool_calls: HashMap<String, ToolCallBlockData>,
    pub completed_tool_call_ids: HashSet<String>,
    pub skill_activation_ids: HashSet<String>,
    pub plugin_command_activation_ids: HashSet<String>,
    pub suppress_next_plan_mode_off_notice: bool,
}

pub fn create_replay_render_context() -> ReplayRenderContext {
    ReplayRenderContext {
        turn_index: 0,
        step_index: 0,
        current_turn_id: None,
        assistant: ReplayAssistantContent::default(),
        tool_calls: HashMap::new(),
        completed_tool_call_ids: HashSet::new(),
        skill_activation_ids: HashSet::new(),
        plugin_command_activation_ids: HashSet::new(),
        suppress_next_plan_mode_off_notice: false,
    }
}

pub fn is_terminal_background_task(info: &BackgroundTaskInfo) -> bool {
    info.status != BackgroundTaskStatus::Running
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ActiveBackgroundTaskCounts {
    pub bash_tasks: usize,
    pub agent_tasks: usize,
}

pub fn count_active_background_tasks(
    tasks: &HashMap<String, BackgroundTaskInfo>,
) -> ActiveBackgroundTaskCounts {
    let mut counts = ActiveBackgroundTaskCounts::default();
    for info in tasks.values() {
        if is_terminal_background_task(info) {
            continue;
        }
        if matches!(&info.kind, BackgroundTaskKind::Agent { .. }) {
            counts.agent_tasks += 1;
        } else {
            counts.bash_tasks += 1;
        }
    }
    counts
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReplayBackgroundProjection {
    pub background_agent_metadata: HashMap<String, BackgroundAgentMetadata>,
}

pub fn replay_background_projection(
    background: &[BackgroundTaskInfo],
) -> ReplayBackgroundProjection {
    let mut background_agent_metadata = HashMap::new();
    for info in background {
        let BackgroundTaskKind::Agent { agent_id, .. } = &info.kind else {
            continue;
        };
        if is_terminal_background_task(info) {
            continue;
        }
        let agent_id = agent_id.as_deref().unwrap_or(&info.task_id).to_owned();
        background_agent_metadata.insert(
            agent_id.clone(),
            BackgroundAgentMetadata {
                agent_id,
                parent_tool_call_id: info.task_id.clone(),
                agent_name: None,
                description: Some(info.description.clone()),
            },
        );
    }
    ReplayBackgroundProjection {
        background_agent_metadata,
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReplayEntryExtras {
    pub detail: Option<String>,
    pub bullet: Option<String>,
}

pub fn replay_entry(
    context: &ReplayRenderContext,
    kind: TranscriptEntryKind,
    content: impl Into<String>,
    render_mode: TranscriptRenderMode,
    extras: ReplayEntryExtras,
) -> TranscriptEntry {
    TranscriptEntry {
        id: next_transcript_id(),
        kind,
        turn_id: context.current_turn_id.clone(),
        render_mode,
        content: content.into(),
        model_text: None,
        color: None,
        detail: extras.detail,
        bullet: extras.bullet,
        compaction_data: None,
        cron_data: None,
        background_agent_status: None,
        image_attachment_ids: None,
        skill_activation_id: None,
        skill_name: None,
        skill_args: None,
        skill_trigger: None,
        plugin_command_data: None,
    }
}

pub fn collect_replay_message_content(
    target: &mut ReplayAssistantContent,
    content: &[ContentPart],
) {
    for part in content {
        match part {
            ContentPart::Think { think } => target.thinking.push(think.clone()),
            ContentPart::Text { text } => target.text.push(text.clone()),
            ContentPart::ImageUrl { .. }
            | ContentPart::AudioUrl { .. }
            | ContentPart::VideoUrl { .. } => {}
        }
    }
}

pub fn tool_call_from_replay_message(
    raw_tool_call: &ToolCall,
    context: &ReplayRenderContext,
) -> Option<ToolCallBlockData> {
    if raw_tool_call.id.is_empty() || raw_tool_call.name.is_empty() {
        return None;
    }
    Some(ToolCallBlockData {
        id: raw_tool_call.id.clone(),
        name: raw_tool_call.name.clone(),
        args: parse_replay_tool_arguments(raw_tool_call.arguments.as_deref()),
        description: None,
        streaming_arguments: None,
        streaming_started_at_ms: None,
        subagent: None,
        step: Some(context.step_index),
        turn_id: context.current_turn_id.clone(),
        truncated: None,
    })
}

pub fn tool_result_output(content: &[ContentPart]) -> Result<String, serde_json::Error> {
    if content
        .iter()
        .any(|part| !matches!(part, ContentPart::Text { .. }))
    {
        serde_json::to_string(content)
    } else {
        Ok(content_parts_to_text(content))
    }
}

pub fn content_parts_to_text(content: &[ContentPart]) -> String {
    content
        .iter()
        .map(content_part_to_text)
        .collect::<Vec<_>>()
        .join("")
}

pub fn background_origin(message: &ContextMessage) -> Option<&PromptOrigin> {
    message
        .origin
        .as_ref()
        .filter(|origin| origin.kind == PromptOriginKind::BackgroundTask)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillActivationProjection {
    pub activation_id: String,
    pub skill_name: String,
    pub skill_args: Option<String>,
    pub trigger: SkillActivationTrigger,
}

pub fn skill_activation_from_origin(
    origin: Option<&PromptOrigin>,
) -> Option<SkillActivationProjection> {
    let origin = origin.filter(|origin| origin.kind == PromptOriginKind::SkillActivation)?;
    Some(SkillActivationProjection {
        activation_id: string_field(&origin.fields, "activationId")?,
        skill_name: string_field(&origin.fields, "skillName")?,
        skill_args: optional_string_field(&origin.fields, "skillArgs"),
        trigger: match string_field(&origin.fields, "trigger")?.as_str() {
            "user-slash" => SkillActivationTrigger::UserSlash,
            "model-tool" => SkillActivationTrigger::ModelTool,
            "nested-skill" => SkillActivationTrigger::NestedSkill,
            _ => return None,
        },
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginCommandProjection {
    pub activation_id: String,
    pub plugin_id: String,
    pub command_name: String,
    pub command_args: Option<String>,
}

pub fn plugin_command_from_origin(
    origin: Option<&PromptOrigin>,
) -> Option<PluginCommandProjection> {
    let origin = origin.filter(|origin| origin.kind == PromptOriginKind::PluginCommand)?;
    if string_field(&origin.fields, "trigger")?.as_str() != "user-slash" {
        return None;
    }
    Some(PluginCommandProjection {
        activation_id: string_field(&origin.fields, "activationId")?,
        plugin_id: string_field(&origin.fields, "pluginId")?,
        command_name: string_field(&origin.fields, "commandName")?,
        command_args: optional_string_field(&origin.fields, "commandArgs"),
    })
}

/// Original:
///   apps/kimi-code/src/tui/utils/message-replay.ts
///   formatHookResultMessageForTranscript()
pub fn format_hook_result_message_for_transcript(
    text: &str,
    fallback_event: &str,
    blocked: bool,
) -> String {
    let Some(pattern) = hook_result_pattern() else {
        return format_hook_result_block(fallback_event, text, blocked);
    };
    let mut results = Vec::new();
    let mut last_index = 0;
    for captures in pattern.captures_iter(text) {
        let Some(complete) = captures.get(0) else {
            return format_hook_result_block(fallback_event, text, blocked);
        };
        if !text[last_index..complete.start()].trim().is_empty() {
            return format_hook_result_block(fallback_event, text, blocked);
        }
        let (Some(event), Some(body)) = (captures.get(1), captures.get(2)) else {
            return format_hook_result_block(fallback_event, text, blocked);
        };
        results.push((event.as_str(), body.as_str()));
        last_index = complete.end();
    }
    if results.is_empty() || !text[last_index..].trim().is_empty() {
        return format_hook_result_block(fallback_event, text, blocked);
    }
    results
        .into_iter()
        .map(|(event, body)| format_hook_result_block(event, body, blocked))
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn parse_replay_tool_arguments(value: Option<&str>) -> Map<String, Value> {
    value
        .filter(|value| !value.is_empty())
        .and_then(|value| serde_json::from_str::<Value>(value).ok())
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default()
}

fn content_part_to_text(part: &ContentPart) -> String {
    match part {
        ContentPart::Text { text } => text.clone(),
        ContentPart::Think { think } => think.clone(),
        ContentPart::ImageUrl { image_url } => {
            media_url_part_to_text(MediaUrlKind::Image, &image_url.url)
        }
        ContentPart::VideoUrl { video_url } => {
            media_url_part_to_text(MediaUrlKind::Video, &video_url.url)
        }
        ContentPart::AudioUrl { audio_url } => {
            media_url_part_to_text(MediaUrlKind::Audio, &audio_url.url)
        }
    }
}

fn hook_result_pattern() -> Option<&'static Regex> {
    static PATTERN: OnceLock<Option<Regex>> = OnceLock::new();
    PATTERN
        .get_or_init(|| {
            Regex::new(r#"(?s)<hook_result\s+hook_event="([^"]+)">\n?(.*?)\n?</hook_result>"#).ok()
        })
        .as_ref()
}

fn format_hook_result_block(event: &str, body: &str, blocked: bool) -> String {
    let blocked = if blocked { " blocked" } else { "" };
    let body = if body.trim().is_empty() {
        "(empty)"
    } else {
        body.trim()
    };
    format!("*{event} hook{blocked}*\n\n{body}")
}

fn string_field(fields: &Map<String, Value>, name: &str) -> Option<String> {
    fields.get(name)?.as_str().map(str::to_owned)
}

fn optional_string_field(fields: &Map<String, Value>, name: &str) -> Option<String> {
    fields.get(name).and_then(Value::as_str).map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sdk::types::{ContextMessageRole, MediaUrl};

    fn task(status: BackgroundTaskStatus, kind: BackgroundTaskKind) -> BackgroundTaskInfo {
        BackgroundTaskInfo {
            task_id: "task-1".to_owned(),
            description: "background work".to_owned(),
            status,
            detached: Some(true),
            started_at: 1.0,
            ended_at: None,
            stop_reason: None,
            terminal_notification_suppressed: None,
            timeout_ms: None,
            kind,
        }
    }

    #[test]
    fn initializes_context_and_builds_replay_entries() {
        let mut context = create_replay_render_context();
        context.current_turn_id = Some("turn-1".to_owned());
        let entry = replay_entry(
            &context,
            TranscriptEntryKind::Assistant,
            "hello",
            TranscriptRenderMode::Markdown,
            ReplayEntryExtras::default(),
        );

        assert_eq!(context.turn_index, 0);
        assert_eq!(entry.turn_id.as_deref(), Some("turn-1"));
        assert!(entry.id.starts_with("entry-"));
    }

    #[test]
    fn counts_only_running_background_tasks_and_projects_agents() {
        let running_agent = task(
            BackgroundTaskStatus::Running,
            BackgroundTaskKind::Agent {
                agent_id: Some("agent-child".to_owned()),
                subagent_type: None,
            },
        );
        let tasks = HashMap::from([
            ("agent".to_owned(), running_agent.clone()),
            (
                "question".to_owned(),
                task(
                    BackgroundTaskStatus::Running,
                    BackgroundTaskKind::Question {
                        question_count: 1,
                        tool_call_id: None,
                    },
                ),
            ),
            (
                "done".to_owned(),
                task(
                    BackgroundTaskStatus::Completed,
                    BackgroundTaskKind::Process {
                        command: "echo".to_owned(),
                        pid: 1,
                        exit_code: Some(0),
                    },
                ),
            ),
        ]);

        assert_eq!(
            count_active_background_tasks(&tasks),
            ActiveBackgroundTaskCounts {
                bash_tasks: 1,
                agent_tasks: 1
            }
        );
        let projection = replay_background_projection(&[running_agent]);
        assert!(
            projection
                .background_agent_metadata
                .contains_key("agent-child")
        );
    }

    #[test]
    fn collects_text_and_thinking_but_not_media() {
        let content = [
            ContentPart::Think {
                think: "reason".to_owned(),
            },
            ContentPart::Text {
                text: "answer".to_owned(),
            },
            ContentPart::ImageUrl {
                image_url: MediaUrl {
                    url: "data:image/png;base64,AQ==".to_owned(),
                    id: None,
                },
            },
        ];
        let mut target = ReplayAssistantContent::default();
        collect_replay_message_content(&mut target, &content);

        assert_eq!(target.thinking, ["reason"]);
        assert_eq!(target.text, ["answer"]);
        assert!(content_parts_to_text(&content).contains("[image image/png, 1 B]"));
    }

    #[test]
    fn parses_replay_tool_calls_and_rejects_missing_identity() {
        let mut context = create_replay_render_context();
        context.step_index = 3;
        context.current_turn_id = Some("turn-1".to_owned());
        let call = ToolCall {
            tool_type: "function".to_owned(),
            id: "call-1".to_owned(),
            name: "Read".to_owned(),
            arguments: Some(r#"{"path":"a.rs"}"#.to_owned()),
        };
        let projected = tool_call_from_replay_message(&call, &context);
        assert!(matches!(
            projected,
            Some(ToolCallBlockData { step: Some(3), args, .. })
                if args.get("path") == Some(&Value::String("a.rs".to_owned()))
        ));
        let mut invalid = call;
        invalid.id.clear();
        assert_eq!(tool_call_from_replay_message(&invalid, &context), None);
    }

    #[test]
    fn serializes_mixed_tool_results_and_flattens_text_only_results() {
        let text = [ContentPart::Text {
            text: "hello".to_owned(),
        }];
        assert!(tool_result_output(&text).is_ok_and(|value| value == "hello"));
        let mixed = [
            ContentPart::Text {
                text: "hello".to_owned(),
            },
            ContentPart::Think {
                think: "reason".to_owned(),
            },
        ];
        assert!(tool_result_output(&mixed).is_ok_and(|value| value.starts_with('[')));
    }

    #[test]
    fn projects_typed_skill_and_plugin_origins() {
        let skill = PromptOrigin {
            kind: PromptOriginKind::SkillActivation,
            fields: Map::from_iter([
                ("activationId".to_owned(), Value::String("a1".to_owned())),
                ("skillName".to_owned(), Value::String("review".to_owned())),
                ("trigger".to_owned(), Value::String("user-slash".to_owned())),
            ]),
        };
        assert!(matches!(
            skill_activation_from_origin(Some(&skill)),
            Some(SkillActivationProjection {
                trigger: SkillActivationTrigger::UserSlash,
                ..
            })
        ));

        let plugin = PromptOrigin {
            kind: PromptOriginKind::PluginCommand,
            fields: Map::from_iter([
                ("activationId".to_owned(), Value::String("p1".to_owned())),
                ("pluginId".to_owned(), Value::String("demo".to_owned())),
                ("commandName".to_owned(), Value::String("run".to_owned())),
                ("trigger".to_owned(), Value::String("user-slash".to_owned())),
            ]),
        };
        assert!(plugin_command_from_origin(Some(&plugin)).is_some());
    }

    #[test]
    fn formats_wrapped_hook_results_or_falls_back_for_mixed_text() {
        let wrapped = "<hook_result hook_event=\"before\">\none\n</hook_result>\n<hook_result hook_event=\"after\"></hook_result>";
        assert_eq!(
            format_hook_result_message_for_transcript(wrapped, "fallback", true),
            "*before hook blocked*\n\none\n\n*after hook blocked*\n\n(empty)"
        );
        assert_eq!(
            format_hook_result_message_for_transcript("prefix ", "fallback", false),
            "*fallback hook*\n\nprefix"
        );

        let message = ContextMessage {
            role: ContextMessageRole::User,
            content: Vec::new(),
            tool_calls: Vec::new(),
            tool_call_id: None,
            origin: Some(PromptOrigin {
                kind: PromptOriginKind::BackgroundTask,
                fields: Map::new(),
            }),
        };
        assert!(background_origin(&message).is_some());
    }
}
