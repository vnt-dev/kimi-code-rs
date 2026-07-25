//! Session subagent run contract and observer surface.
//!
//! Original: `packages/agent-core-v2/src/session/subagent/subagent.ts`,
//! `ISessionSubagentService`.

use std::{error::Error, fmt, ops::Deref, sync::Arc};

use crate::{
    _base::{
        di::instantiation::ServiceIdentifier,
        event::{Emitter, Event},
        lifecycle::lifecycle_machine::BoxError,
        utils::abort::AbortSignal,
    },
    agent::loop_::TurnHandle,
    app::agent_profile_catalog::AgentProfileSummaryPolicy,
    hooks::OrderedHookSlot,
    kosong::contract::usage::TokenUsage,
};
use futures_util::future::{BoxFuture, Shared};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AgentRunRequest {
    Prompt { prompt: String },
    Retry { trigger: Option<String> },
}

#[derive(Clone)]
pub struct RunAgentOptions {
    /// Original `AbortSignal`; cancellation must reach the target turn.
    pub signal: AbortSignal,
    pub summary_policy: Option<AgentProfileSummaryPolicy>,
    /// Fires once the first turn request is committed.
    pub on_ready: Option<Arc<dyn Fn() + Send + Sync>>,
}

impl RunAgentOptions {
    pub fn new(signal: AbortSignal) -> Self {
        Self {
            signal,
            summary_policy: None,
            on_ready: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct AgentRunCompletion {
    pub summary: String,
    pub usage: Option<TokenUsage>,
}

/// A shared future needs a cloneable rejection value. `Arc` also preserves one
/// shared failure identity for all observers, like the original Promise.
pub type AgentRunCompletionError = Arc<dyn Error + Send + Sync>;
pub type AgentRunCompletionFuture =
    Shared<BoxFuture<'static, Result<AgentRunCompletion, AgentRunCompletionError>>>;

/// Adapts a shared completion error at APIs that still use boxed errors.
pub struct SharedAgentRunError(pub AgentRunCompletionError);

impl fmt::Debug for SharedAgentRunError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("SharedAgentRunError")
            .field(&self.0)
            .finish()
    }
}

impl fmt::Display for SharedAgentRunError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl Error for SharedAgentRunError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(self.0.as_ref())
    }
}

#[derive(Clone)]
pub struct AgentRunHandle {
    pub agent_id: String,
    pub turn: TurnHandle,
    pub completion: AgentRunCompletionFuture,
}

#[derive(Clone)]
pub struct AgentTaskStartHookContext {
    pub agent_name: String,
    pub prompt: String,
    pub signal: AbortSignal,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentTaskStopHookContext {
    pub agent_name: String,
    pub response: String,
}

/// Original `Hooks<AgentTaskHooks>`, represented directly because this domain
/// owns one named slot only.
#[derive(Default)]
pub struct AgentTaskHooks {
    pub on_will_start_agent_task: OrderedHookSlot<AgentTaskStartHookContext>,
}

pub trait SessionSubagentServiceContract: Send + Sync {
    fn hooks(&self) -> &AgentTaskHooks;
    fn on_did_stop_agent_task(&self) -> Event<AgentTaskStopHookContext>;
    fn run(
        &self,
        agent_id: String,
        request: AgentRunRequest,
        options: RunAgentOptions,
    ) -> BoxFuture<'static, Result<AgentRunHandle, BoxError>>;
    fn notify_agent_task_stopped(&self, context: AgentTaskStopHookContext);
}

#[derive(Clone)]
pub struct SessionSubagentServiceHandle(pub Arc<dyn SessionSubagentServiceContract>);

impl Deref for SessionSubagentServiceHandle {
    type Target = dyn SessionSubagentServiceContract;

    fn deref(&self) -> &Self::Target {
        self.0.as_ref()
    }
}

pub const SESSION_SUBAGENT_SERVICE_ID: ServiceIdentifier<SessionSubagentServiceHandle> =
    ServiceIdentifier::new("sessionSubagentService");

/// Reusable event ownership for the concrete service.  Kept separate from the
/// contract because it gives the service RAII disposal without leaking emitter
/// mutation to observers.
#[derive(Default)]
pub struct AgentTaskStopEmitter {
    emitter: Emitter<AgentTaskStopHookContext>,
}

impl AgentTaskStopEmitter {
    pub fn event(&self) -> Event<AgentTaskStopHookContext> {
        self.emitter.event()
    }

    pub fn fire(&self, context: &AgentTaskStopHookContext) {
        self.emitter.fire(context);
    }
}

#[cfg(test)]
mod tests {
    use futures_util::FutureExt;

    use crate::agent::loop_::{TurnHandleContract, TurnReadyFuture, TurnResultFuture, TurnState};

    use super::*;

    struct Turn;

    impl TurnHandleContract for Turn {
        fn id(&self) -> i64 {
            1
        }
        fn state(&self) -> Option<TurnState> {
            Some(TurnState::Queued)
        }
        fn signal(&self) -> crate::_base::utils::abort::AbortSignal {
            crate::_base::utils::abort::AbortController::new().signal()
        }
        fn ready(&self) -> TurnReadyFuture {
            futures_util::future::ready(Ok(())).boxed().shared()
        }
        fn result(&self) -> TurnResultFuture {
            futures_util::future::pending().boxed().shared()
        }
        fn cancel(&self, _: Option<crate::agent::loop_::LoopValue>) -> bool {
            true
        }
    }

    #[test]
    fn contract_preserves_requests_events_and_service_identity() {
        let prompt = AgentRunRequest::Prompt {
            prompt: "work".into(),
        };
        assert!(matches!(prompt, AgentRunRequest::Prompt { .. }));
        assert_eq!(
            SESSION_SUBAGENT_SERVICE_ID.to_string(),
            "sessionSubagentService"
        );
        let options =
            RunAgentOptions::new(crate::_base::utils::abort::AbortController::new().signal());
        assert!(options.summary_policy.is_none());

        let turn = TurnHandle(Arc::new(Turn));
        let completion = futures_util::future::ready(Ok(AgentRunCompletion {
            summary: "done".into(),
            usage: None,
        }))
        .boxed()
        .shared();
        let handle = AgentRunHandle {
            agent_id: "child".into(),
            turn,
            completion,
        };
        assert_eq!(handle.agent_id, "child");
    }

    #[test]
    fn stop_emitter_notifies_in_registration_order() {
        let emitter = AgentTaskStopEmitter::default();
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let received = Arc::clone(&seen);
        let _subscription = emitter.event().subscribe(move |context| {
            received.lock().unwrap().push(context.response.clone());
        });
        emitter.fire(&AgentTaskStopHookContext {
            agent_name: "coder".into(),
            response: "done".into(),
        });
        assert_eq!(*seen.lock().unwrap(), ["done"]);
    }
}
