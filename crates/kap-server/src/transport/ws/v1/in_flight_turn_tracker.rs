use std::collections::HashMap;

use indexmap::IndexMap;
use kimi_code_protocol::display::OptionalJsonValue;
use kimi_code_protocol::rest::snapshot::{
    InFlightProgressKind, InFlightToolCall, InFlightToolProgress, InFlightTurn,
};
use serde_json::Value;

const MAIN_AGENT_ID: &str = "main";

#[derive(Debug, Clone, PartialEq)]
pub enum TrackedEvent {
    TurnStarted {
        agent_id: String,
        turn_id: u64,
    },
    TurnEnded {
        agent_id: String,
    },
    TurnStepStarted {
        agent_id: String,
        turn_id: u64,
    },
    AssistantDelta {
        agent_id: String,
        turn_id: u64,
        delta: String,
    },
    ThinkingDelta {
        agent_id: String,
        turn_id: u64,
        delta: String,
    },
    ToolCallStarted {
        agent_id: String,
        turn_id: u64,
        tool_call_id: String,
        name: String,
        args: OptionalJsonValue,
        description: Option<String>,
        display: OptionalJsonValue,
    },
    ToolProgress {
        agent_id: String,
        tool_call_id: String,
        update: TrackedToolProgress,
    },
    ToolResult {
        agent_id: String,
        tool_call_id: String,
    },
    Other {
        agent_id: String,
    },
}

impl TrackedEvent {
    fn agent_id(&self) -> &str {
        match self {
            Self::TurnStarted { agent_id, .. }
            | Self::TurnEnded { agent_id }
            | Self::TurnStepStarted { agent_id, .. }
            | Self::AssistantDelta { agent_id, .. }
            | Self::ThinkingDelta { agent_id, .. }
            | Self::ToolCallStarted { agent_id, .. }
            | Self::ToolProgress { agent_id, .. }
            | Self::ToolResult { agent_id, .. }
            | Self::Other { agent_id } => agent_id,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct TrackedToolProgress {
    pub kind: InFlightProgressKind,
    pub text: Option<String>,
    pub percent: Option<f64>,
    pub custom: Option<Value>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct VolatileAnnotation {
    pub offset: Option<u64>,
}

#[derive(Debug, Clone)]
struct TurnAccum {
    turn_id: u64,
    assistant_text: String,
    thinking_text: String,
    tools: IndexMap<String, InFlightToolCall>,
}

#[derive(Debug, Default)]
pub struct InFlightTurnTracker {
    by_session: HashMap<String, TurnAccum>,
}

impl InFlightTurnTracker {
    // Original: inFlightTurnTracker.ts, InFlightTurnTracker.apply().
    pub fn apply(&mut self, session_id: &str, event: TrackedEvent) -> VolatileAnnotation {
        if event.agent_id() != MAIN_AGENT_ID {
            return VolatileAnnotation::default();
        }

        match event {
            TrackedEvent::TurnStarted { turn_id, .. } => {
                self.by_session.insert(
                    session_id.to_owned(),
                    TurnAccum {
                        turn_id,
                        assistant_text: String::new(),
                        thinking_text: String::new(),
                        tools: IndexMap::new(),
                    },
                );
                VolatileAnnotation::default()
            }
            TrackedEvent::TurnEnded { .. } => {
                self.by_session.remove(session_id);
                VolatileAnnotation::default()
            }
            TrackedEvent::TurnStepStarted { turn_id, .. } => {
                if let Some(turn) = self
                    .by_session
                    .get_mut(session_id)
                    .filter(|turn| turn.turn_id == turn_id)
                {
                    turn.assistant_text.clear();
                    turn.thinking_text.clear();
                }
                VolatileAnnotation::default()
            }
            TrackedEvent::AssistantDelta { turn_id, delta, .. } => {
                let Some(turn) = self
                    .by_session
                    .get_mut(session_id)
                    .filter(|turn| turn.turn_id == turn_id)
                else {
                    return VolatileAnnotation::default();
                };
                // JavaScript String.length counts UTF-16 code units, not UTF-8
                // bytes or Unicode scalar values.
                let offset = turn.assistant_text.encode_utf16().count() as u64;
                turn.assistant_text.push_str(&delta);
                VolatileAnnotation {
                    offset: Some(offset),
                }
            }
            TrackedEvent::ThinkingDelta { turn_id, delta, .. } => {
                let Some(turn) = self
                    .by_session
                    .get_mut(session_id)
                    .filter(|turn| turn.turn_id == turn_id)
                else {
                    return VolatileAnnotation::default();
                };
                let offset = turn.thinking_text.encode_utf16().count() as u64;
                turn.thinking_text.push_str(&delta);
                VolatileAnnotation {
                    offset: Some(offset),
                }
            }
            TrackedEvent::ToolCallStarted {
                turn_id,
                tool_call_id,
                name,
                args,
                description,
                display,
                ..
            } => {
                if let Some(turn) = self
                    .by_session
                    .get_mut(session_id)
                    .filter(|turn| turn.turn_id == turn_id)
                {
                    turn.tools.insert(
                        tool_call_id.clone(),
                        InFlightToolCall {
                            tool_call_id,
                            name,
                            args,
                            description,
                            display,
                            last_progress: None,
                        },
                    );
                }
                VolatileAnnotation::default()
            }
            TrackedEvent::ToolProgress {
                tool_call_id,
                update,
                ..
            } => {
                if update.kind == InFlightProgressKind::Custom {
                    return VolatileAnnotation::default();
                }
                if let Some(tool) = self
                    .by_session
                    .get_mut(session_id)
                    .and_then(|turn| turn.tools.get_mut(&tool_call_id))
                {
                    tool.last_progress = Some(InFlightToolProgress {
                        kind: update.kind,
                        text: update.text,
                        percent: update.percent,
                    });
                }
                VolatileAnnotation::default()
            }
            TrackedEvent::ToolResult { tool_call_id, .. } => {
                if let Some(turn) = self.by_session.get_mut(session_id) {
                    turn.tools.shift_remove(&tool_call_id);
                }
                VolatileAnnotation::default()
            }
            TrackedEvent::Other { .. } => VolatileAnnotation::default(),
        }
    }

    pub fn get(&self, session_id: &str) -> Option<InFlightTurn> {
        let turn = self.by_session.get(session_id)?;
        Some(InFlightTurn {
            turn_id: turn.turn_id,
            assistant_text: turn.assistant_text.clone(),
            thinking_text: turn.thinking_text.clone(),
            running_tools: turn.tools.values().cloned().collect(),
            current_prompt_id: None,
        })
    }

    pub fn clear(&mut self, session_id: &str) {
        self.by_session.remove(session_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SESSION: &str = "sess_1";

    fn started(turn_id: u64) -> TrackedEvent {
        TrackedEvent::TurnStarted {
            agent_id: MAIN_AGENT_ID.into(),
            turn_id,
        }
    }

    fn assistant(turn_id: u64, delta: &str) -> TrackedEvent {
        TrackedEvent::AssistantDelta {
            agent_id: MAIN_AGENT_ID.into(),
            turn_id,
            delta: delta.into(),
        }
    }

    #[test]
    fn accumulates_text_with_javascript_offsets() {
        let mut tracker = InFlightTurnTracker::default();
        tracker.apply(SESSION, started(1));
        assert_eq!(
            tracker.apply(SESSION, assistant(1, "😀")),
            VolatileAnnotation { offset: Some(0) }
        );
        assert_eq!(
            tracker.apply(SESSION, assistant(1, "x")),
            VolatileAnnotation { offset: Some(2) }
        );
        assert_eq!(tracker.get(SESSION).unwrap().assistant_text, "😀x");
        assert_eq!(
            tracker.apply(SESSION, assistant(99, "stale")),
            VolatileAnnotation::default()
        );
    }

    #[test]
    fn resets_text_at_matching_step_and_clears_at_end() {
        let mut tracker = InFlightTurnTracker::default();
        tracker.apply(SESSION, started(1));
        tracker.apply(SESSION, assistant(1, "step one"));
        tracker.apply(
            SESSION,
            TrackedEvent::TurnStepStarted {
                agent_id: MAIN_AGENT_ID.into(),
                turn_id: 99,
            },
        );
        assert_eq!(tracker.get(SESSION).unwrap().assistant_text, "step one");
        tracker.apply(
            SESSION,
            TrackedEvent::TurnStepStarted {
                agent_id: MAIN_AGENT_ID.into(),
                turn_id: 1,
            },
        );
        assert_eq!(tracker.get(SESSION).unwrap().assistant_text, "");
        tracker.apply(
            SESSION,
            TrackedEvent::TurnEnded {
                agent_id: MAIN_AGENT_ID.into(),
            },
        );
        assert!(tracker.get(SESSION).is_none());
    }

    #[test]
    fn ignores_subagents_and_tracks_running_tools() {
        let mut tracker = InFlightTurnTracker::default();
        tracker.apply(SESSION, started(1));
        tracker.apply(
            SESSION,
            TrackedEvent::AssistantDelta {
                agent_id: "subagent".into(),
                turn_id: 1,
                delta: "ignored".into(),
            },
        );
        tracker.apply(
            SESSION,
            TrackedEvent::ToolCallStarted {
                agent_id: MAIN_AGENT_ID.into(),
                turn_id: 1,
                tool_call_id: "tc1".into(),
                name: "bash".into(),
                args: OptionalJsonValue::Absent,
                description: None,
                display: OptionalJsonValue::Absent,
            },
        );
        tracker.apply(
            SESSION,
            TrackedEvent::ToolProgress {
                agent_id: MAIN_AGENT_ID.into(),
                tool_call_id: "tc1".into(),
                update: TrackedToolProgress {
                    kind: InFlightProgressKind::Stdout,
                    text: Some("hi".into()),
                    percent: None,
                    custom: None,
                },
            },
        );
        let turn = tracker.get(SESSION).unwrap();
        assert_eq!(turn.assistant_text, "");
        assert_eq!(turn.running_tools.len(), 1);
        assert_eq!(
            turn.running_tools[0].last_progress.as_ref().unwrap().text,
            Some("hi".into())
        );
        tracker.apply(
            SESSION,
            TrackedEvent::ToolResult {
                agent_id: MAIN_AGENT_ID.into(),
                tool_call_id: "tc1".into(),
            },
        );
        assert!(tracker.get(SESSION).unwrap().running_tools.is_empty());
    }
}
