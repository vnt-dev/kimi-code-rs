use std::sync::{Arc, LazyLock};

use serde::{Deserialize, Serialize};

use crate::wire::{
    model::{ModelDef, ModelOptions, define_model},
    op::{DefineOpOptions, DefinedOp, Op},
};

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct PlanState {
    pub active: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PlanModeEnterPayload {
    pub id: String,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct PlanModeEndPayload {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
}

pub static PLAN_MODEL: LazyLock<ModelDef<PlanState>> =
    LazyLock::new(|| define_model("plan", PlanState::default, ModelOptions::default()));

pub static PLAN_MODE_ENTER: LazyLock<DefinedOp<PlanState, PlanModeEnterPayload>> =
    LazyLock::new(|| {
        let mut options = DefineOpOptions::new(apply_plan_mode_enter);
        options.to_event = Some(Arc::new(|_payload, _state| {
            Some(serde_json::json!({
                "type": "agent.status.updated",
                "planMode": true,
            }))
        }));
        PLAN_MODEL
            .define_op("plan_mode.enter", options)
            .expect("plan_mode.enter must have one global definition")
    });

pub static PLAN_MODE_CANCEL: LazyLock<DefinedOp<PlanState, PlanModeEndPayload>> =
    LazyLock::new(|| define_end_op("plan_mode.cancel"));

pub static PLAN_MODE_EXIT: LazyLock<DefinedOp<PlanState, PlanModeEndPayload>> =
    LazyLock::new(|| define_end_op("plan_mode.exit"));

fn define_end_op(op_type: &'static str) -> DefinedOp<PlanState, PlanModeEndPayload> {
    let mut options = DefineOpOptions::new(apply_plan_mode_end);
    options.to_event = Some(Arc::new(|_payload, _state| {
        Some(serde_json::json!({
            "type": "agent.status.updated",
            "planMode": false,
        }))
    }));
    PLAN_MODEL
        .define_op(op_type, options)
        .expect("plan end op must have one global definition")
}

// Original: planOps.ts, planModeEnter.apply().
fn apply_plan_mode_enter(mut state: PlanState, payload: &PlanModeEnterPayload) -> PlanState {
    if state.active && state.id.as_deref() == Some(payload.id.as_str()) {
        return state;
    }
    state.active = true;
    state.id = Some(payload.id.clone());
    state
}

// Original: planOps.ts, planModeCancel.apply() and planModeExit.apply(). The
// optional payload id is intentionally ignored by both source reducers.
fn apply_plan_mode_end(mut state: PlanState, _payload: &PlanModeEndPayload) -> PlanState {
    if !state.active {
        return state;
    }
    state.active = false;
    state.id = None;
    state
}

pub fn plan_mode_enter(id: impl Into<String>) -> Result<Op, serde_json::Error> {
    PLAN_MODE_ENTER.create(PlanModeEnterPayload { id: id.into() })
}

pub fn plan_mode_cancel(id: Option<String>) -> Result<Op, serde_json::Error> {
    PLAN_MODE_CANCEL.create(PlanModeEndPayload { id })
}

pub fn plan_mode_exit(id: Option<String>) -> Result<Op, serde_json::Error> {
    PLAN_MODE_EXIT.create(PlanModeEndPayload { id })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::op::ErasedOpDescriptor;

    #[test]
    fn model_and_persisted_payloads_match_source() {
        assert_eq!(PLAN_MODEL.name(), "plan");
        assert_eq!(PLAN_MODEL.initial(), PlanState::default());
        assert_eq!(PLAN_MODE_ENTER.op_type(), "plan_mode.enter");
        assert_eq!(PLAN_MODE_CANCEL.op_type(), "plan_mode.cancel");
        assert_eq!(PLAN_MODE_EXIT.op_type(), "plan_mode.exit");
        assert_eq!(
            plan_mode_enter("plan-1").unwrap().payload_value,
            serde_json::json!({"id": "plan-1"})
        );
        assert_eq!(
            plan_mode_cancel(None).unwrap().payload_value,
            serde_json::json!({})
        );
        assert_eq!(
            plan_mode_exit(Some("legacy-id".into()))
                .unwrap()
                .payload_value,
            serde_json::json!({"id": "legacy-id"})
        );
    }

    #[test]
    fn enter_replaces_different_plan_and_same_plan_is_a_state_noop() {
        let active = apply_plan_mode_enter(
            PlanState::default(),
            &PlanModeEnterPayload { id: "a".into() },
        );
        assert_eq!(
            active,
            PlanState {
                active: true,
                id: Some("a".into())
            }
        );
        let same = apply_plan_mode_enter(active.clone(), &PlanModeEnterPayload { id: "a".into() });
        assert_eq!(same, active);
        let replaced = apply_plan_mode_enter(same, &PlanModeEnterPayload { id: "b".into() });
        assert_eq!(replaced.id.as_deref(), Some("b"));
    }

    #[test]
    fn cancel_and_exit_ignore_ids_clear_active_plan_and_keep_inactive_state() {
        let active = PlanState {
            active: true,
            id: Some("current".into()),
        };
        let cancelled = apply_plan_mode_end(
            active,
            &PlanModeEndPayload {
                id: Some("different".into()),
            },
        );
        assert_eq!(cancelled, PlanState::default());
        assert_eq!(
            apply_plan_mode_end(
                cancelled,
                &PlanModeEndPayload {
                    id: Some("ignored".into())
                }
            ),
            PlanState::default()
        );
    }

    #[test]
    fn every_op_projects_the_status_event_even_for_state_noops() {
        let enter = plan_mode_enter("same").unwrap();
        assert_eq!(
            PLAN_MODE_ENTER
                .descriptor()
                .to_event(
                    enter.payload(),
                    &PlanState {
                        active: true,
                        id: Some("same".into())
                    }
                )
                .unwrap(),
            Some(serde_json::json!({
                "type": "agent.status.updated",
                "planMode": true
            }))
        );
        for (descriptor, op) in [
            (&*PLAN_MODE_CANCEL, plan_mode_cancel(None).unwrap()),
            (&*PLAN_MODE_EXIT, plan_mode_exit(None).unwrap()),
        ] {
            assert_eq!(
                descriptor
                    .descriptor()
                    .to_event(op.payload(), &PlanState::default())
                    .unwrap(),
                Some(serde_json::json!({
                    "type": "agent.status.updated",
                    "planMode": false
                }))
            );
        }
    }
}
