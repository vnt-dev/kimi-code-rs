//! Final permission-policy fallback which requests user approval.
//!
//! Original: `agent/permissionPolicy/policies/fallback-ask.ts`.

use futures_util::FutureExt;

use crate::agent::permission_policy::{
    PermissionPolicy, PermissionPolicyFuture, PermissionPolicyResult,
};

#[derive(Default)]
pub struct FallbackAskPermissionPolicy;

impl PermissionPolicy for FallbackAskPermissionPolicy {
    fn name(&self) -> &str {
        "fallback-ask"
    }

    // Original: FallbackAskPermissionPolicyService.evaluate().
    fn evaluate<'a>(
        &'a self,
        _context: &'a crate::agent::tool_executor::ResolvedToolExecutionHookContext,
    ) -> PermissionPolicyFuture<'a> {
        async {
            Some(PermissionPolicyResult::Ask {
                reason: None,
                resolve_approval: None,
                resolve_error: None,
            })
        }
        .boxed()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_source_policy_name() {
        assert_eq!(FallbackAskPermissionPolicy.name(), "fallback-ask");
    }
}
