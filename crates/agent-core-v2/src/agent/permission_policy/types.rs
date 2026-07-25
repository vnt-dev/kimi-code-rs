use std::{collections::BTreeMap, future::Future, pin::Pin};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    agent::tool_executor::{PrepareToolExecutionResult, ResolvedToolExecutionHookContext},
    session::approval::ApprovalResponse,
};

// Original:
//   packages/agent-core-v2/src/agent/permissionPolicy/types.ts
//   PermissionMode
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PermissionMode {
    #[default]
    Manual,
    Yolo,
    Auto,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PermissionDecision {
    Approve,
    Deny,
    Ask,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(untagged)]
pub enum PermissionReasonValue {
    String(String),
    Number(f64),
    Boolean(bool),
    Null(()),
}
pub type PermissionDecisionReason = BTreeMap<String, PermissionReasonValue>;

pub type ApprovalResolver =
    Box<dyn Fn(ApprovalResponse) -> Option<PermissionPolicyResolution> + Send + Sync>;
pub type ErrorResolver = Box<dyn Fn(Value) -> Option<PermissionPolicyResolution> + Send + Sync>;

pub enum PermissionPolicyResolution {
    Result(PermissionPolicyResult),
    Prepared(Box<PrepareToolExecutionResult>),
}

pub enum PermissionPolicyResult {
    Approve {
        reason: Option<PermissionDecisionReason>,
        execution_metadata: Option<Value>,
    },
    Deny {
        reason: Option<PermissionDecisionReason>,
        message: Option<String>,
    },
    Ask {
        reason: Option<PermissionDecisionReason>,
        resolve_approval: Option<ApprovalResolver>,
        resolve_error: Option<ErrorResolver>,
    },
}

pub type PermissionPolicyFuture<'a> =
    Pin<Box<dyn Future<Output = Option<PermissionPolicyResult>> + Send + 'a>>;

/// Original: PermissionPolicy.evaluate(). A boxed future makes the policy
/// trait object-safe so the future registry can own heterogeneous policies.
pub trait PermissionPolicy: Send + Sync {
    fn name(&self) -> &str;
    fn evaluate<'a>(
        &'a self,
        context: &'a ResolvedToolExecutionHookContext,
    ) -> PermissionPolicyFuture<'a>;
}
