use std::sync::{Arc, LazyLock};

use serde::{Deserialize, Serialize};

use crate::wire::{
    model::{ModelDef, ModelOptions, define_model},
    op::{DefineOpOptions, DefinedOp, Op},
};

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
pub struct ContextSizeState {
    #[serde(deserialize_with = "kimi_code_protocol::lenient::lenient_u64")]
    pub length: u64,
    #[serde(deserialize_with = "kimi_code_protocol::lenient::lenient_u64")]
    pub tokens: u64,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
pub struct ContextSizeMeasuredPayload {
    #[serde(deserialize_with = "kimi_code_protocol::lenient::lenient_u64")]
    pub length: u64,
    #[serde(deserialize_with = "kimi_code_protocol::lenient::lenient_u64")]
    pub tokens: u64,
}

pub static CONTEXT_SIZE_MODEL: LazyLock<ModelDef<ContextSizeState>> = LazyLock::new(|| {
    define_model(
        "contextSize",
        || ContextSizeState {
            length: 0,
            tokens: 0,
        },
        ModelOptions::default(),
    )
});

pub static CONTEXT_SIZE_MEASURED: LazyLock<
    DefinedOp<ContextSizeState, ContextSizeMeasuredPayload>,
> = LazyLock::new(|| {
    let mut options = DefineOpOptions::new(apply_context_size_measured);
    options.persist = Some(false);
    options.to_event = Some(Arc::new(|_payload, state| {
        Some(serde_json::json!({
            "type": "agent.status.updated",
            "contextTokens": state.tokens,
        }))
    }));
    CONTEXT_SIZE_MODEL
        .define_op("context_size.measured", options)
        .expect("context_size.measured must have one global definition")
});

// Original:
//   packages/agent-core-v2/src/agent/contextSize/contextSizeOps.ts
//   contextSizeMeasured.apply()
//
// The original reducer returns its existing object for equal normalized
// values. Its current WireService still invokes toEvent for that result, so the
// Rust Op preserves the effective event behavior rather than the contradictory
// source comment about a reference-equality gate.
fn apply_context_size_measured(
    state: ContextSizeState,
    payload: &ContextSizeMeasuredPayload,
) -> ContextSizeState {
    if state.length == payload.length && state.tokens == payload.tokens {
        return state;
    }
    ContextSizeState {
        length: payload.length,
        tokens: payload.tokens,
    }
}

pub fn context_size_measured(payload: ContextSizeMeasuredPayload) -> Result<Op, serde_json::Error> {
    CONTEXT_SIZE_MEASURED.create(payload)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::op::ErasedOpDescriptor;

    #[test]
    fn model_starts_empty_and_op_is_transient() {
        assert_eq!(
            CONTEXT_SIZE_MODEL.initial(),
            ContextSizeState {
                length: 0,
                tokens: 0
            }
        );
        assert_eq!(CONTEXT_SIZE_MODEL.name(), "contextSize");
        assert_eq!(CONTEXT_SIZE_MEASURED.op_type(), "context_size.measured");
        assert_eq!(CONTEXT_SIZE_MEASURED.descriptor().persist(), Some(false));
    }

    #[test]
    fn reducer_passes_values_through_and_deduplicates_equal_state() {
        assert_eq!(
            apply_context_size_measured(
                ContextSizeState {
                    length: 1,
                    tokens: 2,
                },
                &ContextSizeMeasuredPayload {
                    length: 4,
                    tokens: 7,
                },
            ),
            ContextSizeState {
                length: 4,
                tokens: 7,
            }
        );
        let state = ContextSizeState {
            length: 4,
            tokens: 7,
        };
        assert_eq!(
            apply_context_size_measured(
                state,
                &ContextSizeMeasuredPayload {
                    length: 4,
                    tokens: 7,
                },
            ),
            state
        );
    }

    #[test]
    fn op_projects_agent_status_event() {
        let op = context_size_measured(ContextSizeMeasuredPayload {
            length: 2,
            tokens: 17,
        })
        .unwrap();
        let event = CONTEXT_SIZE_MEASURED
            .descriptor()
            .to_event(
                op.payload(),
                &ContextSizeState {
                    length: 2,
                    tokens: 17,
                },
            )
            .unwrap();
        assert_eq!(
            event,
            Some(serde_json::json!({
                "type": "agent.status.updated",
                "contextTokens": 17,
            }))
        );
    }
}
