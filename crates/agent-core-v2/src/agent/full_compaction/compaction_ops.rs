use std::sync::{Arc, LazyLock};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::wire::{
    model::{ModelDef, ModelOptions, define_model},
    op::{DefineOpOptions, DefinedOp, Op},
};

use super::types::CompactionBeginData;

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CompactionPhase {
    #[default]
    Idle,
    Running,
    Cancelled,
    Completed,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct CompactionState {
    pub phase: CompactionPhase,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct EmptyCompactionPayload {}

pub static COMPACTION_MODEL: LazyLock<ModelDef<CompactionState>> = LazyLock::new(|| {
    define_model(
        "fullCompaction",
        CompactionState::default,
        ModelOptions::default(),
    )
});

pub static FULL_COMPACTION_BEGIN: LazyLock<DefinedOp<CompactionState, CompactionBeginData>> =
    LazyLock::new(|| {
        let mut options = DefineOpOptions::new(apply_begin);
        options.to_event = Some(Arc::new(|payload, _state| {
            let mut fields = Map::from_iter([
                ("type".into(), Value::String("compaction.started".into())),
                (
                    "trigger".into(),
                    serde_json::to_value(payload.source)
                        .expect("CompactionSource is always JSON serializable"),
                ),
            ]);
            if let Some(instruction) = &payload.instruction {
                fields.insert("instruction".into(), Value::String(instruction.clone()));
            }
            Some(Value::Object(fields))
        }));
        COMPACTION_MODEL
            .define_op("full_compaction.begin", options)
            .expect("full_compaction.begin must have one global definition")
    });

pub static FULL_COMPACTION_CANCEL: LazyLock<DefinedOp<CompactionState, EmptyCompactionPayload>> =
    LazyLock::new(|| define_end_op("full_compaction.cancel"));

pub static FULL_COMPACTION_COMPLETE: LazyLock<DefinedOp<CompactionState, EmptyCompactionPayload>> =
    LazyLock::new(|| define_end_op("full_compaction.complete"));

fn define_end_op(op_type: &'static str) -> DefinedOp<CompactionState, EmptyCompactionPayload> {
    COMPACTION_MODEL
        .define_op(op_type, DefineOpOptions::new(apply_end))
        .expect("full compaction end op must have one global definition")
}

// Original: compactionOps.ts, fullCompactionBegin.apply().
fn apply_begin(mut state: CompactionState, _payload: &CompactionBeginData) -> CompactionState {
    if state.phase != CompactionPhase::Running {
        state.phase = CompactionPhase::Running;
    }
    state
}

// Original: compactionOps.ts, cancel/complete apply(). Both collapse every
// non-idle phase to idle and intentionally ignore legacy result fields.
fn apply_end(mut state: CompactionState, _payload: &EmptyCompactionPayload) -> CompactionState {
    if state.phase != CompactionPhase::Idle {
        state.phase = CompactionPhase::Idle;
    }
    state
}

pub fn full_compaction_begin(payload: CompactionBeginData) -> Result<Op, serde_json::Error> {
    FULL_COMPACTION_BEGIN.create(payload)
}

pub fn full_compaction_cancel() -> Result<Op, serde_json::Error> {
    FULL_COMPACTION_CANCEL.create(EmptyCompactionPayload {})
}

pub fn full_compaction_complete() -> Result<Op, serde_json::Error> {
    FULL_COMPACTION_COMPLETE.create(EmptyCompactionPayload {})
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        agent::full_compaction::CompactionSource,
        wire::op::{ErasedOpDescriptor, Op, registered_op},
    };

    #[test]
    fn model_and_wire_payloads_match_source() {
        assert_eq!(COMPACTION_MODEL.name(), "fullCompaction");
        assert_eq!(COMPACTION_MODEL.initial(), CompactionState::default());
        assert_eq!(FULL_COMPACTION_BEGIN.op_type(), "full_compaction.begin");
        assert_eq!(FULL_COMPACTION_CANCEL.op_type(), "full_compaction.cancel");
        assert_eq!(
            FULL_COMPACTION_COMPLETE.op_type(),
            "full_compaction.complete"
        );
        assert_eq!(
            full_compaction_begin(CompactionBeginData {
                source: CompactionSource::Manual,
                instruction: Some("focus".into())
            })
            .unwrap()
            .payload_value,
            serde_json::json!({"source": "manual", "instruction": "focus"})
        );
        assert_eq!(
            full_compaction_cancel().unwrap().payload_value,
            serde_json::json!({})
        );
        assert_eq!(
            full_compaction_complete().unwrap().payload_value,
            serde_json::json!({})
        );
    }

    #[test]
    fn phase_reducer_enters_running_and_both_end_ops_return_idle() {
        let begin = CompactionBeginData {
            source: CompactionSource::Auto,
            instruction: None,
        };
        let running = apply_begin(CompactionState::default(), &begin);
        assert_eq!(running.phase, CompactionPhase::Running);
        assert_eq!(apply_begin(running, &begin).phase, CompactionPhase::Running);
        for phase in [
            CompactionPhase::Running,
            CompactionPhase::Cancelled,
            CompactionPhase::Completed,
            CompactionPhase::Idle,
        ] {
            assert_eq!(
                apply_end(CompactionState { phase }, &EmptyCompactionPayload {}).phase,
                CompactionPhase::Idle
            );
        }
    }

    #[test]
    fn begin_event_omits_absent_instruction() {
        let op = full_compaction_begin(CompactionBeginData {
            source: CompactionSource::Auto,
            instruction: None,
        })
        .unwrap();
        assert_eq!(
            FULL_COMPACTION_BEGIN
                .descriptor()
                .to_event(
                    op.payload(),
                    &CompactionState {
                        phase: CompactionPhase::Running
                    }
                )
                .unwrap(),
            Some(serde_json::json!({
                "type": "compaction.started",
                "trigger": "auto"
            }))
        );
    }

    #[test]
    fn replay_accepts_and_ignores_legacy_complete_result_fields() {
        LazyLock::force(&FULL_COMPACTION_COMPLETE);
        let descriptor = registered_op("full_compaction.complete").unwrap();
        let replay = Op::from_wire(
            descriptor.clone(),
            serde_json::json!({"tokensBefore": 100, "tokensAfter": 20}),
        )
        .unwrap();
        let state = descriptor
            .apply(
                Box::new(CompactionState {
                    phase: CompactionPhase::Running,
                }),
                replay.payload(),
            )
            .unwrap()
            .downcast::<CompactionState>()
            .unwrap();
        assert_eq!(state.phase, CompactionPhase::Idle);
    }
}
