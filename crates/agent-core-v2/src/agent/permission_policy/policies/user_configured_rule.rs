//! User-configured allow, deny, and ask rule policies.
//!
//! Original: `agent/permissionPolicy/policies/user-configured-{rule,allow,deny,ask}.ts`.

use std::sync::Arc;

use futures_util::FutureExt;

use crate::agent::{
    permission_policy::{PermissionPolicy, PermissionPolicyFuture, PermissionPolicyResult},
    permission_rules::{
        AgentPermissionRulesServiceContract, PermissionRuleDecision, PermissionRuleMatchExecution,
        PermissionRuleScope, match_permission_rule,
    },
    tool_executor::ResolvedToolExecutionHookContext,
};

fn is_user_configured_scope(scope: PermissionRuleScope) -> bool {
    matches!(
        scope,
        PermissionRuleScope::TurnOverride
            | PermissionRuleScope::Project
            | PermissionRuleScope::User
    )
}

pub fn evaluate_user_configured_rule(
    context: &ResolvedToolExecutionHookContext,
    decision: PermissionRuleDecision,
    rules: &dyn AgentPermissionRulesServiceContract,
) -> Option<PermissionPolicyResult> {
    let matches_rule = |pattern: &str| context.execution.matches_rule(pattern);
    let rule = rules.rules().into_iter().find(|rule| {
        is_user_configured_scope(rule.scope)
            && rule.decision == decision
            && match_permission_rule(
                rule,
                &context.tool_call.name,
                PermissionRuleMatchExecution {
                    matches_rule: Some(&matches_rule),
                },
            )
            .is_some()
    })?;
    match decision {
        PermissionRuleDecision::Deny => Some(PermissionPolicyResult::Deny {
            reason: None,
            message: Some(default_permission_rule_deny_message(
                &context.tool_call.name,
                rule.reason.as_deref(),
            )),
        }),
        PermissionRuleDecision::Ask => Some(PermissionPolicyResult::Ask {
            reason: None,
            resolve_approval: None,
            resolve_error: None,
        }),
        PermissionRuleDecision::Allow => Some(PermissionPolicyResult::Approve {
            reason: None,
            execution_metadata: None,
        }),
    }
}

pub fn default_permission_rule_deny_message(tool: &str, reason: Option<&str>) -> String {
    let suffix = reason
        .filter(|reason| !reason.is_empty())
        .map(|reason| format!(" Reason: {reason}"))
        .unwrap_or_default();
    format!("Tool \"{tool}\" was denied by permission rule.{suffix}")
}

macro_rules! user_policy {
    ($type_name:ident, $name:literal, $decision:expr) => {
        pub struct $type_name { rules: Arc<dyn AgentPermissionRulesServiceContract> }
        impl $type_name { pub fn new(rules: Arc<dyn AgentPermissionRulesServiceContract>) -> Self { Self { rules } } }
        impl PermissionPolicy for $type_name {
            fn name(&self) -> &str { $name }
            fn evaluate<'a>(&'a self, context: &'a ResolvedToolExecutionHookContext) -> PermissionPolicyFuture<'a> {
                async move { evaluate_user_configured_rule(context, $decision, self.rules.as_ref()) }.boxed()
            }
        }
    };
}

user_policy!(
    UserConfiguredAllowPermissionPolicy,
    "user-configured-allow",
    PermissionRuleDecision::Allow
);
user_policy!(
    UserConfiguredDenyPermissionPolicy,
    "user-configured-deny",
    PermissionRuleDecision::Deny
);
user_policy!(
    UserConfiguredAskPermissionPolicy,
    "user-configured-ask",
    PermissionRuleDecision::Ask
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_scopes_and_deny_message_preserve_rule_semantics() {
        assert!(is_user_configured_scope(PermissionRuleScope::TurnOverride));
        assert!(is_user_configured_scope(PermissionRuleScope::Project));
        assert!(is_user_configured_scope(PermissionRuleScope::User));
        assert!(!is_user_configured_scope(
            PermissionRuleScope::SessionRuntime
        ));
        assert_eq!(
            default_permission_rule_deny_message("Bash", None),
            "Tool \"Bash\" was denied by permission rule."
        );
        assert_eq!(
            default_permission_rule_deny_message("Bash", Some("unsafe")),
            "Tool \"Bash\" was denied by permission rule. Reason: unsafe"
        );
        assert_eq!(
            default_permission_rule_deny_message("Bash", Some("")),
            "Tool \"Bash\" was denied by permission rule."
        );
    }
}
