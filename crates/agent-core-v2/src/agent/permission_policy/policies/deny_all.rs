//! Policy which rejects every tool call.
//!
//! Original: `agent/permissionPolicy/policies/deny-all.ts`.

use crate::agent::permission_policy::{PermissionPolicy, PermissionPolicyResult};
use futures_util::FutureExt;

pub const DEFAULT_DENY_ALL_MESSAGE: &str = "Tool calls are disabled for this agent.";

pub struct DenyAllPermissionPolicy {
    message: String,
}

impl DenyAllPermissionPolicy {
    pub fn new(message: Option<String>) -> Self {
        Self {
            message: message.unwrap_or_else(|| DEFAULT_DENY_ALL_MESSAGE.into()),
        }
    }
}

impl Default for DenyAllPermissionPolicy {
    fn default() -> Self {
        Self::new(None)
    }
}

impl PermissionPolicy for DenyAllPermissionPolicy {
    // Original: DenyAllPermissionPolicyService.evaluate(). Context is
    // intentionally ignored: every resolved execution receives the same deny.
    fn evaluate<'a>(
        &'a self,
        _context: &crate::agent::tool_executor::ResolvedToolExecutionHookContext,
    ) -> crate::agent::permission_policy::PermissionPolicyFuture<'a> {
        async move {
            Some(PermissionPolicyResult::Deny {
                reason: None,
                message: Some(self.message.clone()),
            })
        }
        .boxed()
    }

    fn name(&self) -> &str {
        "deny-all"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_the_source_name_and_default_message() {
        let policy = DenyAllPermissionPolicy::default();
        assert_eq!(policy.name(), "deny-all");
        assert_eq!(policy.message, DEFAULT_DENY_ALL_MESSAGE);
        assert_eq!(
            DenyAllPermissionPolicy::new(Some("text only".into())).message,
            "text only"
        );
    }
}
