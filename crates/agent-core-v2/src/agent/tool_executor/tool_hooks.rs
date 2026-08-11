//! Tool-execution hook contexts and decisions.
//!
//! Original: `packages/agent-core-v2/src/agent/toolExecutor/toolHooks.ts`.

use std::{
    ops::{Deref, DerefMut},
    sync::Arc,
};

use serde_json::Value;

use crate::{
    _base::utils::abort::AbortSignal,
    kosong::contract::{message::ToolCall, request_trace::LlmRequestTrace},
    tool::{ErasedExecutableTool, ExecutableToolResult, RunnableToolExecution},
};

/// Context shared by authorization, preparation, and completion tool hooks.
///
/// `tool` stays optional because hooks run for unresolved and invalid calls as
/// well as successfully resolved executable tools.
#[derive(Clone)]
pub struct ToolExecutionHookContext {
    pub turn_id: crate::agent::TurnId,
    pub signal: AbortSignal,
    pub trace: Option<LlmRequestTrace>,
    pub tool_call: ToolCall,
    pub tool_calls: Vec<ToolCall>,
    pub tool: Option<Arc<dyn ErasedExecutableTool>>,
    pub args: Value,
}

/// Hook context after a tool has produced a runnable execution.
///
/// Rust keeps the shared fields in `context` to avoid copying a move-only
/// `AbortSignal`; `Deref` retains the source contract's direct field access.
#[derive(Clone)]
pub struct ResolvedToolExecutionHookContext {
    pub context: ToolExecutionHookContext,
    pub execution: RunnableToolExecution,
}

impl ResolvedToolExecutionHookContext {
    pub fn new(context: ToolExecutionHookContext, execution: RunnableToolExecution) -> Self {
        Self { context, execution }
    }
}

impl Deref for ResolvedToolExecutionHookContext {
    type Target = ToolExecutionHookContext;

    fn deref(&self) -> &Self::Target {
        &self.context
    }
}

impl DerefMut for ResolvedToolExecutionHookContext {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.context
    }
}

/// Decision returned by a tool authorization hook.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AuthorizeToolExecutionResult {
    pub block: Option<bool>,
    pub reason: Option<String>,
    pub synthetic_result: Option<ExecutableToolResult>,
    pub execution_metadata: Option<Value>,
}

/// Decision returned by a tool preparation hook.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PrepareToolExecutionResult {
    pub block: Option<bool>,
    pub reason: Option<String>,
    pub synthetic_result: Option<ExecutableToolResult>,
    pub execution_metadata: Option<Value>,
    pub updated_args: Option<Value>,
}

impl PrepareToolExecutionResult {
    pub fn from_authorization(authorization: AuthorizeToolExecutionResult) -> Self {
        Self {
            block: authorization.block,
            reason: authorization.reason,
            synthetic_result: authorization.synthetic_result,
            execution_metadata: authorization.execution_metadata,
            updated_args: None,
        }
    }
}

/// Mutable context passed through the ordered pre-execution hook chain.
#[derive(Clone)]
pub struct ToolBeforeExecuteContext {
    pub resolved: ResolvedToolExecutionHookContext,
    pub decision: Option<AuthorizeToolExecutionResult>,
}

impl ToolBeforeExecuteContext {
    pub fn new(resolved: ResolvedToolExecutionHookContext) -> Self {
        Self {
            resolved,
            decision: None,
        }
    }
}

impl Deref for ToolBeforeExecuteContext {
    type Target = ResolvedToolExecutionHookContext;

    fn deref(&self) -> &Self::Target {
        &self.resolved
    }
}

impl DerefMut for ToolBeforeExecuteContext {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.resolved
    }
}

/// Mutable context passed through the ordered post-execution hook chain.
#[derive(Clone)]
pub struct ToolDidExecuteContext {
    pub context: ToolExecutionHookContext,
    pub result: ExecutableToolResult,
    pub stop_turn: Option<bool>,
}

impl Deref for ToolDidExecuteContext {
    type Target = ToolExecutionHookContext;

    fn deref(&self) -> &Self::Target {
        &self.context
    }
}

impl DerefMut for ToolDidExecuteContext {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.context
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        _base::utils::abort::AbortController,
        kosong::contract::message::{ToolCall, ToolCallType},
        tool::ExecutableToolResult,
    };

    fn hook_context() -> ToolExecutionHookContext {
        ToolExecutionHookContext {
            turn_id: crate::agent::TurnId::new(7),
            signal: AbortController::new().signal(),
            trace: Some(LlmRequestTrace::new(Some("trace-7".into()))),
            tool_call: ToolCall {
                call_type: ToolCallType::Function,
                id: "call-1".into(),
                name: "Read".into(),
                arguments: Some("{\"path\":\"a.txt\"}".into()),
                extras: None,
                stream_index: None,
            },
            tool_calls: Vec::new(),
            tool: None,
            args: serde_json::json!({"path": "a.txt"}),
        }
    }

    #[test]
    fn hook_decisions_keep_optional_source_fields_and_mutable_contexts() {
        let context = hook_context();
        let authorization = AuthorizeToolExecutionResult {
            block: Some(true),
            reason: Some("permission denied".into()),
            synthetic_result: Some(ExecutableToolResult::error("blocked")),
            execution_metadata: Some(serde_json::json!({"policy": "ask"})),
        };
        let prepared = PrepareToolExecutionResult::from_authorization(authorization.clone());

        assert_eq!(prepared.block, authorization.block);
        assert_eq!(prepared.reason, authorization.reason);
        assert_eq!(prepared.synthetic_result, authorization.synthetic_result);
        assert_eq!(
            prepared.execution_metadata,
            authorization.execution_metadata
        );
        assert_eq!(prepared.updated_args, None);

        let did_execute = ToolDidExecuteContext {
            context,
            result: ExecutableToolResult::success("done"),
            stop_turn: Some(true),
        };
        assert_eq!(did_execute.context.turn_id, crate::agent::TurnId::new(7));
        assert_eq!(did_execute.stop_turn, Some(true));
        assert!(!did_execute.context.signal.aborted());
    }
}
