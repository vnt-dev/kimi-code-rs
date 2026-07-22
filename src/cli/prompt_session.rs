use std::{collections::BTreeMap, error::Error, future::Future, pin::Pin, sync::Arc};

use async_trait::async_trait;
use serde_json::{Map, Value};

use crate::sdk::types::{
    CronTaskSnapshot, GoalSnapshot, PermissionMode, PromptPart, SessionStatus, SessionSummary,
};

use super::{
    prompt_render::{HookResultEvent, PromptValue, RetryingEvent},
    run_prompt::{TurnEndReason, TurnErrorPayload},
};

pub type PromptSessionError = Box<dyn Error + Send + Sync>;
pub type TelemetryProperties = Map<String, Value>;
pub type EventListener = Arc<dyn Fn(PromptEvent) + Send + Sync>;
pub type Unsubscribe = Box<dyn FnOnce() + Send>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptConfig {
    pub default_model: Option<String>,
    pub telemetry: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ConfigDiagnostics {
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ListSessionsOptions {
    pub work_dir: Option<String>,
    pub session_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateSessionOptions {
    pub work_dir: String,
    pub model: Option<String>,
    pub permission: Option<PermissionMode>,
    pub additional_dirs: Option<Vec<String>>,
    pub drain_agent_tasks_on_stop: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResumeSessionInput {
    pub id: String,
    pub additional_dirs: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateGoalInput {
    pub objective: String,
    pub replace: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PromptInput {
    Text(String),
    Parts(Vec<PromptPart>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrintTurnAction {
    Finish,
    Continue,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PromptEvent {
    pub session_id: String,
    pub agent_id: String,
    pub kind: PromptEventKind,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PromptEventKind {
    Error {
        code: String,
        message: String,
    },
    TurnStarted {
        turn_id: u64,
    },
    GoalUpdated {
        snapshot: Option<GoalSnapshot>,
        completion: bool,
    },
    TurnStepStarted {
        turn_id: u64,
    },
    TurnStepInterrupted {
        turn_id: u64,
    },
    TurnStepRetrying {
        turn_id: u64,
        event: RetryingEvent,
    },
    AssistantDelta {
        turn_id: u64,
        delta: String,
    },
    HookResult {
        turn_id: Option<u64>,
        event: HookResultEvent,
    },
    ThinkingDelta {
        turn_id: u64,
        delta: String,
    },
    ToolCallStarted {
        turn_id: u64,
        tool_call_id: String,
        name: String,
        args: PromptValue,
    },
    ToolCallDelta {
        turn_id: u64,
        tool_call_id: String,
        name: Option<String>,
        arguments_part: Option<String>,
    },
    ToolResult {
        turn_id: u64,
        tool_call_id: String,
        output: PromptValue,
    },
    ToolProgress {
        turn_id: u64,
        text: Option<String>,
    },
    TurnEnded {
        turn_id: u64,
        reason: TurnEndReason,
        error: Option<TurnErrorPayload>,
    },
    Ignored {
        turn_id: Option<u64>,
        event_type: String,
    },
}

impl PromptEventKind {
    pub fn turn_id(&self) -> Option<u64> {
        match self {
            Self::TurnStarted { turn_id }
            | Self::TurnStepStarted { turn_id }
            | Self::TurnStepInterrupted { turn_id }
            | Self::TurnStepRetrying { turn_id, .. }
            | Self::AssistantDelta { turn_id, .. }
            | Self::ThinkingDelta { turn_id, .. }
            | Self::ToolCallStarted { turn_id, .. }
            | Self::ToolCallDelta { turn_id, .. }
            | Self::ToolResult { turn_id, .. }
            | Self::ToolProgress { turn_id, .. }
            | Self::TurnEnded { turn_id, .. } => Some(*turn_id),
            Self::HookResult { turn_id, .. } | Self::Ignored { turn_id, .. } => *turn_id,
            Self::Error { .. } | Self::GoalUpdated { .. } => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalRequest {
    pub turn_id: Option<u64>,
    pub tool_call_id: String,
    pub tool_name: String,
    pub action: String,
    pub display: Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalDecision {
    Approved,
    Rejected,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalScope {
    Session,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalResponse {
    pub decision: ApprovalDecision,
    pub scope: Option<ApprovalScope>,
    pub feedback: Option<String>,
    pub selected_label: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuestionRequest {
    pub turn_id: Option<u64>,
    pub tool_call_id: Option<String>,
    pub questions: Vec<QuestionItem>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuestionOption {
    pub label: String,
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuestionItem {
    pub question: String,
    pub header: Option<String>,
    pub body: Option<String>,
    pub options: Vec<QuestionOption>,
    pub multi_select: bool,
    pub other_label: Option<String>,
    pub other_description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QuestionAnswer {
    Text(String),
    Answered(bool),
}

pub type QuestionAnswers = BTreeMap<String, QuestionAnswer>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuestionAnswerMethod {
    Enter,
    Space,
    NumberKey,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QuestionResult {
    Answers(QuestionAnswers),
    Response {
        answers: QuestionAnswers,
        method: Option<QuestionAnswerMethod>,
    },
}

pub type ApprovalHandlerFuture = Pin<Box<dyn Future<Output = ApprovalResponse> + Send + 'static>>;
pub type QuestionHandlerFuture =
    Pin<Box<dyn Future<Output = Option<QuestionResult>> + Send + 'static>>;
pub type ApprovalHandler = Arc<dyn Fn(ApprovalRequest) -> ApprovalHandlerFuture + Send + Sync>;
pub type QuestionHandler = Arc<dyn Fn(QuestionRequest) -> QuestionHandlerFuture + Send + Sync>;

// Original:
//   apps/kimi-code/src/cli/prompt-session.ts
//   PromptSession
//
// Rust adaptation:
//   Dynamic dispatch is intentional: both v1 and v2 sessions are selected at
//   runtime. async-trait supplies the object-safe boxed futures required here.
#[async_trait]
pub trait PromptSession: Send + Sync {
    fn id(&self) -> &str;
    fn work_dir(&self) -> &str;

    async fn get_status(&self) -> Result<SessionStatus, PromptSessionError>;
    async fn set_model(&self, model: &str) -> Result<(), PromptSessionError>;
    async fn set_permission(&self, mode: PermissionMode) -> Result<(), PromptSessionError>;
    fn set_approval_handler(&self, handler: Option<ApprovalHandler>);
    fn set_question_handler(&self, handler: Option<QuestionHandler>);
    fn on_event(&self, listener: EventListener) -> Unsubscribe;
    async fn prompt(&self, input: PromptInput) -> Result<(), PromptSessionError>;
    async fn wait_for_background_tasks_on_print(&self) -> Result<(), PromptSessionError>;
    async fn handle_print_main_turn_completed(&self)
    -> Result<PrintTurnAction, PromptSessionError>;
    async fn create_goal(&self, input: CreateGoalInput)
    -> Result<GoalSnapshot, PromptSessionError>;
    async fn get_goal(&self) -> Result<Option<GoalSnapshot>, PromptSessionError>;
    async fn get_cron_tasks(&self) -> Result<Vec<CronTaskSnapshot>, PromptSessionError>;
}

// Original: PromptHarness
#[async_trait]
pub trait PromptHarness: Send + Sync {
    fn home_dir(&self) -> &str;
    fn track(&self, event: &str, properties: Option<&TelemetryProperties>);

    async fn ensure_config_file(&self) -> Result<(), PromptSessionError>;
    async fn get_config(&self) -> Result<PromptConfig, PromptSessionError>;
    async fn get_config_diagnostics(&self) -> Result<ConfigDiagnostics, PromptSessionError>;
    async fn list_sessions(
        &self,
        options: ListSessionsOptions,
    ) -> Result<Vec<SessionSummary>, PromptSessionError>;
    async fn create_session(
        &self,
        options: CreateSessionOptions,
    ) -> Result<Arc<dyn PromptSession>, PromptSessionError>;
    async fn resume_session(
        &self,
        input: ResumeSessionInput,
    ) -> Result<Arc<dyn PromptSession>, PromptSessionError>;
    async fn close(&self) -> Result<(), PromptSessionError>;
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    struct SessionMock {
        permissions: Mutex<Vec<PermissionMode>>,
    }

    #[async_trait]
    impl PromptSession for SessionMock {
        fn id(&self) -> &str {
            "ses_1"
        }
        fn work_dir(&self) -> &str {
            "/work"
        }

        async fn get_status(&self) -> Result<SessionStatus, PromptSessionError> {
            Ok(SessionStatus {
                model: Some("k2".to_owned()),
                thinking_effort: "on".to_owned(),
                permission: PermissionMode::Manual,
                plan_mode: false,
                swarm_mode: None,
                context_tokens: 0,
                max_context_tokens: 0,
                context_usage: 0.0,
                usage: None,
            })
        }
        async fn set_model(&self, _: &str) -> Result<(), PromptSessionError> {
            Ok(())
        }
        async fn set_permission(&self, mode: PermissionMode) -> Result<(), PromptSessionError> {
            self.permissions.lock().expect("permissions").push(mode);
            Ok(())
        }
        fn set_approval_handler(&self, _: Option<ApprovalHandler>) {}
        fn set_question_handler(&self, _: Option<QuestionHandler>) {}
        fn on_event(&self, _: EventListener) -> Unsubscribe {
            Box::new(|| {})
        }
        async fn prompt(&self, _: PromptInput) -> Result<(), PromptSessionError> {
            Ok(())
        }
        async fn wait_for_background_tasks_on_print(&self) -> Result<(), PromptSessionError> {
            Ok(())
        }
        async fn handle_print_main_turn_completed(
            &self,
        ) -> Result<PrintTurnAction, PromptSessionError> {
            Ok(PrintTurnAction::Finish)
        }
        async fn create_goal(
            &self,
            _: CreateGoalInput,
        ) -> Result<GoalSnapshot, PromptSessionError> {
            Err(std::io::Error::other("unused").into())
        }
        async fn get_goal(&self) -> Result<Option<GoalSnapshot>, PromptSessionError> {
            Ok(None)
        }
        async fn get_cron_tasks(&self) -> Result<Vec<CronTaskSnapshot>, PromptSessionError> {
            Ok(Vec::new())
        }
    }

    #[tokio::test]
    async fn supports_dynamic_dispatch_for_v1_and_v2_session_adapters() {
        let first: Arc<dyn PromptSession> = Arc::new(SessionMock {
            permissions: Mutex::new(Vec::new()),
        });
        let second: Arc<dyn PromptSession> = Arc::new(SessionMock {
            permissions: Mutex::new(Vec::new()),
        });
        for session in [first, second] {
            assert_eq!(session.id(), "ses_1");
            assert_eq!(
                session.get_status().await.expect("status").model.as_deref(),
                Some("k2")
            );
            session
                .set_permission(PermissionMode::Auto)
                .await
                .expect("permission");
        }
    }

    #[test]
    fn exposes_turn_ids_for_only_turn_scoped_events() {
        assert_eq!(
            PromptEventKind::AssistantDelta {
                turn_id: 7,
                delta: "x".to_owned()
            }
            .turn_id(),
            Some(7)
        );
        assert_eq!(
            PromptEventKind::GoalUpdated {
                snapshot: None,
                completion: false
            }
            .turn_id(),
            None
        );
    }
}
