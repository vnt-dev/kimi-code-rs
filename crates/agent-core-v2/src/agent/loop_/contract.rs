//! Agent loop service, turn, step, hook, and result contracts.
//!
//! Original: `packages/agent-core-v2/src/agent/loop/loop.ts`.

use std::{error::Error, fmt, sync::Arc};

use async_trait::async_trait;
use futures_util::future::{BoxFuture, Shared};
use serde_json::Value;

use crate::{
    _base::{
        di::{instantiation::ServiceIdentifier, lifecycle::DisposableHandle},
        utils::abort::AbortSignal,
    },
    hooks::OrderedHookSlot,
    kosong::contract::{provider::FinishReason, usage::TokenUsage},
};

use super::{StepRequest, StepRequestQueuePosition};

#[derive(Clone)]
pub enum LoopValue {
    Error(Arc<dyn Error + Send + Sync>),
    Value(Value),
}

impl fmt::Debug for LoopValue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Error(error) => formatter.debug_tuple("Error").field(error).finish(),
            Self::Value(value) => formatter.debug_tuple("Value").field(value).finish(),
        }
    }
}

impl fmt::Display for LoopValue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Error(error) => error.fmt(formatter),
            Self::Value(value) => value.fmt(formatter),
        }
    }
}

impl Error for LoopValue {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Error(error) => Some(error.as_ref()),
            Self::Value(_) => None,
        }
    }
}

impl From<Arc<dyn Error + Send + Sync>> for LoopValue {
    fn from(error: Arc<dyn Error + Send + Sync>) -> Self {
        Self::Error(error)
    }
}

impl From<Value> for LoopValue {
    fn from(value: Value) -> Self {
        Self::Value(value)
    }
}

#[derive(Clone)]
pub struct BeforeStepContext {
    pub turn_id: i64,
    pub step: u64,
    pub signal: AbortSignal,
}

#[derive(Clone)]
pub struct AfterStepContext {
    pub turn_id: i64,
    pub step: u64,
    pub signal: AbortSignal,
    pub usage: TokenUsage,
    pub finish_reason: FinishReason,
    pub stop_turn: bool,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct LoopErrorHandlerRegistrationOptions<'a> {
    pub before: Option<&'a str>,
    pub after: Option<&'a str>,
}

pub type LoopOnStarted = Arc<dyn Fn(u64) + Send + Sync>;

#[derive(Clone, Default)]
pub struct LoopRunOptions {
    pub turn_id: i64,
    pub signal: Option<AbortSignal>,
    pub on_started: Option<LoopOnStarted>,
}

#[derive(Clone, Debug)]
pub enum LoopRunResult {
    Completed { steps: u64, truncated: bool },
    Failed { steps: u64, error: LoopValue },
    Cancelled { steps: u64, reason: LoopValue },
}

pub type TurnResult = LoopRunResult;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StepState {
    Queued,
    Running,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Clone, Debug)]
pub enum StepResult {
    Completed,
    Failed { error: LoopValue },
    Cancelled { reason: LoopValue },
}

pub type StepResultFuture = Shared<BoxFuture<'static, StepResult>>;
pub type TurnReadyFuture = Shared<BoxFuture<'static, Result<(), LoopValue>>>;
pub type TurnResultFuture = Shared<BoxFuture<'static, LoopRunResult>>;

pub trait StepHandleContract: Send + Sync {
    fn id(&self) -> &str;
    fn turn_id(&self) -> i64;
    fn state(&self) -> StepState;
    fn signal(&self) -> AbortSignal;
    fn result(&self) -> StepResultFuture;
    fn cancel(&self, reason: Option<LoopValue>) -> bool;
}

#[derive(Clone)]
pub struct StepHandle(pub Arc<dyn StepHandleContract>);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TurnState {
    Queued,
    Running,
    Completed,
    Failed,
    Cancelled,
}

pub trait TurnHandleContract: Send + Sync {
    fn id(&self) -> i64;
    fn state(&self) -> Option<TurnState>;
    fn signal(&self) -> AbortSignal;
    fn ready(&self) -> TurnReadyFuture;
    fn result(&self) -> TurnResultFuture;
    fn cancel(&self, reason: Option<LoopValue>) -> bool;
}

#[derive(Clone)]
pub struct TurnHandle(pub Arc<dyn TurnHandleContract>);

#[derive(Clone)]
pub struct StepAssignment {
    pub turn: TurnHandle,
    pub step: StepHandle,
}

pub type StepAssignmentFuture = Shared<BoxFuture<'static, Result<StepAssignment, LoopValue>>>;
pub type EnqueueAbort = Arc<dyn Fn(Option<LoopValue>) -> bool + Send + Sync>;

#[derive(Clone)]
pub struct EnqueueReceipt {
    pub assigned: StepAssignmentFuture,
    abort: EnqueueAbort,
}

impl EnqueueReceipt {
    pub fn new(assigned: StepAssignmentFuture, abort: EnqueueAbort) -> Self {
        Self { assigned, abort }
    }

    pub fn abort(&self, reason: Option<LoopValue>) -> bool {
        (self.abort)(reason)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AgentLoopState {
    Idle,
    Running,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentLoopStatus {
    pub state: AgentLoopState,
    pub active_turn_id: Option<i64>,
    pub pending_turn_ids: Vec<i64>,
    pub has_pending_requests: bool,
    pub active_trace_id: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct StepEnqueueOptions {
    pub at: Option<StepRequestQueuePosition>,
}

pub type LoopRetry =
    Arc<dyn Fn(Arc<dyn StepRequest>, Option<StepEnqueueOptions>) -> StepHandle + Send + Sync>;

pub struct LoopErrorContext {
    pub current_step: Option<StepHandle>,
    pub turn_id: i64,
    pub step: Option<u64>,
    pub step_id: Option<String>,
    pub signal: AbortSignal,
    pub error: LoopValue,
    pub failed_driver: Option<Arc<dyn StepRequest>>,
    pub retry: LoopRetry,
}

#[async_trait]
pub trait LoopErrorHandler: Send + Sync {
    fn id(&self) -> &str;
    fn matches(&self, context: &LoopErrorContext) -> bool;
    async fn handle(&self, context: &mut LoopErrorContext) -> Result<Option<bool>, LoopValue>;
}

pub struct AgentLoopHooks {
    pub on_will_begin_step: OrderedHookSlot<BeforeStepContext>,
    pub on_did_finish_step: OrderedHookSlot<AfterStepContext>,
}

impl Default for AgentLoopHooks {
    fn default() -> Self {
        Self {
            on_will_begin_step: OrderedHookSlot::new(),
            on_did_finish_step: OrderedHookSlot::new(),
        }
    }
}

#[async_trait]
pub trait AgentLoopServiceContract: Send + Sync {
    fn enqueue(
        &self,
        request: Arc<dyn StepRequest>,
        options: Option<StepEnqueueOptions>,
    ) -> Result<EnqueueReceipt, LoopValue>;

    async fn run(&self, options: LoopRunOptions) -> LoopRunResult;

    fn status(&self) -> AgentLoopStatus;
    fn cancel(&self, turn_id: Option<i64>, reason: Option<LoopValue>) -> bool;
    async fn settled(&self);
    fn has_pending_requests(&self) -> bool;

    fn register_loop_error_handler(
        &self,
        handler: Arc<dyn LoopErrorHandler>,
        options: LoopErrorHandlerRegistrationOptions<'_>,
    ) -> Result<DisposableHandle, LoopValue>;

    fn hooks(&self) -> &AgentLoopHooks;
}

pub const AGENT_LOOP_SERVICE_ID: ServiceIdentifier<dyn AgentLoopServiceContract> =
    ServiceIdentifier::new("agentLoopService");

#[cfg(test)]
mod tests {
    use futures_util::{FutureExt, future::ready};

    use super::*;

    #[test]
    fn states_defaults_and_service_identity_match_source_contract() {
        assert_eq!(AGENT_LOOP_SERVICE_ID.to_string(), "agentLoopService");
        assert_eq!(LoopRunOptions::default().turn_id, 0);
        assert_eq!(StepEnqueueOptions::default().at, None);
        assert_eq!(StepState::Queued, StepState::Queued);
        assert_eq!(TurnState::Running, TurnState::Running);
        let status = AgentLoopStatus {
            state: AgentLoopState::Idle,
            active_turn_id: None,
            pending_turn_ids: vec![2, 3],
            has_pending_requests: true,
            active_trace_id: Some("trace".into()),
        };
        assert_eq!(status.pending_turn_ids, [2, 3]);
    }

    #[tokio::test]
    async fn result_futures_are_shared_like_javascript_promises() {
        let result: StepResultFuture = ready(StepResult::Completed).boxed().shared();
        assert!(matches!(result.clone().await, StepResult::Completed));
        assert!(matches!(result.await, StepResult::Completed));

        let turn: TurnResultFuture = ready(LoopRunResult::Completed {
            steps: 2,
            truncated: false,
        })
        .boxed()
        .shared();
        assert!(matches!(
            turn.clone().await,
            LoopRunResult::Completed {
                steps: 2,
                truncated: false
            }
        ));
        assert!(matches!(turn.await, LoopRunResult::Completed { .. }));
    }
}
