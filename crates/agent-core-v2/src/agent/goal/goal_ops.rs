use std::sync::{Arc, LazyLock};

use serde::{Deserialize, Serialize, de::DeserializeOwned};

use crate::wire::{
    model::{ModelDef, ModelOptions, define_model},
    op::{DefineOpOptions, DefinedOp, Op},
};

use super::types::{GoalActor, GoalBudgetLimits, GoalStatus};

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GoalState {
    pub goal_id: String,
    pub objective: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completion_criterion: Option<String>,
    pub status: GoalStatus,
    #[serde(deserialize_with = "kimi_code_protocol::lenient::lenient_u64")]
    pub turns_used: u64,
    #[serde(deserialize_with = "kimi_code_protocol::lenient::lenient_u64")]
    pub tokens_used: u64,
    #[serde(deserialize_with = "kimi_code_protocol::lenient::lenient_u64")]
    pub wall_clock_ms: u64,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "kimi_code_protocol::lenient::lenient_nullable_i64"
    )]
    pub wall_clock_resumed_at: Option<i64>,
    pub budget_limits: GoalBudgetLimits,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal_reason: Option<String>,
}

pub type GoalModelState = Option<GoalState>;

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateGoalPayload {
    pub goal_id: String,
    pub objective: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completion_criterion: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "kimi_code_protocol::lenient::lenient_optional_i64"
    )]
    pub wall_clock_resumed_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<GoalStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor: Option<GoalActor>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget_limits: Option<GoalBudgetLimits>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateGoalPayload {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub goal_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<GoalStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "kimi_code_protocol::lenient::lenient_optional_u64"
    )]
    pub turns_used: Option<u64>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "kimi_code_protocol::lenient::lenient_optional_u64"
    )]
    pub tokens_used: Option<u64>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "kimi_code_protocol::lenient::lenient_optional_u64"
    )]
    pub wall_clock_ms: Option<u64>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "kimi_code_protocol::lenient::lenient_optional_i64"
    )]
    pub wall_clock_resumed_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget_limits: Option<GoalBudgetLimits>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor: Option<GoalActor>,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct EmptyGoalPayload {}

pub static GOAL_MODEL: LazyLock<ModelDef<GoalModelState>> =
    LazyLock::new(|| define_model("goal", || None, ModelOptions::default()));

pub static CREATE_GOAL: LazyLock<DefinedOp<GoalModelState, CreateGoalPayload>> =
    LazyLock::new(|| {
        GOAL_MODEL
            .define_op(
                "goal.create",
                validated_options(apply_create, validate_create),
            )
            .expect("goal.create must have one global definition")
    });

pub static UPDATE_GOAL: LazyLock<DefinedOp<GoalModelState, UpdateGoalPayload>> =
    LazyLock::new(|| {
        GOAL_MODEL
            .define_op(
                "goal.update",
                validated_options(apply_update, validate_update),
            )
            .expect("goal.update must have one global definition")
    });

pub static CLEAR_GOAL: LazyLock<DefinedOp<GoalModelState, EmptyGoalPayload>> =
    LazyLock::new(|| define_clear_op("goal.clear"));

pub static FORK_GOAL: LazyLock<DefinedOp<GoalModelState, EmptyGoalPayload>> =
    LazyLock::new(|| define_clear_op("forked"));

fn validated_options<P>(
    apply: impl Fn(GoalModelState, &P) -> GoalModelState + Send + Sync + 'static,
    validate: fn(&P) -> Result<(), String>,
) -> DefineOpOptions<GoalModelState, P>
where
    P: DeserializeOwned + Send + Sync + 'static,
{
    let mut options = DefineOpOptions::new(apply);
    options.parse_payload = Arc::new(move |value| {
        let payload: P =
            serde_json::from_value(value.clone()).map_err(|error| error.to_string())?;
        validate(&payload)?;
        Ok(payload)
    });
    options
}

fn define_clear_op(op_type: &'static str) -> DefinedOp<GoalModelState, EmptyGoalPayload> {
    GOAL_MODEL
        .define_op(op_type, DefineOpOptions::new(|_state, _payload| None))
        .expect("goal clear op must have one global definition")
}

fn validate_nonnegative_timestamp(value: Option<i64>, field: &str) -> Result<(), String> {
    if value.is_some_and(|value| value < 0) {
        Err(format!("{field} must be a nonnegative integer"))
    } else {
        Ok(())
    }
}

// Counter and budget fields are u64, so serde already rejects negatives,
// NaN, and Infinity at deserialization time.
fn validate_create(payload: &CreateGoalPayload) -> Result<(), String> {
    validate_nonnegative_timestamp(payload.wall_clock_resumed_at, "wallClockResumedAt")
}

fn validate_update(payload: &UpdateGoalPayload) -> Result<(), String> {
    validate_nonnegative_timestamp(payload.wall_clock_resumed_at, "wallClockResumedAt")
}

// Original: goalOps.ts, createGoal.apply(). Audit-only status, actor, and
// budgetLimits payload fields are deliberately ignored.
fn apply_create(_state: GoalModelState, payload: &CreateGoalPayload) -> GoalModelState {
    Some(GoalState {
        goal_id: payload.goal_id.clone(),
        objective: payload.objective.clone(),
        completion_criterion: payload.completion_criterion.clone(),
        status: GoalStatus::Active,
        turns_used: 0,
        tokens_used: 0,
        wall_clock_ms: 0,
        wall_clock_resumed_at: payload.wall_clock_resumed_at,
        budget_limits: GoalBudgetLimits::default(),
        terminal_reason: None,
    })
}

// Original: goalOps.ts, updateGoal.apply().
fn apply_update(state: GoalModelState, payload: &UpdateGoalPayload) -> GoalModelState {
    let mut state = state?;
    if let Some(status) = payload.status.filter(|status| *status != state.status) {
        state.status = status;
        if status == GoalStatus::Active {
            state.terminal_reason = None;
            state.wall_clock_resumed_at = payload.wall_clock_resumed_at;
        } else {
            state.terminal_reason = payload.reason.clone();
            state.wall_clock_resumed_at = None;
        }
    }
    if let Some(turns_used) = payload
        .turns_used
        .filter(|value| *value != state.turns_used)
    {
        state.turns_used = turns_used;
    }
    if let Some(tokens_used) = payload
        .tokens_used
        .filter(|value| *value != state.tokens_used)
    {
        state.tokens_used = tokens_used;
    }
    if let Some(wall_clock_ms) = payload
        .wall_clock_ms
        .filter(|value| *value != state.wall_clock_ms)
    {
        state.wall_clock_ms = wall_clock_ms;
    }
    if let Some(resumed_at) = payload.wall_clock_resumed_at.filter(|value| {
        payload.status.unwrap_or(state.status) == GoalStatus::Active
            && Some(*value) != state.wall_clock_resumed_at
    }) {
        state.wall_clock_resumed_at = Some(resumed_at);
    }
    if let Some(budget_limits) = payload.budget_limits {
        state.budget_limits = budget_limits;
    }
    Some(state)
}

pub fn create_goal(payload: CreateGoalPayload) -> Result<Op, serde_json::Error> {
    CREATE_GOAL.create(payload)
}

pub fn update_goal(payload: UpdateGoalPayload) -> Result<Op, serde_json::Error> {
    UPDATE_GOAL.create(payload)
}

pub fn clear_goal() -> Result<Op, serde_json::Error> {
    CLEAR_GOAL.create(EmptyGoalPayload {})
}

pub fn fork_goal() -> Result<Op, serde_json::Error> {
    FORK_GOAL.create(EmptyGoalPayload {})
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::op::{Op, registered_op};

    fn create_payload() -> CreateGoalPayload {
        CreateGoalPayload {
            goal_id: "goal-1".into(),
            objective: "work".into(),
            completion_criterion: Some("done".into()),
            wall_clock_resumed_at: Some(1000),
            status: Some(GoalStatus::Blocked),
            actor: Some(GoalActor::System),
            budget_limits: Some(GoalBudgetLimits {
                token_budget: Some(50),
                ..GoalBudgetLimits::default()
            }),
        }
    }

    #[test]
    fn create_resets_state_and_ignores_audit_status_actor_and_budget() {
        let state = apply_create(None, &create_payload()).unwrap();
        assert_eq!(state.status, GoalStatus::Active);
        assert_eq!(state.turns_used, 0);
        assert_eq!(state.tokens_used, 0);
        assert_eq!(state.wall_clock_ms, 0);
        assert_eq!(state.wall_clock_resumed_at, Some(1000));
        assert_eq!(state.budget_limits, GoalBudgetLimits::default());
        assert_eq!(state.completion_criterion.as_deref(), Some("done"));
    }

    #[test]
    fn update_preserves_status_transition_and_counter_ordering_quirks() {
        let active = apply_create(None, &create_payload());
        let blocked = apply_update(
            active,
            &UpdateGoalPayload {
                goal_id: Some("different-is-audit-only".into()),
                status: Some(GoalStatus::Blocked),
                reason: Some("waiting".into()),
                turns_used: Some(2),
                tokens_used: Some(9),
                wall_clock_ms: Some(500),
                wall_clock_resumed_at: Some(2000),
                budget_limits: Some(GoalBudgetLimits {
                    turn_budget: Some(3),
                    ..GoalBudgetLimits::default()
                }),
                actor: Some(GoalActor::Runtime),
            },
        )
        .unwrap();
        assert_eq!(blocked.goal_id, "goal-1");
        assert_eq!(blocked.status, GoalStatus::Blocked);
        assert_eq!(blocked.terminal_reason.as_deref(), Some("waiting"));
        assert_eq!(blocked.wall_clock_resumed_at, None);
        assert_eq!(blocked.turns_used, 2);
        assert_eq!(blocked.tokens_used, 9);
        assert_eq!(blocked.wall_clock_ms, 500);
        assert_eq!(blocked.budget_limits.turn_budget, Some(3));

        let resumed = apply_update(
            Some(blocked),
            &UpdateGoalPayload {
                status: Some(GoalStatus::Active),
                wall_clock_resumed_at: Some(3000),
                reason: Some("ignored".into()),
                ..UpdateGoalPayload::default()
            },
        )
        .unwrap();
        assert_eq!(resumed.terminal_reason, None);
        assert_eq!(resumed.wall_clock_resumed_at, Some(3000));
    }

    #[test]
    fn update_without_goal_and_clear_or_fork_all_produce_none() {
        assert_eq!(apply_update(None, &UpdateGoalPayload::default()), None);
        assert_eq!(clear_goal().unwrap().payload_value, serde_json::json!({}));
        assert_eq!(fork_goal().unwrap().op_type, "forked");
    }

    #[test]
    fn replay_rejects_invalid_enums_negative_numbers_and_non_strict_budget() {
        LazyLock::force(&UPDATE_GOAL);
        let descriptor = registered_op("goal.update").unwrap();
        for invalid in [
            serde_json::json!({"status": "cancelled"}),
            serde_json::json!({"actor": "assistant"}),
            serde_json::json!({"turnsUsed": -1}),
            serde_json::json!({"tokensUsed": -0.1}),
            serde_json::json!({"budgetLimits": {"turnBudget": -1}}),
            serde_json::json!({"budgetLimits": {"turnBudget": 1, "extra": true}}),
        ] {
            assert!(Op::from_wire(descriptor.clone(), invalid).is_err());
        }
        assert!(Op::from_wire(descriptor, serde_json::json!({"unknown": true})).is_ok());
    }

    #[test]
    fn live_payloads_preserve_complete_camel_case_audit_shape() {
        let create = create_goal(create_payload()).unwrap().payload_value;
        assert_eq!(create["goalId"], "goal-1");
        assert_eq!(create["completionCriterion"], "done");
        assert_eq!(create["wallClockResumedAt"], 1000);
        assert_eq!(create["status"], "blocked");
        assert_eq!(create["actor"], "system");
        assert_eq!(create["budgetLimits"]["tokenBudget"], 50);
    }
}
