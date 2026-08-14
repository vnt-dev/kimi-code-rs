//! Durable turn counter and turn lifecycle wire operations.
//!
//! Original: `packages/agent-core-v2/src/agent/loop/turnOps.ts`.

use std::{collections::BTreeSet, sync::LazyLock};

use serde::{Deserialize, Serialize};

use crate::{
    _base::errors::serialize::KimiErrorPayload,
    agent::{
        context_memory::{ContextAppendLoopEventPayload, LoopRecordedEvent, PromptOrigin},
        loop_::{TurnEndReason, TurnSeed},
    },
    kosong::contract::message::ContentPart,
    wire::{
        model::{ModelCrossReducer, ModelDef, ModelOptions, define_model},
        op::{DefineOpOptions, DefinedOp, Op},
    },
};

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnModelState {
    pub next_turn_id: crate::agent::TurnId,
    pub cancelled_turn_ids: Vec<crate::agent::TurnId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_ended: Option<LastEndedTurn>,
}
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LastEndedTurn {
    pub turn_id: crate::agent::TurnId,
    pub reason: TurnEndReason,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "kimi_code_protocol::lenient::lenient_optional_u64"
    )]
    pub duration_ms: Option<u64>,
}
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct TurnInputPayload {
    pub input: Vec<ContentPart>,
    pub origin: PromptOrigin,
}
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CancelTurnPayload {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<crate::agent::TurnId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<CancelTurnTarget>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<CancelTurnReason>,
}
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CancelTurnTarget {
    Active,
    Queued,
}
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CancelTurnReason {
    UserCancelled,
    Aborted,
}
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EndTurnPayload {
    pub turn_id: crate::agent::TurnId,
    pub reason: TurnEndReason,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<KimiErrorPayload>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "kimi_code_protocol::lenient::lenient_optional_u64"
    )]
    pub duration_ms: Option<u64>,
}

pub static TURN_MODEL: LazyLock<ModelDef<TurnModelState>> = LazyLock::new(|| {
    define_model(
        "turn",
        || TurnModelState {
            next_turn_id: crate::agent::TurnId::new(0),
            cancelled_turn_ids: Vec::new(),
            last_ended: None,
        },
        ModelOptions {
            blobs: None,
            reducers: vec![ModelCrossReducer::typed(
                "context.append_loop_event",
                observe_restored_turn_id,
            )],
        },
    )
});
pub static PROMPT_TURN: LazyLock<DefinedOp<TurnModelState, TurnInputPayload>> =
    LazyLock::new(|| {
        TURN_MODEL
            .define_op(
                "turn.prompt",
                DefineOpOptions::new(|state: TurnModelState, _: &TurnInputPayload| {
                    let next_turn_id = state.next_turn_id + 1;
                    advance_turn_clock(state, next_turn_id, None)
                }),
            )
            .expect("turn.prompt must have one global definition")
    });
pub static STEER_TURN: LazyLock<DefinedOp<TurnModelState, TurnInputPayload>> =
    LazyLock::new(|| {
        TURN_MODEL
            .define_op(
                "turn.steer",
                DefineOpOptions::new(|state: TurnModelState, _: &TurnInputPayload| state),
            )
            .expect("turn.steer must have one global definition")
    });
pub static CANCEL_TURN: LazyLock<DefinedOp<TurnModelState, CancelTurnPayload>> =
    LazyLock::new(|| {
        TURN_MODEL
            .define_op(
                "turn.cancel",
                DefineOpOptions::new(|state: TurnModelState, payload: &CancelTurnPayload| {
                    let (Some(turn_id), Some(_)) = (payload.turn_id, payload.target) else {
                        return state;
                    };
                    if turn_id < state.next_turn_id {
                        return state;
                    }
                    let next_turn_id = state.next_turn_id;
                    let mut cancelled_turn_ids = state.cancelled_turn_ids.clone();
                    cancelled_turn_ids.push(turn_id);
                    advance_turn_clock(state, next_turn_id, Some(cancelled_turn_ids))
                }),
            )
            .expect("turn.cancel must have one global definition")
    });
pub static END_TURN: LazyLock<DefinedOp<TurnModelState, EndTurnPayload>> = LazyLock::new(|| {
    TURN_MODEL
        .define_op(
            "turn.ended",
            DefineOpOptions::new(|mut state: TurnModelState, payload: &EndTurnPayload| {
                state.last_ended = Some(LastEndedTurn {
                    turn_id: payload.turn_id,
                    reason: payload.reason,
                    duration_ms: payload.duration_ms,
                });
                state
            }),
        )
        .expect("turn.ended must have one global definition")
});

/// Registers the turn model, its cross-model reducer, and persisted operations.
///
/// TypeScript performs this registration while evaluating `turnOps.ts`.
/// Rust statics are lazy, so the loop service must force the equivalent side
/// effects before the wire log is restored.
pub(crate) fn ensure_turn_wire_registered() {
    LazyLock::force(&TURN_MODEL);
    LazyLock::force(&PROMPT_TURN);
    LazyLock::force(&STEER_TURN);
    LazyLock::force(&CANCEL_TURN);
    LazyLock::force(&END_TURN);
}

fn observe_restored_turn_id(
    mut state: TurnModelState,
    payload: &ContextAppendLoopEventPayload,
) -> TurnModelState {
    let Some(turn_id) = event_turn_id(&payload.event) else {
        return state;
    };
    if turn_id >= state.next_turn_id {
        let next_turn_id = turn_id + 1;
        state = advance_turn_clock(state, next_turn_id, None);
    }
    if state
        .last_ended
        .as_ref()
        .is_some_and(|ended| turn_id > ended.turn_id)
    {
        state.last_ended = None;
    }
    state
}
fn advance_turn_clock(
    mut state: TurnModelState,
    mut next_turn_id: crate::agent::TurnId,
    cancelled_turn_ids: Option<Vec<crate::agent::TurnId>>,
) -> TurnModelState {
    let mut pending_cancellations = cancelled_turn_ids
        .unwrap_or_else(|| std::mem::take(&mut state.cancelled_turn_ids))
        .into_iter()
        .filter(|turn_id| *turn_id >= next_turn_id)
        .collect::<BTreeSet<_>>();
    while pending_cancellations.remove(&next_turn_id) {
        next_turn_id = next_turn_id + 1;
    }
    state.next_turn_id = next_turn_id;
    state.cancelled_turn_ids = pending_cancellations.into_iter().collect();
    state
}
fn event_turn_id(event: &LoopRecordedEvent) -> Option<crate::agent::TurnId> {
    match event {
        LoopRecordedEvent::StepBegin { turn_id, .. }
        | LoopRecordedEvent::StepEnd { turn_id, .. }
        | LoopRecordedEvent::ContentPart { turn_id, .. }
        | LoopRecordedEvent::ToolCall { turn_id, .. } => *turn_id,
        LoopRecordedEvent::ToolResult { .. } => None,
    }
}
pub fn prompt_turn(seed: TurnSeed) -> Result<Op, serde_json::Error> {
    PROMPT_TURN.create(TurnInputPayload {
        input: seed.input,
        origin: seed.origin,
    })
}
pub fn steer_turn(seed: TurnSeed) -> Result<Op, serde_json::Error> {
    STEER_TURN.create(TurnInputPayload {
        input: seed.input,
        origin: seed.origin,
    })
}
pub fn cancel_turn(payload: CancelTurnPayload) -> Result<Op, serde_json::Error> {
    CANCEL_TURN.create(payload)
}
pub fn end_turn(payload: EndTurnPayload) -> Result<Op, serde_json::Error> {
    END_TURN.create(payload)
}

#[cfg(test)]
mod tests {
    use crate::wire::{model::model_cross_reducers, op::registered_op};

    use super::*;

    #[test]
    fn registration_covers_every_persisted_turn_operation() {
        ensure_turn_wire_registered();

        for op_type in ["turn.prompt", "turn.steer", "turn.cancel", "turn.ended"] {
            assert!(
                registered_op(op_type).is_some(),
                "{op_type} must be registered before wire restore"
            );
        }
        assert!(
            model_cross_reducers("context.append_loop_event")
                .iter()
                .any(|reducer| reducer.model.id() == TURN_MODEL.id()),
            "context loop events must restore the turn counter"
        );
    }

    #[test]
    fn prompt_increments_and_replayed_events_advance_without_tool_results() {
        let state = TurnModelState {
            next_turn_id: crate::agent::TurnId::new(2),
            cancelled_turn_ids: Vec::new(),
            last_ended: None,
        };
        let event = ContextAppendLoopEventPayload {
            event: LoopRecordedEvent::StepBegin {
                uuid: "x".into(),
                turn_id: Some(crate::agent::TurnId::new(3)),
                step: None,
            },
        };
        assert_eq!(
            observe_restored_turn_id(state, &event).next_turn_id,
            crate::agent::TurnId::new(4)
        );
        assert_eq!(
            PROMPT_TURN
                .create(TurnInputPayload {
                    input: vec![],
                    origin: PromptOrigin::User
                })
                .unwrap()
                .op_type,
            "turn.prompt"
        );
        assert_eq!(
            cancel_turn(CancelTurnPayload {
                turn_id: Some(crate::agent::TurnId::new(2)),
                target: Some(CancelTurnTarget::Queued),
                reason: Some(CancelTurnReason::UserCancelled),
            })
            .unwrap()
            .payload_value,
            serde_json::json!({
                "turnId": 2,
                "target": "queued",
                "reason": "user_cancelled"
            })
        );
    }

    #[test]
    fn legacy_cancel_payload_accepts_float_and_prefixed_turn_ids() {
        for (value, expected) in [
            (serde_json::json!(2.9), 2),
            (serde_json::json!("t100"), 100),
        ] {
            let payload: CancelTurnPayload =
                serde_json::from_value(serde_json::json!({"turnId": value})).unwrap();
            assert_eq!(payload.turn_id, Some(crate::agent::TurnId::new(expected)));
        }
    }

    #[test]
    fn legacy_turn_ended_record_is_restorable_without_rewriting_history() {
        ensure_turn_wire_registered();
        let descriptor = registered_op("turn.ended").expect("compatibility op must be registered");
        let op = Op::from_wire(
            descriptor,
            serde_json::json!({
                "turnId": 0,
                "reason": "completed",
                "durationMs": 24620
            }),
        )
        .expect("historical turn.ended payload must remain readable");

        let state = Box::new(TurnModelState {
            next_turn_id: crate::agent::TurnId::new(0),
            cancelled_turn_ids: Vec::new(),
            last_ended: None,
        });
        let restored = op
            .descriptor
            .apply(state, op.payload())
            .expect("compatibility op must apply")
            .downcast::<TurnModelState>()
            .expect("compatibility op must retain turn model state");
        assert_eq!(restored.next_turn_id, crate::agent::TurnId::new(0));
        assert_eq!(
            restored.last_ended,
            Some(LastEndedTurn {
                turn_id: crate::agent::TurnId::new(0),
                reason: TurnEndReason::Completed,
                duration_ms: Some(24620),
            })
        );
        assert_eq!(op.descriptor.persist(), None);
    }
}
