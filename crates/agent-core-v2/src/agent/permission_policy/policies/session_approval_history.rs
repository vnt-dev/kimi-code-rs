//! Reuses session-scoped approvals recorded by the permission-rules service.
//!
//! Original: `agent/permissionPolicy/policies/session-approval-history.ts`.

use std::sync::Arc;

use futures_util::FutureExt;

use crate::agent::{
    permission_policy::{
        PermissionDecisionReason, PermissionPolicy, PermissionPolicyFuture, PermissionPolicyResult,
        PermissionReasonValue,
    },
    permission_rules::{
        AgentPermissionRulesServiceContract, PermissionRule, PermissionRuleDecision,
        PermissionRuleMatchExecution, PermissionRuleMatchStrategy, PermissionRuleScope,
        match_permission_rule,
    },
    tool_executor::ResolvedToolExecutionHookContext,
};

pub struct SessionApprovalHistoryPermissionPolicy {
    rules: Arc<dyn AgentPermissionRulesServiceContract>,
}

impl SessionApprovalHistoryPermissionPolicy {
    pub fn new(rules: Arc<dyn AgentPermissionRulesServiceContract>) -> Self {
        Self { rules }
    }
}

fn strategy_name(strategy: PermissionRuleMatchStrategy) -> &'static str {
    match strategy {
        PermissionRuleMatchStrategy::ToolNameOnly => "tool_name_only",
        PermissionRuleMatchStrategy::MatchesRule => "matches_rule",
    }
}

// Original: SessionApprovalHistoryPermissionPolicyService.evaluate().
pub fn session_approval_history(
    context: &ResolvedToolExecutionHookContext,
    patterns: &[String],
) -> Option<PermissionPolicyResult> {
    let matches_rule = |pattern: &str| context.execution.matches_rule(pattern);
    for pattern in patterns {
        let rule = PermissionRule {
            decision: PermissionRuleDecision::Allow,
            scope: PermissionRuleScope::SessionRuntime,
            pattern: pattern.clone(),
            reason: Some("approve for session".into()),
        };
        let Some(matched) = match_permission_rule(
            &rule,
            &context.tool_call.name,
            PermissionRuleMatchExecution {
                matches_rule: Some(&matches_rule),
            },
        ) else {
            continue;
        };
        let mut reason = PermissionDecisionReason::new();
        reason.insert(
            "has_rule_args".into(),
            PermissionReasonValue::Boolean(matched.has_rule_args),
        );
        reason.insert(
            "match_strategy".into(),
            PermissionReasonValue::String(strategy_name(matched.strategy).into()),
        );
        return Some(PermissionPolicyResult::Approve {
            reason: Some(reason),
            execution_metadata: None,
        });
    }
    None
}

impl PermissionPolicy for SessionApprovalHistoryPermissionPolicy {
    fn name(&self) -> &str {
        "session-approval-history"
    }
    fn evaluate<'a>(
        &'a self,
        context: &'a ResolvedToolExecutionHookContext,
    ) -> PermissionPolicyFuture<'a> {
        async move { session_approval_history(context, &self.rules.session_approval_rule_patterns()) }.boxed()
    }
}
