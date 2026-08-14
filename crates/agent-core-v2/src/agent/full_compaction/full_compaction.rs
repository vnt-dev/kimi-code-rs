//! Full-compaction contract.
//!
//! Original: `agent/fullCompaction/fullCompaction.ts`.

use parking_lot::Mutex;
use std::sync::Arc;
use std::{error::Error, fmt, ops::Deref};

use crate::{
    _base::{
        di::{
            instantiation::ServiceIdentifier,
            lifecycle::{Disposable, DisposeResult},
        },
        event::Event,
        utils::abort::AbortController,
    },
    hooks::OrderedHookSlot,
    kosong::contract::request_trace::LlmRequestTrace,
};
use async_trait::async_trait;
use futures_util::future::{BoxFuture, Shared};

use super::{CompactionResult, CompactionSource};

pub type FullCompactionError = Arc<dyn Error + Send + Sync>;
pub type FullCompactionFuture =
    Shared<BoxFuture<'static, Result<CompactionResult, FullCompactionError>>>;

#[derive(Clone, Debug)]
pub struct FullCompactionInput {
    pub source: CompactionSource,
    pub instruction: Option<String>,
}

#[derive(Clone)]
pub struct FullCompactionTask {
    pub abort_controller: AbortController,
    pub promise: FullCompactionFuture,
    pub trigger: CompactionSource,
    pub token_count: u64,
    trace: Arc<Mutex<Option<LlmRequestTrace>>>,
}

impl FullCompactionTask {
    pub(crate) fn new(
        abort_controller: AbortController,
        promise: FullCompactionFuture,
        trigger: CompactionSource,
        token_count: u64,
        trace: Arc<Mutex<Option<LlmRequestTrace>>>,
    ) -> Self {
        Self {
            abort_controller,
            promise,
            trigger,
            token_count,
            trace,
        }
    }

    pub fn trace_id(&self) -> Option<String> {
        self.trace
            .lock()
            .as_ref()
            .and_then(LlmRequestTrace::trace_id)
    }

    pub(crate) fn set_trace(&self, trace: LlmRequestTrace) {
        *self.trace.lock() = Some(trace);
    }

    pub(crate) fn set_trace_id(&self, trace_id: Option<String>) {
        let mut trace = self.trace.lock();
        match trace.as_ref() {
            Some(trace) => trace.set_trace_id(trace_id),
            None => *trace = Some(LlmRequestTrace::new(trace_id)),
        }
    }
}

impl fmt::Debug for FullCompactionTask {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("FullCompactionTask")
            .field("trigger", &self.trigger)
            .field("token_count", &self.token_count)
            .field("trace_id", &self.trace_id())
            .field("aborted", &self.abort_controller.signal().aborted())
            .finish_non_exhaustive()
    }
}

pub struct AgentFullCompactionHooks {
    pub on_will_compact: OrderedHookSlot<FullCompactionTask>,
}

impl Default for AgentFullCompactionHooks {
    fn default() -> Self {
        Self {
            on_will_compact: OrderedHookSlot::new(),
        }
    }
}

#[async_trait]
pub trait AgentFullCompactionServiceContract: Disposable + Send + Sync {
    fn compacting(&self) -> Option<FullCompactionTask>;
    fn begin(&self, input: FullCompactionInput) -> Result<bool, FullCompactionError>;
    async fn shutdown(&self) {}
    fn hooks(&self) -> &AgentFullCompactionHooks;
    fn on_did_finish_compaction(&self) -> Event<FullCompactionTask>;
}

#[derive(Clone)]
pub struct AgentFullCompactionServiceHandle(pub Arc<dyn AgentFullCompactionServiceContract>);

impl Deref for AgentFullCompactionServiceHandle {
    type Target = dyn AgentFullCompactionServiceContract;
    fn deref(&self) -> &Self::Target {
        self.0.as_ref()
    }
}

impl Disposable for AgentFullCompactionServiceHandle {
    fn dispose(&self) -> DisposeResult {
        self.0.dispose()
    }
}

pub const AGENT_FULL_COMPACTION_SERVICE_ID: ServiceIdentifier<AgentFullCompactionServiceHandle> =
    ServiceIdentifier::new("agentFullCompactionService");
