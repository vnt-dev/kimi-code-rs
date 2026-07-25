//! Context projector implementation.
//!
//! Original: `packages/agent-core-v2/src/agent/contextProjector/contextProjectorService.ts`.

use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex},
};

use sha2::{Digest, Sha256};

use crate::{
    _base::{
        di::{
            descriptors::SyncDescriptor,
            instantiation::ServicesAccessorExt,
            scope::{InstantiationType, LifecycleScope, register_scoped_service},
        },
        log::{LOG_SERVICE_ID, LogPayload, LogServiceHandle},
    },
    agent::{
        context_memory::{
            ContextMessage, PromptOrigin, RenderableToolOutput, RenderableToolResult,
            is_vacuous_content_part, render_tool_result_for_model,
        },
        context_projector::{
            AGENT_CONTEXT_PROJECTOR_SERVICE_ID, AgentContextProjectorServiceContract,
            AgentContextProjectorServiceHandle, ContextProjectorError, ContextProjectorResult,
            MediaStripSnapshot,
        },
    },
    app::telemetry::{TELEMETRY_SERVICE_ID, TelemetryProperties, TelemetryServiceHandle},
    kosong::contract::message::{ContentPart, MediaUrl, Message, Role},
};

pub const MEDIA_DEGRADE_KEEP_RECENT: usize = 2;
pub const TOOL_INTERRUPTED_TEXT: &str = "Tool result is not available in the current context. Do not assume the tool completed successfully.";

const IMAGE_DEGRADED: &str =
    "[image omitted: dropped to fit the provider request size limit; re-read the file to view it]";
const AUDIO_DEGRADED: &str =
    "[audio omitted: dropped to fit the provider request size limit; re-read the file to hear it]";
const VIDEO_DEGRADED: &str =
    "[video omitted: dropped to fit the provider request size limit; re-read the file to view it]";
const IMAGE_STRIPPED: &str = "[image omitted for provider compatibility; re-read the file to view it or get conversion guidance]";
const AUDIO_STRIPPED: &str =
    "[audio omitted for provider compatibility; re-read the file to hear it]";
const VIDEO_STRIPPED: &str =
    "[video omitted for provider compatibility; re-read the file to view it]";

#[derive(Clone, Debug, Eq, PartialEq)]
enum ProjectionAnomaly {
    ToolResultReordered(String),
    ToolResultSynthesized(String, bool),
    OrphanToolResultDropped(String),
    DuplicateToolCallDropped(String),
    DuplicateToolResultDropped(String),
    LeadingNonUserDropped(Role),
    ConsecutiveAssistantsMerged,
    WhitespaceTextDropped(Role),
    VacuousMessageDropped(Role),
}

pub struct AgentContextProjectorService {
    log: LogServiceHandle,
    telemetry: TelemetryServiceHandle,
    last_repair_signature: Mutex<Option<String>>,
}

impl AgentContextProjectorService {
    pub fn new(log: LogServiceHandle, telemetry: TelemetryServiceHandle) -> Self {
        Self {
            log,
            telemetry,
            last_repair_signature: Mutex::new(None),
        }
    }

    fn with_trace(
        &self,
        messages: &[ContextMessage],
        strict: bool,
    ) -> ContextProjectorResult<Vec<Message>> {
        let mut anomalies = Vec::new();
        let projected = project(messages, &mut anomalies)?;
        let projected = if strict {
            project_strict(projected, &mut anomalies)
        } else {
            projected
        };
        self.report_repairs(&anomalies);
        Ok(projected)
    }

    fn report_repairs(&self, anomalies: &[ProjectionAnomaly]) {
        let notable: Vec<_> = anomalies
            .iter()
            .filter(|a| !matches!(a, ProjectionAnomaly::ToolResultSynthesized(_, true)))
            .collect();
        if notable.is_empty() {
            *self.last_repair_signature.lock().unwrap() = None;
            return;
        }
        let mut signature: Vec<_> = notable.iter().map(|a| format!("{a:?}")).collect();
        signature.sort();
        let signature = signature.join("|");
        let mut last = self.last_repair_signature.lock().unwrap();
        if last.as_deref() == Some(&signature) {
            return;
        }
        *last = Some(signature);
        let count = |predicate: fn(&ProjectionAnomaly) -> bool| {
            notable.iter().filter(|a| predicate(a)).count() as i64
        };
        let reordered = count(|a| matches!(a, ProjectionAnomaly::ToolResultReordered(_)));
        let synthesized = count(|a| matches!(a, ProjectionAnomaly::ToolResultSynthesized(_, _)));
        let orphan = count(|a| matches!(a, ProjectionAnomaly::OrphanToolResultDropped(_)));
        let duplicate_calls =
            count(|a| matches!(a, ProjectionAnomaly::DuplicateToolCallDropped(_)));
        let duplicate_results =
            count(|a| matches!(a, ProjectionAnomaly::DuplicateToolResultDropped(_)));
        let leading = count(|a| matches!(a, ProjectionAnomaly::LeadingNonUserDropped(_)));
        let merged = count(|a| matches!(a, ProjectionAnomaly::ConsecutiveAssistantsMerged));
        let whitespace = count(|a| matches!(a, ProjectionAnomaly::WhitespaceTextDropped(_)));
        let vacuous = count(|a| matches!(a, ProjectionAnomaly::VacuousMessageDropped(_)));
        self.log.0.warn(
            "repaired the request to keep it wire-valid",
            Some(LogPayload::Context(serde_json::Map::from_iter([
                ("reordered".into(), reordered.into()),
                ("synthesized".into(), synthesized.into()),
                ("droppedOrphan".into(), orphan.into()),
                ("duplicateCallsDropped".into(), duplicate_calls.into()),
                ("duplicateResultsDropped".into(), duplicate_results.into()),
                ("leadingDropped".into(), leading.into()),
                ("assistantsMerged".into(), merged.into()),
                ("whitespaceDropped".into(), whitespace.into()),
                ("vacuousDropped".into(), vacuous.into()),
            ]))),
        );
        self.telemetry.track(
            "context_projection_repaired",
            Some(&TelemetryProperties::from([
                ("reordered".into(), Some(reordered.into())),
                ("synthesized".into(), Some(synthesized.into())),
                ("dropped_orphan".into(), Some(orphan.into())),
                (
                    "duplicate_calls_dropped".into(),
                    Some(duplicate_calls.into()),
                ),
                (
                    "duplicate_results_dropped".into(),
                    Some(duplicate_results.into()),
                ),
                ("leading_dropped".into(), Some(leading.into())),
                ("assistants_merged".into(), Some(merged.into())),
                ("whitespace_dropped".into(), Some(whitespace.into())),
                ("vacuous_dropped".into(), Some(vacuous.into())),
            ])),
        );
    }
}

impl AgentContextProjectorServiceContract for AgentContextProjectorService {
    fn project(&self, messages: &[ContextMessage]) -> ContextProjectorResult<Vec<Message>> {
        self.with_trace(messages, false)
    }
    fn project_strict(&self, messages: &[ContextMessage]) -> ContextProjectorResult<Vec<Message>> {
        self.with_trace(messages, true)
    }
    fn project_media_degraded(
        &self,
        messages: &[ContextMessage],
    ) -> ContextProjectorResult<Vec<Message>> {
        Ok(degrade_older_media_parts(
            &self.with_trace(messages, false)?,
            MEDIA_DEGRADE_KEEP_RECENT,
            false,
        ))
    }
    fn capture_media_strip_snapshot(
        &self,
        messages: &[ContextMessage],
    ) -> ContextProjectorResult<MediaStripSnapshot> {
        Ok(capture_media_strip_snapshot(
            &self.with_trace(messages, false)?,
        ))
    }
    fn project_media_stripped(
        &self,
        messages: &[ContextMessage],
        snapshot: Option<&MediaStripSnapshot>,
    ) -> ContextProjectorResult<Vec<Message>> {
        let projected = self.with_trace(messages, false)?;
        Ok(strip_media_parts_by_snapshot(
            &projected,
            snapshot
                .cloned()
                .unwrap_or_else(|| capture_media_strip_snapshot(&projected)),
        ))
    }
}

pub fn register_agent_context_projector_service() {
    register_scoped_service(
        LifecycleScope::Agent,
        AGENT_CONTEXT_PROJECTOR_SERVICE_ID,
        SyncDescriptor::new(|accessor| {
            let log = accessor.get(LOG_SERVICE_ID)?;
            let telemetry = accessor.get(TELEMETRY_SERVICE_ID)?;
            Ok(AgentContextProjectorServiceHandle(Arc::new(
                AgentContextProjectorService::new((*log).clone(), (*telemetry).clone()),
            )))
        }),
        InstantiationType::Eager,
        "contextProjector",
    );
}

fn project(
    history: &[ContextMessage],
    anomalies: &mut Vec<ProjectionAnomaly>,
) -> ContextProjectorResult<Vec<Message>> {
    let has_assistant = history
        .iter()
        .any(|m| m.message.partial != Some(true) && m.message.role == Role::Assistant);
    let last_non_tool = history
        .iter()
        .rposition(|m| m.message.role != Role::Tool && m.message.partial != Some(true));
    let mut out = Vec::new();
    let mut slots: HashMap<String, (usize, usize, bool)> = HashMap::new();
    let mut user_merge: Option<usize> = None;
    for (index, source) in history.iter().enumerate() {
        if source.message.partial == Some(true) {
            continue;
        }
        if source.message.role == Role::Tool && has_assistant {
            let Some(id) = source.message.tool_call_id.clone() else {
                continue;
            };
            let Some((slot, _, foreign)) = slots.remove(&id) else {
                anomalies.push(ProjectionAnomaly::OrphanToolResultDropped(id));
                continue;
            };
            if foreign {
                anomalies.push(ProjectionAnomaly::ToolResultReordered(id));
            }
            out[slot] = to_wire(source, clean_content(source, anomalies)?);
            user_merge = None;
            continue;
        }
        if !slots.is_empty() {
            for slot in slots.values_mut() {
                slot.2 = true;
            }
        }
        let content = clean_content(source, anomalies)?;
        if source.message.tool_calls.is_empty()
            && source.message.tools.as_ref().is_none_or(Vec::is_empty)
        {
            if content.is_empty() {
                continue;
            }
            if content.iter().all(is_vacuous_content_part) {
                anomalies.push(ProjectionAnomaly::VacuousMessageDropped(
                    source.message.role,
                ));
                continue;
            }
        }
        let can_merge =
            source.message.role == Role::User && matches!(source.origin, Some(PromptOrigin::User));
        if can_merge && user_merge.is_some() {
            let prior = &mut out[user_merge.unwrap()];
            prior.content.push(ContentPart::Text {
                text: "\n\n".into(),
            });
            prior.content.extend(content);
            continue;
        }
        out.push(to_wire(source, content));
        user_merge = can_merge.then_some(out.len() - 1);
        for call in &source.message.tool_calls {
            if let Some((slot, owner, _)) = slots.insert(call.id.clone(), (out.len(), index, false))
            {
                out[slot] = interrupted_tool_result(&call.id);
                anomalies.push(ProjectionAnomaly::ToolResultSynthesized(
                    call.id.clone(),
                    last_non_tool.is_some_and(|last| owner >= last),
                ));
            }
            out.push(interrupted_tool_result(&call.id));
        }
    }
    for (id, (slot, owner, _)) in slots {
        out[slot] = interrupted_tool_result(&id);
        anomalies.push(ProjectionAnomaly::ToolResultSynthesized(
            id,
            last_non_tool.is_some_and(|last| owner >= last),
        ));
    }
    Ok(out)
}

fn project_strict(messages: Vec<Message>, anomalies: &mut Vec<ProjectionAnomaly>) -> Vec<Message> {
    let mut calls = HashSet::new();
    let mut results: HashMap<String, usize> = HashMap::new();
    let mut out = Vec::new();
    for mut message in messages {
        if message.role == Role::Assistant {
            message.tool_calls.retain(|call| {
                if calls.insert(call.id.clone()) {
                    true
                } else {
                    anomalies.push(ProjectionAnomaly::DuplicateToolCallDropped(call.id.clone()));
                    false
                }
            });
        }
        if message.role == Role::Tool {
            if let Some(id) = message.tool_call_id.clone() {
                if results.insert(id.clone(), out.len()).is_some() {
                    anomalies.push(ProjectionAnomaly::DuplicateToolResultDropped(id));
                    continue;
                }
            }
        }
        if out.last().is_some_and(|previous: &Message| {
            previous.role == Role::Assistant && message.role == Role::Assistant
        }) {
            let previous = out.last_mut().unwrap();
            previous.content.extend(message.content);
            previous.tool_calls.extend(message.tool_calls);
            anomalies.push(ProjectionAnomaly::ConsecutiveAssistantsMerged);
        } else {
            out.push(message);
        }
    }
    let first = out
        .iter()
        .position(|m| m.role == Role::User)
        .unwrap_or(out.len());
    for message in &out[..first] {
        anomalies.push(ProjectionAnomaly::LeadingNonUserDropped(message.role));
    }
    out.into_iter().skip(first).collect()
}

fn clean_content(
    source: &ContextMessage,
    anomalies: &mut Vec<ProjectionAnomaly>,
) -> ContextProjectorResult<Vec<ContentPart>> {
    let raw = if source.message.role == Role::Tool {
        let output = if source.message.content.len() == 1 {
            match &source.message.content[0] {
                ContentPart::Text { text } => RenderableToolOutput::Text(text),
                _ => RenderableToolOutput::Parts(&source.message.content),
            }
        } else {
            RenderableToolOutput::Parts(&source.message.content)
        };
        render_tool_result_for_model(RenderableToolResult {
            output,
            note: source.note.as_deref(),
            is_error: source.is_error,
        })
    } else {
        source.message.content.clone()
    };
    let content: Vec<_> = raw
        .into_iter()
        .filter(|part| match part {
            ContentPart::Text { text } if text.trim().is_empty() => {
                if !text.is_empty() {
                    anomalies.push(ProjectionAnomaly::WhitespaceTextDropped(
                        source.message.role,
                    ));
                }
                false
            }
            _ => true,
        })
        .collect();
    if source.message.role == Role::Tool && content.is_empty() {
        return Err(ContextProjectorError::EmptyToolResult {
            tool_call_id: source.message.tool_call_id.clone(),
        });
    }
    Ok(content)
}

fn to_wire(source: &ContextMessage, content: Vec<ContentPart>) -> Message {
    let mut message = source.message.clone();
    message.content = content;
    message
}
fn interrupted_tool_result(id: &str) -> Message {
    let mut message = Message::new(
        Role::Tool,
        vec![ContentPart::Text {
            text: TOOL_INTERRUPTED_TEXT.into(),
        }],
        Vec::new(),
    );
    message.tool_call_id = Some(id.into());
    message
}

fn media_key(part: &ContentPart) -> Option<String> {
    let (kind, value): (&str, &MediaUrl) = match part {
        ContentPart::ImageUrl { image_url } => ("image_url", image_url),
        ContentPart::AudioUrl { audio_url } => ("audio_url", audio_url),
        ContentPart::VideoUrl { video_url } => ("video_url", video_url),
        _ => return None,
    };
    let mut hash = Sha256::new();
    hash.update(kind);
    hash.update([0]);
    hash.update(value.id.as_deref().unwrap_or(""));
    hash.update([0]);
    hash.update(&value.url);
    Some(
        hash.finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect(),
    )
}
fn placeholder(part: &ContentPart, stripped: bool) -> Option<&'static str> {
    Some(match (part, stripped) {
        (ContentPart::ImageUrl { .. }, false) => IMAGE_DEGRADED,
        (ContentPart::AudioUrl { .. }, false) => AUDIO_DEGRADED,
        (ContentPart::VideoUrl { .. }, false) => VIDEO_DEGRADED,
        (ContentPart::ImageUrl { .. }, true) => IMAGE_STRIPPED,
        (ContentPart::AudioUrl { .. }, true) => AUDIO_STRIPPED,
        (ContentPart::VideoUrl { .. }, true) => VIDEO_STRIPPED,
        _ => return None,
    })
}
pub fn capture_media_strip_snapshot(messages: &[Message]) -> MediaStripSnapshot {
    MediaStripSnapshot {
        keys: messages
            .iter()
            .flat_map(|m| m.content.iter())
            .filter_map(media_key)
            .collect(),
    }
}
pub fn strip_media_parts_by_snapshot(
    messages: &[Message],
    snapshot: MediaStripSnapshot,
) -> Vec<Message> {
    messages
        .iter()
        .cloned()
        .map(|mut message| {
            message.content = message
                .content
                .into_iter()
                .map(|part| {
                    if media_key(&part).is_some_and(|key| snapshot.keys.contains(&key)) {
                        ContentPart::Text {
                            text: placeholder(&part, true).unwrap().into(),
                        }
                    } else {
                        part
                    }
                })
                .collect();
            message
        })
        .collect()
}
pub fn degrade_older_media_parts(
    messages: &[Message],
    keep_recent: usize,
    stripped: bool,
) -> Vec<Message> {
    let mut remaining = messages
        .iter()
        .flat_map(|m| m.content.iter())
        .filter(|p| media_key(p).is_some())
        .count()
        .saturating_sub(keep_recent);
    messages
        .iter()
        .cloned()
        .map(|mut message| {
            message.content = message
                .content
                .into_iter()
                .map(|part| {
                    if remaining > 0 && media_key(&part).is_some() {
                        remaining -= 1;
                        ContentPart::Text {
                            text: placeholder(&part, stripped).unwrap().into(),
                        }
                    } else {
                        part
                    }
                })
                .collect();
            message
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn media(url: &str) -> ContentPart {
        ContentPart::ImageUrl {
            image_url: MediaUrl {
                url: url.into(),
                id: None,
            },
        }
    }

    fn message(parts: Vec<ContentPart>) -> Message {
        Message::new(Role::User, parts, Vec::new())
    }

    #[test]
    fn media_snapshot_strips_only_media_present_when_captured() {
        let original = vec![message(vec![media("old.png")])];
        let snapshot = capture_media_strip_snapshot(&original);
        let later = vec![message(vec![media("old.png"), media("new.png")])];
        let stripped = strip_media_parts_by_snapshot(&later, snapshot);
        assert!(matches!(stripped[0].content[0], ContentPart::Text { .. }));
        assert!(matches!(
            stripped[0].content[1],
            ContentPart::ImageUrl { .. }
        ));
    }

    #[test]
    fn media_degradation_keeps_the_two_most_recent_media_parts() {
        let messages = vec![message(vec![media("one"), media("two"), media("three")])];
        let degraded = degrade_older_media_parts(&messages, MEDIA_DEGRADE_KEEP_RECENT, false);
        assert!(matches!(degraded[0].content[0], ContentPart::Text { .. }));
        assert!(matches!(
            degraded[0].content[1],
            ContentPart::ImageUrl { .. }
        ));
        assert!(matches!(
            degraded[0].content[2],
            ContentPart::ImageUrl { .. }
        ));
    }
}
