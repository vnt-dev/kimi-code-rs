//! L1 store for one agent.
//!
//! Original:
//!   `packages/transcript/src/store/agentTranscript.ts`

use indexmap::IndexMap;

use crate::model::{
    AgentId, AttachmentId, InteractionId, TaskId, TodoId, TranscriptAttachment,
    TranscriptInteraction, TranscriptItem, TranscriptMeta, TranscriptTask, TranscriptTodo,
    TranscriptTurn, TurnId,
};
use crate::ops::{
    AgentState, AgentTranscriptSnapshot, AppendGap, AppliedOps, TranscriptChangeEvent,
    TranscriptOperation, apply_operation,
};

use super::{Disposable, ListenerRegistry};

pub type TranscriptListener = dyn FnMut(&TranscriptChangeEvent);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SnapshotWindow {
    pub tail_turns: usize,
}

pub struct AgentTranscript {
    pub agent_id: AgentId,
    state: AgentState,
    listeners: ListenerRegistry<TranscriptChangeEvent>,
}

impl AgentTranscript {
    pub fn new(agent_id: AgentId) -> Self {
        Self {
            agent_id,
            state: AgentState::default(),
            listeners: ListenerRegistry::new(),
        }
    }

    /// Full loads use the same convergence path as incremental operations.
    pub fn receive(&mut self, operations: &[TranscriptOperation]) -> AppliedOps {
        self.apply(operations)
    }

    /// Apply a causal batch and emit exactly one notification when it mutates.
    pub fn apply(&mut self, operations: &[TranscriptOperation]) -> AppliedOps {
        let mut accepted = Vec::new();
        let mut gap = None;
        let mut state = self.state.clone();

        for operation in operations {
            let result = apply_operation(&state, operation);
            if let Some(offset_gap) = result.gap {
                if let TranscriptOperation::Append { target, .. } = operation {
                    gap = Some(AppendGap {
                        target: target.clone(),
                        expected: offset_gap.expected,
                        got: offset_gap.got,
                    });
                }
                continue;
            }
            if !result.changed {
                continue;
            }
            state = result.state;
            accepted.push(operation.clone());
        }

        self.state = state;
        if !accepted.is_empty() {
            self.listeners.emit(&TranscriptChangeEvent {
                agent_id: self.agent_id.clone(),
                ops: accepted.clone(),
            });
        }
        AppliedOps { accepted, gap }
    }

    pub fn on_change(&self, listener: impl FnMut(&TranscriptChangeEvent) + 'static) -> Disposable {
        self.listeners.register(listener)
    }

    pub fn get_items(&self) -> &[TranscriptItem] {
        &self.state.items
    }

    pub fn get_turn(&self, turn_id: &TurnId) -> Option<&TranscriptTurn> {
        self.state.items.iter().find_map(|item| match item {
            TranscriptItem::Turn(turn) if turn.turn_id == *turn_id => Some(turn),
            _ => None,
        })
    }

    pub fn get_tasks(&self) -> &IndexMap<TaskId, TranscriptTask> {
        &self.state.tasks
    }

    pub fn get_task(&self, task_id: &TaskId) -> Option<&TranscriptTask> {
        self.state.tasks.get(task_id)
    }

    pub fn get_interactions(&self) -> &IndexMap<InteractionId, TranscriptInteraction> {
        &self.state.interactions
    }

    pub fn get_interaction(
        &self,
        interaction_id: &InteractionId,
    ) -> Option<&TranscriptInteraction> {
        self.state.interactions.get(interaction_id)
    }

    pub fn get_attachments(&self) -> &IndexMap<AttachmentId, TranscriptAttachment> {
        &self.state.attachments
    }

    pub fn get_attachment(&self, attachment_id: &AttachmentId) -> Option<&TranscriptAttachment> {
        self.state.attachments.get(attachment_id)
    }

    pub fn get_todos(&self) -> &IndexMap<TodoId, TranscriptTodo> {
        &self.state.todos
    }

    pub fn get_todo(&self, todo_id: &TodoId) -> Option<&TranscriptTodo> {
        self.state.todos.get(todo_id)
    }

    pub fn get_meta(&self) -> &TranscriptMeta {
        &self.state.meta
    }

    pub fn list_pending_interactions(&self) -> Vec<InteractionId> {
        self.state.pending_interactions.iter().cloned().collect()
    }

    pub fn has_more_older(&self) -> bool {
        self.state.has_more_older
    }

    /// Materialize the current state, optionally keeping only newest turns.
    pub fn snapshot(&self, window: Option<SnapshotWindow>) -> AgentTranscriptSnapshot {
        let mut items = self.state.items.as_ref().clone();
        let mut has_more_older = self.state.has_more_older;

        if let Some(window) = window {
            let turn_count = items
                .iter()
                .filter(|item| matches!(item, TranscriptItem::Turn(_)))
                .count();
            if turn_count > window.tail_turns {
                let skip = turn_count - window.tail_turns;
                let mut kept = Vec::new();
                let mut seen = 0;
                for item in items {
                    if matches!(item, TranscriptItem::Turn(_)) {
                        seen += 1;
                        if seen <= skip {
                            continue;
                        }
                        kept.push(item);
                    } else if seen > skip {
                        kept.push(item);
                    }
                }
                items = kept;
                has_more_older = true;
            }
        }

        AgentTranscriptSnapshot {
            items,
            tasks: self.state.tasks.values().cloned().collect(),
            interactions: self.state.interactions.values().cloned().collect(),
            attachments: self.state.attachments.values().cloned().collect(),
            todos: self.state.todos.values().cloned().collect(),
            meta: self.state.meta.as_ref().clone(),
            has_more_older: Some(has_more_older),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use crate::model::{
        FrameId, MarkerId, TextFrame, TextRole, TranscriptFrame, TranscriptMarker, TurnOrigin,
        TurnState,
    };
    use crate::ops::{AppendTarget, TurnHeader};

    use super::*;

    fn turn(id: &str, ordinal: i64) -> TranscriptOperation {
        TranscriptOperation::TurnUpsert {
            turn: TurnHeader {
                turn_id: TurnId::from(id),
                ordinal,
                state: TurnState::Completed,
                origin: TurnOrigin::User { payload: None },
                prompt: None,
                attachment_ids: None,
                started_at: None,
                ended_at: None,
                usage: None,
            },
        }
    }

    #[test]
    fn applies_batch_collects_last_gap_and_continues() {
        let mut transcript = AgentTranscript::new(AgentId::from("main"));
        let target = AppendTarget::Frame {
            turn_id: TurnId::from("t1"),
            step_id: crate::model::StepId::from("t1.0"),
            frame_id: FrameId::from("f1"),
        };
        let result = transcript.apply(&[
            TranscriptOperation::Append {
                target: target.clone(),
                offset: 5,
                text: "late".to_owned(),
            },
            turn("t1", 1),
        ]);
        assert_eq!(result.accepted.len(), 1);
        assert_eq!(
            result.gap,
            Some(AppendGap {
                target,
                expected: 0,
                got: 5
            })
        );
        assert!(transcript.get_turn(&TurnId::from("t1")).is_some());
    }

    #[test]
    fn notifies_once_per_mutating_batch_and_disposes_explicitly() {
        let mut transcript = AgentTranscript::new(AgentId::from("main"));
        let seen = Rc::new(RefCell::new(Vec::new()));
        let callback_seen = seen.clone();
        let mut disposable = transcript.on_change(move |event| {
            callback_seen.borrow_mut().push(event.ops.len());
        });
        transcript.apply(&[turn("t1", 1), turn("t1", 1)]);
        assert_eq!(*seen.borrow(), [1]);
        disposable.dispose();
        transcript.apply(&[turn("t2", 2)]);
        assert_eq!(*seen.borrow(), [1]);
    }

    #[test]
    fn windows_complete_turn_segments_and_round_trips_through_receive() {
        let mut transcript = AgentTranscript::new(AgentId::from("main"));
        for ordinal in 1..=5 {
            transcript.apply(&[
                TranscriptOperation::MarkerUpsert {
                    item: TranscriptMarker {
                        marker_id: MarkerId::new(format!("m{ordinal}")),
                        marker: "goal".to_owned(),
                        payload: None,
                        at: None,
                    },
                    before_turn: None,
                },
                turn(&format!("t{ordinal}"), ordinal),
            ]);
        }
        let snapshot = transcript.snapshot(Some(SnapshotWindow { tail_turns: 2 }));
        let turns: Vec<_> = snapshot
            .items
            .iter()
            .filter_map(|item| match item {
                TranscriptItem::Turn(turn) => Some(turn.turn_id.as_ref()),
                _ => None,
            })
            .collect();
        assert_eq!(turns, ["t4", "t5"]);
        assert_eq!(snapshot.has_more_older, Some(true));

        let mut fresh = AgentTranscript::new(AgentId::from("main"));
        fresh.receive(&[TranscriptOperation::Reset {
            agent_id: AgentId::from("main"),
            snapshot: snapshot.clone(),
        }]);
        assert_eq!(fresh.get_items(), snapshot.items);
        assert!(fresh.has_more_older());
    }

    #[test]
    fn appends_frame_text_through_store_convergence_path() {
        let mut transcript = AgentTranscript::new(AgentId::from("main"));
        transcript.apply(&[
            TranscriptOperation::FrameUpsert {
                turn_id: TurnId::from("t1"),
                step_id: crate::model::StepId::from("t1.0"),
                frame: TranscriptFrame::Text(TextFrame {
                    frame_id: FrameId::from("f1"),
                    role: TextRole::Assistant,
                    text: String::new(),
                    attachment_ids: None,
                    task_id: None,
                }),
            },
            TranscriptOperation::Append {
                target: AppendTarget::Frame {
                    turn_id: TurnId::from("t1"),
                    step_id: crate::model::StepId::from("t1.0"),
                    frame_id: FrameId::from("f1"),
                },
                offset: 0,
                text: "hello".to_owned(),
            },
        ]);
        let frame = &transcript.get_turn(&TurnId::from("t1")).unwrap().steps[0].frames[0];
        assert!(matches!(frame, TranscriptFrame::Text(frame) if frame.text == "hello"));
    }
}
