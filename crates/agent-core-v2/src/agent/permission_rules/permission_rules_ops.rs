use std::sync::LazyLock;

use serde::{Deserialize, Serialize};

use crate::{
    session::approval::{ApprovalDecision, ApprovalScope},
    wire::{
        model::{ModelDef, ModelOptions, define_model},
        op::{DefineOpOptions, DefinedOp, Op},
    },
};

use super::types::{PermissionApprovalResultRecord, PermissionRule};

// Original:
//   packages/agent-core-v2/src/agent/permissionRules/permissionRulesOps.ts
//   PermissionRulesModelState / PermissionRulesModel
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PermissionRulesModelState {
    pub rules: Vec<PermissionRule>,
    pub session_approval_rule_patterns: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct AddPermissionRulesPayload {
    pub rules: Vec<PermissionRule>,
}

pub static PERMISSION_RULES_MODEL: LazyLock<ModelDef<PermissionRulesModelState>> =
    LazyLock::new(|| {
        define_model(
            "permissionRules",
            PermissionRulesModelState::default,
            ModelOptions::default(),
        )
    });

pub static ADD_PERMISSION_RULES: LazyLock<
    DefinedOp<PermissionRulesModelState, AddPermissionRulesPayload>,
> = LazyLock::new(|| {
    let mut options = DefineOpOptions::new(apply_add_permission_rules);
    options.persist = Some(false);
    PERMISSION_RULES_MODEL
        .define_op("permission.rules.add", options)
        .expect("permission.rules.add must have one global definition")
});

pub static RECORD_APPROVAL_RESULT: LazyLock<
    DefinedOp<PermissionRulesModelState, PermissionApprovalResultRecord>,
> = LazyLock::new(|| {
    PERMISSION_RULES_MODEL
        .define_op(
            "permission.record_approval_result",
            DefineOpOptions::new(apply_approval_result),
        )
        .expect("permission.record_approval_result must have one global definition")
});

// Original: permissionRulesOps.ts, addPermissionRules.apply().
fn apply_add_permission_rules(
    mut state: PermissionRulesModelState,
    payload: &AddPermissionRulesPayload,
) -> PermissionRulesModelState {
    if !payload.rules.is_empty() {
        state.rules.extend(payload.rules.iter().cloned());
    }
    state
}

// Original: permissionRulesOps.ts, recordApprovalResult.apply().
fn apply_approval_result(
    mut state: PermissionRulesModelState,
    payload: &PermissionApprovalResultRecord,
) -> PermissionRulesModelState {
    let Some(pattern) = payload.session_approval_rule.as_ref() else {
        return state;
    };
    if payload.result.decision != ApprovalDecision::Approved
        || payload.result.scope != Some(ApprovalScope::Session)
        || state
            .session_approval_rule_patterns
            .iter()
            .any(|existing| existing == pattern)
    {
        return state;
    }
    state.session_approval_rule_patterns.push(pattern.clone());
    state
}

pub fn add_permission_rules(rules: Vec<PermissionRule>) -> Result<Op, serde_json::Error> {
    ADD_PERMISSION_RULES.create(AddPermissionRulesPayload { rules })
}

pub fn record_approval_result(
    record: PermissionApprovalResultRecord,
) -> Result<Op, serde_json::Error> {
    RECORD_APPROVAL_RESULT.create(record)
}

#[cfg(test)]
mod tests {
    use crate::{
        agent::permission_rules::types::{PermissionRuleDecision, PermissionRuleScope},
        session::approval::ApprovalResponse,
        wire::op::ErasedOpDescriptor,
    };

    use super::*;

    fn rule(pattern: &str) -> PermissionRule {
        PermissionRule {
            decision: PermissionRuleDecision::Allow,
            scope: PermissionRuleScope::User,
            pattern: pattern.into(),
            reason: None,
        }
    }

    fn approval(
        decision: ApprovalDecision,
        scope: Option<ApprovalScope>,
        pattern: Option<&str>,
    ) -> PermissionApprovalResultRecord {
        PermissionApprovalResultRecord {
            turn_id: crate::agent::TurnId::new(3),
            tool_call_id: "call-1".into(),
            tool_name: "Bash".into(),
            action: "run".into(),
            session_approval_rule: pattern.map(str::to_owned),
            result: ApprovalResponse {
                decision,
                scope,
                feedback: None,
                selected_label: None,
            },
        }
    }

    #[test]
    fn model_starts_empty_and_add_op_is_transient() {
        assert_eq!(PERMISSION_RULES_MODEL.name(), "permissionRules");
        assert_eq!(
            PERMISSION_RULES_MODEL.initial(),
            PermissionRulesModelState::default()
        );
        assert_eq!(ADD_PERMISSION_RULES.descriptor().persist(), Some(false));
        assert_eq!(RECORD_APPROVAL_RESULT.descriptor().persist(), None);
    }

    #[test]
    fn add_rules_appends_in_order_and_empty_payload_is_unchanged() {
        let initial = PermissionRulesModelState {
            rules: vec![rule("Read")],
            ..PermissionRulesModelState::default()
        };
        assert_eq!(
            apply_add_permission_rules(
                initial.clone(),
                &AddPermissionRulesPayload { rules: Vec::new() }
            ),
            initial
        );
        assert_eq!(
            apply_add_permission_rules(
                initial,
                &AddPermissionRulesPayload {
                    rules: vec![rule("Bash"), rule("Write")]
                }
            )
            .rules
            .iter()
            .map(|rule| rule.pattern.as_str())
            .collect::<Vec<_>>(),
            ["Read", "Bash", "Write"]
        );
    }

    #[test]
    fn approval_reducer_only_adds_unique_approved_session_patterns() {
        let approved = approval(
            ApprovalDecision::Approved,
            Some(ApprovalScope::Session),
            Some("Bash(git *)"),
        );
        let state = apply_approval_result(PermissionRulesModelState::default(), &approved);
        assert_eq!(state.session_approval_rule_patterns, ["Bash(git *)"]);
        assert_eq!(apply_approval_result(state.clone(), &approved), state);

        for ignored in [
            approval(
                ApprovalDecision::Rejected,
                Some(ApprovalScope::Session),
                Some("Read(*)"),
            ),
            approval(ApprovalDecision::Approved, None, Some("Read(*)")),
            approval(
                ApprovalDecision::Approved,
                Some(ApprovalScope::Session),
                None,
            ),
        ] {
            assert_eq!(apply_approval_result(state.clone(), &ignored), state);
        }
    }

    #[test]
    fn op_creators_preserve_wire_names_and_camel_case_payload() {
        let add = add_permission_rules(vec![rule("Read")]).unwrap();
        assert_eq!(add.op_type, "permission.rules.add");
        assert_eq!(add.payload_value["rules"][0]["pattern"], "Read");

        let record = record_approval_result(approval(
            ApprovalDecision::Approved,
            Some(ApprovalScope::Session),
            Some("Bash(git *)"),
        ))
        .unwrap();
        assert_eq!(record.op_type, "permission.record_approval_result");
        assert_eq!(record.payload_value["turnId"], 3);
        assert_eq!(record.payload_value["toolCallId"], "call-1");
        assert_eq!(record.payload_value["sessionApprovalRule"], "Bash(git *)");
        assert_eq!(record.payload_value["result"]["decision"], "approved");
    }

    #[test]
    fn legacy_approval_payload_accepts_float_turn_id() {
        let mut value = serde_json::to_value(approval(
            ApprovalDecision::Approved,
            Some(ApprovalScope::Session),
            Some("Bash(git *)"),
        ))
        .unwrap();
        value["turnId"] = serde_json::json!(3.9);

        let record: PermissionApprovalResultRecord = serde_json::from_value(value).unwrap();
        assert_eq!(record.turn_id, crate::agent::TurnId::new(3));
    }
}
