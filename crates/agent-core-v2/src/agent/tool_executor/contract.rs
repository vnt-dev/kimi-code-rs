//! Agent-scope tool executor public contract.
//!
//! Original: `packages/agent-core-v2/src/agent/toolExecutor/toolExecutor.ts`.

use std::{ops::Deref, sync::Arc};

use futures_util::stream::BoxStream;
use serde_json::Value;

use crate::{
    _base::{
        di::{instantiation::ServiceIdentifier, lifecycle::DisposableHandle},
        lifecycle::lifecycle_machine::BoxError,
        utils::abort::AbortSignal,
    },
    hooks::OrderedHookSlot,
    kosong::contract::{message::ToolCall, request_trace::LlmRequestTrace},
    tool::{ToolResult, ToolSource},
};

use super::{ToolBeforeExecuteContext, ToolDidExecuteContext};

#[derive(Clone, Debug, PartialEq)]
pub struct ToolCallStartedPayload {
    pub tool_call_id: String,
    pub name: String,
    pub args: Value,
}

pub type ToolCallStartedHandler = Arc<dyn Fn(ToolCallStartedPayload) + Send + Sync>;

#[derive(Clone)]
pub struct ToolExecutorExecuteOptions {
    pub signal: AbortSignal,
    pub turn_id: i64,
    pub trace: Option<LlmRequestTrace>,
    pub on_tool_call: Option<ToolCallStartedHandler>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ToolExecutionResult {
    pub tool_call_id: String,
    pub tool_name: String,
    pub result: ToolResult,
}

/// Rust's fallible stream directly represents JavaScript's async iterator
/// whose `next()` may reject while preserving yielded-result ordering.
pub type ToolExecutionStream = BoxStream<'static, Result<ToolExecutionResult, BoxError>>;

pub type MissingToolDescriber = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;
pub type UnavailableToolDescriber = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ToolCallGuardInput {
    pub name: String,
    pub source: ToolSource,
}

pub type ToolCallGuard = Arc<dyn Fn(&ToolCallGuardInput) -> Option<String> + Send + Sync>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ToolCallDupType {
    SameStep,
    CrossStep,
}

pub struct AgentToolExecutorHooks {
    pub on_before_execute_tool: OrderedHookSlot<ToolBeforeExecuteContext>,
    pub on_did_execute_tool: OrderedHookSlot<ToolDidExecuteContext>,
}

impl Default for AgentToolExecutorHooks {
    fn default() -> Self {
        Self {
            on_before_execute_tool: OrderedHookSlot::new(),
            on_did_execute_tool: OrderedHookSlot::new(),
        }
    }
}

pub trait AgentToolExecutorServiceContract: Send + Sync {
    fn execute(
        &self,
        calls: Vec<ToolCall>,
        options: ToolExecutorExecuteOptions,
    ) -> ToolExecutionStream;

    fn hooks(&self) -> &AgentToolExecutorHooks;
    fn record_dup_type(&self, tool_call_id: String, dup_type: ToolCallDupType);

    fn register_tool_call_guard(&self, guard: ToolCallGuard) -> DisposableHandle;
    fn register_unavailable_tool_describer(
        &self,
        describer: UnavailableToolDescriber,
    ) -> DisposableHandle;
    fn register_missing_tool_describer(&self, describer: MissingToolDescriber) -> DisposableHandle;
}

#[derive(Clone)]
pub struct AgentToolExecutorServiceHandle(pub Arc<dyn AgentToolExecutorServiceContract>);

impl Deref for AgentToolExecutorServiceHandle {
    type Target = dyn AgentToolExecutorServiceContract;

    fn deref(&self) -> &Self::Target {
        self.0.as_ref()
    }
}

pub const AGENT_TOOL_EXECUTOR_SERVICE_ID: ServiceIdentifier<AgentToolExecutorServiceHandle> =
    ServiceIdentifier::new("agentToolExecutorService");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_identity_and_contract_values_match_source() {
        assert_eq!(
            AGENT_TOOL_EXECUTOR_SERVICE_ID.to_string(),
            "agentToolExecutorService"
        );
        assert_eq!(ToolCallDupType::SameStep, ToolCallDupType::SameStep);
        assert_eq!(ToolCallDupType::CrossStep, ToolCallDupType::CrossStep);
        assert_eq!(
            ToolCallGuardInput {
                name: "Read".into(),
                source: ToolSource::Builtin,
            },
            ToolCallGuardInput {
                name: "Read".into(),
                source: ToolSource::Builtin,
            }
        );
    }
}
