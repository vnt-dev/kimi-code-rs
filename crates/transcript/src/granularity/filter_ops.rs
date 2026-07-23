//! Grade-based operation clipping and reset redaction.
//!
//! Original:
//!   `packages/transcript/src/granularity/filterOps.ts`

use std::borrow::Cow;

use crate::model::TranscriptItem;
use crate::ops::{AgentTranscriptSnapshot, TranscriptOperation};

use super::TranscriptGrade;

pub fn filter_ops_for_grade(
    grade: TranscriptGrade,
    operations: &[TranscriptOperation],
) -> Vec<&TranscriptOperation> {
    if grade == TranscriptGrade::Off {
        return Vec::new();
    }
    operations
        .iter()
        .filter(|operation| admits(grade, operation))
        .collect()
}

fn admits(grade: TranscriptGrade, operation: &TranscriptOperation) -> bool {
    match operation {
        TranscriptOperation::Append { .. } => grade.rank() >= TranscriptGrade::Delta.rank(),
        TranscriptOperation::StepUpsert { .. } | TranscriptOperation::FrameUpsert { .. } => {
            grade.rank() >= TranscriptGrade::Block.rank()
        }
        _ => true,
    }
}

pub fn is_append_only(operations: &[TranscriptOperation]) -> bool {
    !operations.is_empty()
        && operations
            .iter()
            .all(|operation| matches!(operation, TranscriptOperation::Append { .. }))
}

pub fn redact_snapshot_for_grade(
    grade: TranscriptGrade,
    snapshot: &AgentTranscriptSnapshot,
) -> Cow<'_, AgentTranscriptSnapshot> {
    if grade.rank() >= TranscriptGrade::Block.rank() {
        return Cow::Borrowed(snapshot);
    }
    let mut redacted = snapshot.clone();
    redacted.items = snapshot
        .items
        .iter()
        .cloned()
        .map(|item| match item {
            TranscriptItem::Turn(mut turn) => {
                turn.steps.clear();
                TranscriptItem::Turn(turn)
            }
            item => item,
        })
        .collect();
    Cow::Owned(redacted)
}

#[cfg(test)]
mod tests {
    use crate::model::{
        ActivityMeta, StepId, StepState, TranscriptMetaMerge, TurnId, TurnOrigin, TurnState,
    };
    use crate::ops::{AppendTarget, StepHeader, TurnHeader};

    use super::*;
    use crate::granularity::{TranscriptGradeSpec, grade_for, needs_reset_on_transition};

    fn operations() -> Vec<TranscriptOperation> {
        vec![
            TranscriptOperation::TurnUpsert {
                turn: TurnHeader {
                    turn_id: TurnId::from("t1"),
                    ordinal: 1,
                    state: TurnState::Running,
                    origin: TurnOrigin::other(),
                    prompt: None,
                    attachment_ids: None,
                    started_at: None,
                    ended_at: None,
                    usage: None,
                },
            },
            TranscriptOperation::StepUpsert {
                turn_id: TurnId::from("t1"),
                step: StepHeader {
                    step_id: StepId::from("t1.0"),
                    turn_id: TurnId::from("t1"),
                    ordinal: 0,
                    state: StepState::Running,
                    started_at: None,
                    ended_at: None,
                },
            },
            TranscriptOperation::Append {
                target: AppendTarget::Task {
                    task_id: crate::model::TaskId::from("task"),
                },
                offset: 0,
                text: "x".to_owned(),
            },
            TranscriptOperation::MetaMerge {
                meta: TranscriptMetaMerge {
                    activity: Some(ActivityMeta::Turn),
                    ..Default::default()
                },
            },
        ]
    }

    #[test]
    fn grade_resolution_filtering_and_upgrade_rules_match_rank_table() {
        let mut spec = TranscriptGradeSpec::new();
        spec.insert("*".to_owned(), Some(TranscriptGrade::Turn));
        spec.insert("main".to_owned(), Some(TranscriptGrade::Delta));
        assert_eq!(grade_for(Some(&spec), "main"), TranscriptGrade::Delta);
        assert_eq!(grade_for(Some(&spec), "sub"), TranscriptGrade::Turn);
        assert_eq!(grade_for(None, "main"), TranscriptGrade::Off);
        assert!(needs_reset_on_transition(
            TranscriptGrade::Turn,
            TranscriptGrade::Delta
        ));
        assert!(!needs_reset_on_transition(
            TranscriptGrade::Delta,
            TranscriptGrade::Turn
        ));

        let operations = operations();
        let names = |grade| {
            filter_ops_for_grade(grade, &operations)
                .into_iter()
                .map(TranscriptOperation::op_name)
                .collect::<Vec<_>>()
        };
        assert!(names(TranscriptGrade::Off).is_empty());
        assert_eq!(names(TranscriptGrade::Turn), ["turn.upsert", "meta.merge"]);
        assert_eq!(
            names(TranscriptGrade::Block),
            ["turn.upsert", "step.upsert", "meta.merge"]
        );
        assert_eq!(names(TranscriptGrade::Delta).len(), operations.len());
        assert!(is_append_only(&operations[2..3]));
        assert!(!is_append_only(&operations));
    }

    #[test]
    fn redacts_only_turn_detail_below_block() {
        let snapshot = AgentTranscriptSnapshot {
            items: vec![TranscriptItem::Turn(
                TurnHeader {
                    turn_id: TurnId::from("t1"),
                    ordinal: 1,
                    state: TurnState::Completed,
                    origin: TurnOrigin::other(),
                    prompt: Some("hi".to_owned()),
                    attachment_ids: None,
                    started_at: None,
                    ended_at: None,
                    usage: None,
                }
                .into_turn(vec![
                    StepHeader {
                        step_id: StepId::from("t1.0"),
                        turn_id: TurnId::from("t1"),
                        ordinal: 0,
                        state: StepState::Completed,
                        started_at: None,
                        ended_at: None,
                    }
                    .into_step(Vec::new()),
                ]),
            )],
            ..Default::default()
        };
        let turn = redact_snapshot_for_grade(TranscriptGrade::Turn, &snapshot);
        let TranscriptItem::Turn(turn) = &turn.items[0] else {
            panic!("expected turn");
        };
        assert!(turn.steps.is_empty());
        assert_eq!(turn.prompt.as_deref(), Some("hi"));
        assert!(matches!(
            redact_snapshot_for_grade(TranscriptGrade::Block, &snapshot),
            Cow::Borrowed(_)
        ));
    }
}
