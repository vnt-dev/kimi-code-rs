use std::collections::HashMap;

use indexmap::IndexMap;
use kimi_code_protocol::rest::snapshot::{SnapshotSubagent, SnapshotSubagentPhase};
use kimi_code_protocol::{Task, TaskKind, TaskStatus, now_iso_date_time};

const MAIN_AGENT_ID: &str = "main";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RosterEvent {
    Spawned {
        subagent_id: String,
        subagent_name: Option<String>,
        parent_tool_call_id: String,
        description: Option<String>,
        swarm_index: Option<u64>,
        run_in_background: bool,
    },
    Started {
        subagent_id: String,
    },
    Suspended {
        subagent_id: String,
        reason: String,
    },
    Completed {
        subagent_id: String,
        result_summary: Option<String>,
    },
    Failed {
        subagent_id: String,
        error: String,
    },
    TaskStarted {
        agent_id: Option<String>,
        kind_is_agent: bool,
        detached: bool,
    },
    TurnEnded {
        agent_id: String,
        reason: String,
    },
    TurnStarted {
        agent_id: String,
    },
    Other,
}

#[derive(Debug, Default)]
pub struct SubagentRosterTracker {
    by_session: HashMap<String, IndexMap<String, SnapshotSubagent>>,
}

impl SubagentRosterTracker {
    // Original: subagentRosterTracker.ts, SubagentRosterTracker.apply().
    pub fn apply(&mut self, session_id: &str, event: RosterEvent) {
        match event {
            RosterEvent::Spawned {
                subagent_id,
                subagent_name,
                parent_tool_call_id,
                description,
                swarm_index,
                run_in_background,
            } => {
                if run_in_background {
                    return;
                }
                let created_at = now_iso_date_time();
                self.by_session
                    .entry(session_id.to_owned())
                    .or_default()
                    .insert(
                        subagent_id.clone(),
                        SnapshotSubagent {
                            task: Task {
                                id: subagent_id,
                                session_id: session_id.to_owned(),
                                kind: TaskKind::Subagent,
                                description: description
                                    .or_else(|| subagent_name.clone())
                                    .unwrap_or_else(|| "Sub Agent".into()),
                                status: TaskStatus::Running,
                                command: None,
                                created_at,
                                started_at: None,
                                completed_at: None,
                                output_preview: None,
                                output_bytes: None,
                            },
                            subagent_phase: Some(SnapshotSubagentPhase::Queued),
                            subagent_type: subagent_name,
                            parent_tool_call_id: (!parent_tool_call_id.is_empty())
                                .then_some(parent_tool_call_id),
                            suspended_reason: None,
                            swarm_index,
                            run_in_background: Some(false),
                        },
                    );
            }
            RosterEvent::Started { subagent_id } => {
                if let Some(entry) = self.entry_mut(session_id, &subagent_id) {
                    entry.subagent_phase = Some(SnapshotSubagentPhase::Working);
                    entry.suspended_reason = None;
                    if entry.task.started_at.is_none() {
                        entry.task.started_at = Some(now_iso_date_time());
                    }
                }
            }
            RosterEvent::Suspended {
                subagent_id,
                reason,
            } => {
                if let Some(entry) = self.entry_mut(session_id, &subagent_id) {
                    entry.subagent_phase = Some(SnapshotSubagentPhase::Suspended);
                    entry.suspended_reason = Some(reason);
                }
            }
            RosterEvent::Completed {
                subagent_id,
                result_summary,
            } => {
                if let Some(entry) = self.entry_mut(session_id, &subagent_id) {
                    complete_entry(
                        entry,
                        SnapshotSubagentPhase::Completed,
                        TaskStatus::Completed,
                        result_summary,
                    );
                }
            }
            RosterEvent::Failed { subagent_id, error } => {
                if let Some(entry) = self.entry_mut(session_id, &subagent_id) {
                    complete_entry(
                        entry,
                        SnapshotSubagentPhase::Failed,
                        TaskStatus::Failed,
                        Some(error),
                    );
                }
            }
            RosterEvent::TaskStarted {
                agent_id,
                kind_is_agent,
                detached,
            } => {
                if kind_is_agent
                    && detached
                    && let Some(agent_id) = agent_id
                    && let Some(roster) = self.by_session.get_mut(session_id)
                {
                    roster.shift_remove(&agent_id);
                }
            }
            RosterEvent::TurnEnded { agent_id, reason } => {
                if agent_id != MAIN_AGENT_ID || reason == "completed" {
                    return;
                }
                if let Some(roster) = self.by_session.get_mut(session_id) {
                    for entry in roster.values_mut() {
                        if entry.task.status == TaskStatus::Running {
                            complete_entry(
                                entry,
                                SnapshotSubagentPhase::Failed,
                                TaskStatus::Failed,
                                Some(format!("Main turn {reason}")),
                            );
                        }
                    }
                }
            }
            RosterEvent::TurnStarted { agent_id } => {
                if agent_id == MAIN_AGENT_ID {
                    self.by_session.remove(session_id);
                }
            }
            RosterEvent::Other => {}
        }
    }

    pub fn get(&self, session_id: &str) -> Vec<SnapshotSubagent> {
        self.by_session
            .get(session_id)
            .map(|roster| roster.values().cloned().collect())
            .unwrap_or_default()
    }

    pub fn clear(&mut self, session_id: &str) {
        self.by_session.remove(session_id);
    }

    fn entry_mut(&mut self, session_id: &str, subagent_id: &str) -> Option<&mut SnapshotSubagent> {
        self.by_session.get_mut(session_id)?.get_mut(subagent_id)
    }
}

fn complete_entry(
    entry: &mut SnapshotSubagent,
    phase: SnapshotSubagentPhase,
    status: TaskStatus,
    output: Option<String>,
) {
    entry.subagent_phase = Some(phase);
    entry.task.status = status;
    entry.task.completed_at = Some(now_iso_date_time());
    if output.is_some() {
        entry.task.output_preview = output;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SESSION: &str = "sess_1";

    fn spawn(id: &str, background: bool) -> RosterEvent {
        RosterEvent::Spawned {
            subagent_id: id.into(),
            subagent_name: Some("kimi-subagent".into()),
            parent_tool_call_id: "tc1".into(),
            description: Some(format!("task {id}")),
            swarm_index: Some(2),
            run_in_background: background,
        }
    }

    #[test]
    fn tracks_foreground_lifecycle_and_returns_copies() {
        let mut tracker = SubagentRosterTracker::default();
        tracker.apply(SESSION, spawn("agent-1", false));
        tracker.apply(
            SESSION,
            RosterEvent::Started {
                subagent_id: "agent-1".into(),
            },
        );
        tracker.apply(
            SESSION,
            RosterEvent::Suspended {
                subagent_id: "agent-1".into(),
                reason: "rate limit".into(),
            },
        );
        tracker.apply(
            SESSION,
            RosterEvent::Started {
                subagent_id: "agent-1".into(),
            },
        );
        tracker.apply(
            SESSION,
            RosterEvent::Completed {
                subagent_id: "agent-1".into(),
                result_summary: Some("done".into()),
            },
        );
        let mut copy = tracker.get(SESSION);
        assert_eq!(copy[0].task.status, TaskStatus::Completed);
        assert_eq!(copy[0].task.output_preview.as_deref(), Some("done"));
        copy[0].task.description = "mutated".into();
        assert_eq!(tracker.get(SESSION)[0].task.description, "task agent-1");
    }

    #[test]
    fn skips_background_detaches_and_finalizes_abort() {
        let mut tracker = SubagentRosterTracker::default();
        tracker.apply(SESSION, spawn("background", true));
        assert!(tracker.get(SESSION).is_empty());
        tracker.apply(SESSION, spawn("agent-1", false));
        tracker.apply(
            SESSION,
            RosterEvent::TurnEnded {
                agent_id: MAIN_AGENT_ID.into(),
                reason: "cancelled".into(),
            },
        );
        assert_eq!(tracker.get(SESSION)[0].task.status, TaskStatus::Failed);
        assert_eq!(
            tracker.get(SESSION)[0].task.output_preview.as_deref(),
            Some("Main turn cancelled")
        );
        tracker.apply(
            SESSION,
            RosterEvent::TurnStarted {
                agent_id: MAIN_AGENT_ID.into(),
            },
        );
        assert!(tracker.get(SESSION).is_empty());
    }
}
