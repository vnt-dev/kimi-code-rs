use std::sync::LazyLock;

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

use crate::{
    kosong::contract::usage::{TokenUsage, add_usage},
    wire::{
        model::{ModelDef, ModelOptions, define_model},
        op::{DefineOpOptions, DefinedOp, Op},
    },
};

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageStatus {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by_model: Option<IndexMap<String, TokenUsage>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total: Option<TokenUsage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_turn: Option<TokenUsage>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum UsageRecordScope {
    Session,
    Turn,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageModelState {
    pub by_model: IndexMap<String, TokenUsage>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordUsagePayload {
    pub model: String,
    pub usage: TokenUsage,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage_scope: Option<UsageRecordScope>,
}

pub static USAGE_MODEL: LazyLock<ModelDef<UsageModelState>> =
    LazyLock::new(|| define_model("usage", UsageModelState::default, ModelOptions::default()));

pub static RECORD_USAGE: LazyLock<DefinedOp<UsageModelState, RecordUsagePayload>> =
    LazyLock::new(|| {
        USAGE_MODEL
            .define_op("usage.record", DefineOpOptions::new(apply_record_usage))
            .expect("usage.record must have one global definition")
    });

// Original:
//   packages/agent-core-v2/src/agent/usage/usageOps.ts
//   recordUsage.apply()
fn apply_record_usage(mut state: UsageModelState, payload: &RecordUsagePayload) -> UsageModelState {
    let usage = state
        .by_model
        .get(&payload.model)
        .map(|current| add_usage(current, &payload.usage))
        .unwrap_or(payload.usage);
    state.by_model.insert(payload.model.clone(), usage);
    state
}

pub fn record_usage(payload: RecordUsagePayload) -> Result<Op, serde_json::Error> {
    RECORD_USAGE.create(payload)
}

// Original: usageOps.ts, copyUsage(). TokenUsage is Copy in Rust.
pub fn copy_usage(usage: &TokenUsage) -> TokenUsage {
    *usage
}

// Original: usageOps.ts, usageStatusFromState(). Every nested usage is copied
// into a defensive snapshot and totals follow model insertion order.
pub fn usage_status_from_state(
    model: &UsageModelState,
    current_turn: Option<&TokenUsage>,
) -> UsageStatus {
    if model.by_model.is_empty() {
        return UsageStatus {
            current_turn: current_turn.copied(),
            ..UsageStatus::default()
        };
    }
    let by_model = model.by_model.clone();
    let total = total_usage(&by_model);
    UsageStatus {
        by_model: Some(by_model),
        total,
        current_turn: current_turn.copied(),
    }
}

fn total_usage(by_model: &IndexMap<String, TokenUsage>) -> Option<TokenUsage> {
    by_model
        .values()
        .copied()
        .reduce(|total, usage| add_usage(&total, &usage))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn usage(value: u64) -> TokenUsage {
        TokenUsage {
            input_other: value,
            output: value * 2,
            input_cache_read: value * 3,
            input_cache_creation: value * 4,
        }
    }

    #[test]
    fn model_and_record_wire_shape_match_source() {
        assert_eq!(USAGE_MODEL.name(), "usage");
        assert!(USAGE_MODEL.initial().by_model.is_empty());
        assert_eq!(RECORD_USAGE.op_type(), "usage.record");
        let op = record_usage(RecordUsagePayload {
            model: "kimi".into(),
            usage: usage(1),
            usage_scope: Some(UsageRecordScope::Turn),
        })
        .unwrap();
        assert_eq!(op.payload_value["model"], "kimi");
        assert_eq!(op.payload_value["usageScope"], "turn");
    }

    #[test]
    fn reducer_copies_first_usage_and_accumulates_per_model() {
        let first = apply_record_usage(
            UsageModelState::default(),
            &RecordUsagePayload {
                model: "a".into(),
                usage: usage(1),
                usage_scope: None,
            },
        );
        let second = apply_record_usage(
            first,
            &RecordUsagePayload {
                model: "a".into(),
                usage: usage(2),
                usage_scope: Some(UsageRecordScope::Session),
            },
        );
        assert_eq!(second.by_model["a"], usage(3));
    }

    #[test]
    fn status_returns_empty_or_defensive_totals_and_current_turn() {
        let current = usage(4);
        assert_eq!(
            usage_status_from_state(&UsageModelState::default(), Some(&current)),
            UsageStatus {
                by_model: None,
                total: None,
                current_turn: Some(current),
            }
        );
        let model = UsageModelState {
            by_model: IndexMap::from([("a".into(), usage(1)), ("b".into(), usage(2))]),
        };
        let mut status = usage_status_from_state(&model, None);
        assert_eq!(status.total, Some(usage(3)));
        status.by_model.as_mut().unwrap().clear();
        assert_eq!(model.by_model.len(), 2);
    }
}
