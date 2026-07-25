//! Durable turn counter and turn lifecycle wire operations.
//!
//! Original: `packages/agent-core-v2/src/agent/loop/turnOps.ts`.

use std::sync::LazyLock;

use serde::{Deserialize, Serialize};

use crate::{
    agent::{
        context_memory::{ContextAppendLoopEventPayload, LoopRecordedEvent, PromptOrigin},
        loop_::TurnSeed,
    },
    kosong::contract::message::ContentPart,
    wire::{
        model::{ModelCrossReducer, ModelDef, ModelOptions, define_model},
        op::{DefineOpOptions, DefinedOp, Op},
    },
};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnModelState {
    pub next_turn_id: i64,
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
    pub turn_id: Option<f64>,
}

pub static TURN_MODEL: LazyLock<ModelDef<TurnModelState>> = LazyLock::new(|| {
    define_model(
        "turn",
        || TurnModelState { next_turn_id: 0 },
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
                    TurnModelState {
                        next_turn_id: state.next_turn_id + 1,
                    }
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
                DefineOpOptions::new(|state: TurnModelState, _: &CancelTurnPayload| state),
            )
            .expect("turn.cancel must have one global definition")
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
}

fn observe_restored_turn_id(
    state: TurnModelState,
    payload: &ContextAppendLoopEventPayload,
) -> TurnModelState {
    let Some(turn_id) = event_turn_id(&payload.event) else {
        return state;
    };
    let Some(turn_id) = javascript_parse_int(turn_id) else {
        return state;
    };
    if turn_id >= state.next_turn_id {
        TurnModelState {
            next_turn_id: turn_id + 1,
        }
    } else {
        state
    }
}
fn event_turn_id(event: &LoopRecordedEvent) -> Option<&str> {
    match event {
        LoopRecordedEvent::StepBegin { turn_id, .. }
        | LoopRecordedEvent::StepEnd { turn_id, .. }
        | LoopRecordedEvent::ContentPart { turn_id, .. }
        | LoopRecordedEvent::ToolCall { turn_id, .. } => turn_id.as_deref(),
        LoopRecordedEvent::ToolResult { .. } => None,
    }
}
fn javascript_parse_int(value: &str) -> Option<i64> {
    let value = value.trim_start();
    let end = value
        .char_indices()
        .take_while(|(index, character)| {
            *index == 0 && (*character == '+' || *character == '-') || character.is_ascii_digit()
        })
        .last()
        .map_or(0, |(index, character)| index + character.len_utf8());
    if end == 0 || matches!(value.as_bytes().first(), Some(b'+') | Some(b'-')) && end == 1 {
        None
    } else {
        value[..end].parse().ok()
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
pub fn cancel_turn(turn_id: Option<f64>) -> Result<Op, serde_json::Error> {
    CANCEL_TURN.create(CancelTurnPayload { turn_id })
}

#[cfg(test)]
mod tests {
    use crate::wire::{model::model_cross_reducers, op::registered_op};

    use super::*;

    #[test]
    fn registration_covers_every_persisted_turn_operation() {
        ensure_turn_wire_registered();

        for op_type in ["turn.prompt", "turn.steer", "turn.cancel"] {
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
        let state = TurnModelState { next_turn_id: 2 };
        let event = ContextAppendLoopEventPayload {
            event: LoopRecordedEvent::StepBegin {
                uuid: "x".into(),
                turn_id: Some("3.8".into()),
                step: None,
            },
        };
        assert_eq!(observe_restored_turn_id(state, &event).next_turn_id, 4);
        assert_eq!(javascript_parse_int(" -12x"), Some(-12));
        assert_eq!(javascript_parse_int("x"), None);
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
            cancel_turn(Some(2.0)).unwrap().payload_value,
            serde_json::json!({"turnId": 2.0})
        );
    }
}
