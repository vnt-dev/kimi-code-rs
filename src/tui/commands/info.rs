use std::collections::BTreeMap;
use std::{error::Error, fmt};

use async_trait::async_trait;

use crate::{
    oauth::{
        managed_feedback::{FetchSubmitFeedbackResult, SubmitFeedbackBody},
        managed_usage::{ParsedManagedUsage, is_managed_kimi_code},
    },
    sdk::{
        model_alias::ModelAlias,
        types::{
            McpServerStatusSnapshot, PermissionMode, SessionStatus, SessionUsage, ThinkingEffort,
        },
    },
    tui::{
        commands::prompts::{
            FeedbackAttachmentLevel, PromptHost, prompt_feedback_attachment, prompt_feedback_input,
        },
        components::{
            Component,
            messages::{
                mcp_status_panel::build_mcp_status_report_lines,
                status_panel::{StatusReportOptions, build_status_report_lines},
                usage_panel::{UsagePanelComponent, UsageReportOptions, build_usage_report_lines},
            },
        },
        constant::feedback::{
            FEEDBACK_ISSUE_URL, FEEDBACK_STATUS_CANCELLED, FEEDBACK_STATUS_FALLBACK,
            FEEDBACK_STATUS_NETWORK_ERROR, FEEDBACK_STATUS_NOT_SIGNED_IN,
            FEEDBACK_STATUS_SUBMITTING, FEEDBACK_STATUS_SUCCESS, FEEDBACK_STATUS_UPLOAD_FAILED,
            FEEDBACK_TELEMETRY_EVENT, feedback_id_line, feedback_session_line,
            with_feedback_version_prefix,
        },
    },
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeedbackCommandState {
    pub version: String,
    pub model: String,
    pub session_id: String,
    pub provider_key: Option<String>,
    pub os: String,
}

#[derive(Debug)]
pub struct FeedbackCommandHostError(Box<dyn Error + Send + Sync>);

impl FeedbackCommandHostError {
    pub fn new(error: impl Error + Send + Sync + 'static) -> Self {
        Self(Box::new(error))
    }
}

impl fmt::Display for FeedbackCommandHostError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl Error for FeedbackCommandHostError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(self.0.as_ref())
    }
}

#[async_trait(?Send)]
pub trait FeedbackCommandHost: PromptHost {
    fn feedback_command_state(&self) -> FeedbackCommandState;
    fn show_status(&mut self, message: &str);
    fn open_url(&mut self, url: &str);
    fn start_feedback_spinner(&mut self, label: &str);
    fn stop_feedback_spinner(&mut self, ok: bool, label: &str);
    fn track(&mut self, event: &str);

    async fn submit_feedback(
        &mut self,
        body: SubmitFeedbackBody,
    ) -> Result<FetchSubmitFeedbackResult, FeedbackCommandHostError>;

    async fn submit_feedback_attachments(
        &mut self,
        feedback_id: f64,
        level: FeedbackAttachmentLevel,
    ) -> Result<bool, FeedbackCommandHostError>;
}

// Original: `src/tui/commands/info.ts`, `handleFeedbackCommand()`.
pub async fn handle_feedback_command(
    host: &mut impl FeedbackCommandHost,
) -> Result<(), FeedbackCommandHostError> {
    let state = host.feedback_command_state();
    if !is_managed_kimi_code(state.provider_key.as_deref()) {
        feedback_fallback(host, FEEDBACK_STATUS_NOT_SIGNED_IN);
        return Ok(());
    }

    let Some(input) = prompt_feedback_input(host).await else {
        host.show_status(FEEDBACK_STATUS_CANCELLED);
        return Ok(());
    };
    let Some(level) = prompt_feedback_attachment(host).await else {
        host.show_status(FEEDBACK_STATUS_CANCELLED);
        return Ok(());
    };

    host.start_feedback_spinner(FEEDBACK_STATUS_SUBMITTING);
    let result = async {
        let response = host
            .submit_feedback(SubmitFeedbackBody {
                session_id: state.session_id.clone(),
                content: input.value,
                version: with_feedback_version_prefix(&state.version),
                os: state.os,
                model: (!state.model.is_empty()).then_some(state.model),
                contact: None,
                info: None,
            })
            .await?;
        let feedback_id = match response {
            FetchSubmitFeedbackResult::Ok { feedback_id } => feedback_id,
            FetchSubmitFeedbackResult::Error { message, .. } => {
                return Ok(FeedbackCommandOutcome::ApiError(message));
            }
        };
        let attachment_failed = host.submit_feedback_attachments(feedback_id, level).await?;
        Ok(FeedbackCommandOutcome::Submitted {
            feedback_id,
            attachment_failed,
        })
    }
    .await;

    match result {
        Ok(FeedbackCommandOutcome::ApiError(message)) => {
            host.stop_feedback_spinner(false, &message);
            feedback_fallback(host, FEEDBACK_STATUS_FALLBACK);
            Ok(())
        }
        Ok(FeedbackCommandOutcome::Submitted {
            feedback_id,
            attachment_failed,
        }) => {
            host.stop_feedback_spinner(true, FEEDBACK_STATUS_SUCCESS);
            host.show_status(&feedback_session_line(&state.session_id));
            host.show_status(&feedback_id_line(feedback_id));
            host.track(FEEDBACK_TELEMETRY_EVENT);
            if attachment_failed {
                host.show_status(FEEDBACK_STATUS_UPLOAD_FAILED);
            }
            Ok(())
        }
        Err(error) => {
            host.stop_feedback_spinner(false, FEEDBACK_STATUS_NETWORK_ERROR);
            Err(error)
        }
    }
}

enum FeedbackCommandOutcome {
    ApiError(String),
    Submitted {
        feedback_id: f64,
        attachment_failed: bool,
    },
}

fn feedback_fallback(host: &mut impl FeedbackCommandHost, reason: &str) {
    host.show_status(reason);
    host.show_status(FEEDBACK_ISSUE_URL);
    host.open_url(FEEDBACK_ISSUE_URL);
}

#[derive(Debug, Clone, PartialEq)]
pub struct InfoAppState {
    pub version: String,
    pub model: String,
    pub work_dir: String,
    pub session_id: String,
    pub session_title: Option<String>,
    pub thinking_effort: ThinkingEffort,
    pub permission_mode: PermissionMode,
    pub plan_mode: bool,
    pub context_usage: f64,
    pub context_tokens: u64,
    pub max_context_tokens: u64,
    pub available_models: BTreeMap<String, ModelAlias>,
}

#[async_trait(?Send)]
pub trait InfoCommandHost {
    fn info_app_state(&self) -> InfoAppState;
    async fn get_session_usage(&self) -> Result<SessionUsage, String>;
    async fn get_runtime_status(&self) -> Result<SessionStatus, String>;
    async fn get_managed_usage(&self, provider_key: &str) -> Result<ParsedManagedUsage, String>;
    async fn list_mcp_servers(&self) -> Result<Vec<McpServerStatusSnapshot>, String>;
    fn add_transcript_component(&mut self, component: Box<dyn Component>);
    fn request_render(&mut self);
    fn show_error(&mut self, message: &str);
}

#[derive(Debug)]
struct ReportResult<T> {
    value: Option<T>,
    error: Option<String>,
}

impl<T> ReportResult<T> {
    fn from_result(result: Result<T, String>) -> Self {
        match result {
            Ok(value) => Self {
                value: Some(value),
                error: None,
            },
            Err(error) => Self {
                value: None,
                error: Some(error),
            },
        }
    }
}

fn provider_key(state: &InfoAppState) -> Option<&str> {
    state
        .available_models
        .get(&state.model)
        .map(|model| model.provider.as_str())
}

async fn load_managed_usage_report(
    host: &impl InfoCommandHost,
    state: &InfoAppState,
) -> Option<ReportResult<ParsedManagedUsage>> {
    let provider_key = provider_key(state).filter(|key| is_managed_kimi_code(Some(key)))?;
    Some(ReportResult::from_result(
        host.get_managed_usage(provider_key).await,
    ))
}

// Original: `src/tui/commands/info.ts`, `showUsage()`.
pub async fn show_usage(host: &mut impl InfoCommandHost) {
    let state = host.info_app_state();
    let session_usage = ReportResult::from_result(host.get_session_usage().await);
    let managed_usage = load_managed_usage_report(host, &state).await;
    let lines = build_usage_report_lines(UsageReportOptions {
        session_usage: session_usage.value.as_ref(),
        session_usage_error: session_usage.error.as_deref(),
        context_usage: state.context_usage,
        context_tokens: state.context_tokens,
        max_context_tokens: state.max_context_tokens,
        managed_usage: managed_usage
            .as_ref()
            .and_then(|result| result.value.as_ref()),
        managed_usage_error: managed_usage
            .as_ref()
            .and_then(|result| result.error.as_deref()),
    });
    host.add_transcript_component(Box::new(UsagePanelComponent::usage(move || lines.clone())));
    host.request_render();
}

// Original: `showStatusReport()`.
pub async fn show_status_report(host: &mut impl InfoCommandHost) {
    let state = host.info_app_state();
    let (runtime_status, managed_usage) = tokio::join!(
        async { ReportResult::from_result(host.get_runtime_status().await) },
        load_managed_usage_report(host, &state),
    );
    let lines = build_status_report_lines(StatusReportOptions {
        version: &state.version,
        model: &state.model,
        work_dir: &state.work_dir,
        session_id: &state.session_id,
        session_title: state.session_title.as_deref(),
        thinking_effort: &state.thinking_effort,
        permission_mode: state.permission_mode,
        plan_mode: state.plan_mode,
        context_usage: state.context_usage,
        context_tokens: state.context_tokens,
        max_context_tokens: state.max_context_tokens,
        available_models: &state.available_models,
        status: runtime_status.value.as_ref(),
        status_error: runtime_status.error.as_deref(),
        managed_usage: managed_usage
            .as_ref()
            .and_then(|result| result.value.as_ref()),
        managed_usage_error: managed_usage
            .as_ref()
            .and_then(|result| result.error.as_deref()),
    });
    host.add_transcript_component(Box::new(UsagePanelComponent::new(
        move || lines.clone(),
        crate::tui::theme::ColorToken::Primary,
        " Status ",
    )));
    host.request_render();
}

// Original: `showMcpServers()`.
pub async fn show_mcp_servers(host: &mut impl InfoCommandHost) {
    let servers = match host.list_mcp_servers().await {
        Ok(servers) => servers,
        Err(error) => {
            host.show_error(&format!("Failed to load MCP servers: {error}"));
            return;
        }
    };
    let title = if servers.is_empty() {
        " MCP ".to_owned()
    } else {
        format!(" MCP ({}) ", servers.len())
    };
    let lines = build_mcp_status_report_lines(&servers);
    host.add_transcript_component(Box::new(UsagePanelComponent::new(
        move || lines.clone(),
        crate::tui::theme::ColorToken::Primary,
        title,
    )));
    host.request_render();
}

#[cfg(test)]
mod tests {
    use std::{collections::VecDeque, sync::Mutex};

    use crate::{
        oauth::managed_usage::UsageRow,
        sdk::{model_alias::ModelProtocol, types::McpServerTransport},
    };

    use super::*;

    struct Host {
        state: InfoAppState,
        usage: Result<SessionUsage, String>,
        status: Result<SessionStatus, String>,
        managed: Result<ParsedManagedUsage, String>,
        mcp: Result<Vec<McpServerStatusSnapshot>, String>,
        panels: Vec<Box<dyn Component>>,
        renders: usize,
        errors: Vec<String>,
        managed_calls: Mutex<Vec<String>>,
    }

    #[async_trait(?Send)]
    impl InfoCommandHost for Host {
        fn info_app_state(&self) -> InfoAppState {
            self.state.clone()
        }

        async fn get_session_usage(&self) -> Result<SessionUsage, String> {
            self.usage.clone()
        }

        async fn get_runtime_status(&self) -> Result<SessionStatus, String> {
            self.status.clone()
        }

        async fn get_managed_usage(
            &self,
            provider_key: &str,
        ) -> Result<ParsedManagedUsage, String> {
            self.managed_calls
                .lock()
                .expect("managed calls")
                .push(provider_key.to_owned());
            self.managed.clone()
        }

        async fn list_mcp_servers(&self) -> Result<Vec<McpServerStatusSnapshot>, String> {
            self.mcp.clone()
        }

        fn add_transcript_component(&mut self, component: Box<dyn Component>) {
            self.panels.push(component);
        }

        fn request_render(&mut self) {
            self.renders += 1;
        }

        fn show_error(&mut self, message: &str) {
            self.errors.push(message.to_owned());
        }
    }

    fn alias(provider: &str) -> ModelAlias {
        ModelAlias {
            provider: provider.to_owned(),
            model: "model".to_owned(),
            max_context_size: 128_000,
            max_output_size: None,
            capabilities: None,
            display_name: Some("Model".to_owned()),
            reasoning_key: None,
            protocol: Some(ModelProtocol::Anthropic),
            adaptive_thinking: None,
            support_efforts: None,
            default_effort: None,
            beta_api: None,
            overrides: None,
        }
    }

    fn host(provider: &str) -> Host {
        Host {
            state: InfoAppState {
                version: "1.2.3".to_owned(),
                model: "alias".to_owned(),
                work_dir: "C:/repo".to_owned(),
                session_id: "session-1".to_owned(),
                session_title: Some("Title".to_owned()),
                thinking_effort: ThinkingEffort::from("on"),
                permission_mode: PermissionMode::Manual,
                plan_mode: false,
                context_usage: 0.25,
                context_tokens: 32_000,
                max_context_tokens: 128_000,
                available_models: BTreeMap::from([("alias".to_owned(), alias(provider))]),
            },
            usage: Ok(SessionUsage::default()),
            status: Err("status unavailable".to_owned()),
            managed: Ok(ParsedManagedUsage {
                summary: Some(UsageRow {
                    label: "Weekly limit".to_owned(),
                    used: 3.0,
                    limit: 10.0,
                    reset_hint: None,
                }),
                limits: Vec::new(),
                extra_usage: None,
            }),
            mcp: Ok(Vec::new()),
            panels: Vec::new(),
            renders: 0,
            errors: Vec::new(),
            managed_calls: Mutex::new(Vec::new()),
        }
    }

    fn rendered_panel(host: &mut Host) -> String {
        host.panels[0].render(100).join("\n")
    }

    #[tokio::test]
    async fn usage_loads_managed_plan_for_managed_provider() {
        let mut host = host("managed:kimi-code");
        show_usage(&mut host).await;
        assert_eq!(
            *host.managed_calls.lock().expect("managed calls"),
            ["managed:kimi-code"]
        );
        assert_eq!((host.panels.len(), host.renders), (1, 1));
        let output = rendered_panel(&mut host);
        assert!(output.contains("Session usage"));
        assert!(output.contains("Weekly limit"));
    }

    #[tokio::test]
    async fn status_skips_managed_usage_for_other_provider_and_keeps_errors() {
        let mut host = host("openai");
        show_status_report(&mut host).await;
        assert!(host.managed_calls.lock().expect("managed calls").is_empty());
        let output = rendered_panel(&mut host);
        assert!(output.contains("status unavailable"));
        assert!(output.contains("v1.2.3"));
    }

    #[tokio::test]
    async fn mcp_error_is_reported_without_panel_or_render() {
        let mut host = host("openai");
        host.mcp = Err("session closed".to_owned());
        show_mcp_servers(&mut host).await;
        assert_eq!(host.errors, ["Failed to load MCP servers: session closed"]);
        assert!(host.panels.is_empty());
        assert_eq!(host.renders, 0);
    }

    #[tokio::test]
    async fn mcp_panel_title_includes_server_count() {
        let mut host = host("openai");
        host.mcp = Ok(vec![McpServerStatusSnapshot {
            name: "tools".to_owned(),
            transport: McpServerTransport::Stdio,
            status: crate::sdk::types::McpServerStatus::Connected,
            tool_count: 3,
            error: None,
        }]);
        show_mcp_servers(&mut host).await;
        let output = rendered_panel(&mut host);
        assert!(output.contains("MCP (1)"));
        assert!(output.contains("tools"));
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum FeedbackFailure {
        None,
        Submit,
        Attachments,
    }

    #[derive(Debug, Clone, Copy)]
    struct FeedbackTestError(&'static str);

    impl fmt::Display for FeedbackTestError {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str(self.0)
        }
    }

    impl Error for FeedbackTestError {}

    struct FeedbackHost {
        state: FeedbackCommandState,
        prompt_inputs: VecDeque<Vec<String>>,
        response: FetchSubmitFeedbackResult,
        attachment_failed: bool,
        failure: FeedbackFailure,
        submitted: Vec<SubmitFeedbackBody>,
        attachment_calls: Vec<(f64, FeedbackAttachmentLevel)>,
        statuses: Vec<String>,
        spinner: Vec<(bool, String)>,
        opened: Vec<String>,
        tracked: Vec<String>,
        restored: usize,
    }

    impl FeedbackHost {
        fn managed() -> Self {
            Self {
                state: FeedbackCommandState {
                    version: "1.2.3".to_owned(),
                    model: "kimi/model".to_owned(),
                    session_id: "ses-1".to_owned(),
                    provider_key: Some("managed:kimi-code".to_owned()),
                    os: "Windows_NT 10.0.26100".to_owned(),
                },
                prompt_inputs: VecDeque::from([
                    vec!["useful feedback".to_owned(), "\r".to_owned()],
                    vec!["\r".to_owned()],
                ]),
                response: FetchSubmitFeedbackResult::Ok { feedback_id: 3.0 },
                attachment_failed: false,
                failure: FeedbackFailure::None,
                submitted: Vec::new(),
                attachment_calls: Vec::new(),
                statuses: Vec::new(),
                spinner: Vec::new(),
                opened: Vec::new(),
                tracked: Vec::new(),
                restored: 0,
            }
        }
    }

    impl PromptHost for FeedbackHost {
        fn mount_editor_replacement(&mut self, mut component: Box<dyn Component>) {
            for input in self.prompt_inputs.pop_front().expect("prompt input") {
                component.handle_input(&input);
            }
        }

        fn restore_editor(&mut self) {
            self.restored += 1;
        }

        fn show_error(&mut self, message: &str) {
            self.statuses.push(format!("error:{message}"));
        }
    }

    #[async_trait(?Send)]
    impl FeedbackCommandHost for FeedbackHost {
        fn feedback_command_state(&self) -> FeedbackCommandState {
            self.state.clone()
        }

        fn show_status(&mut self, message: &str) {
            self.statuses.push(message.to_owned());
        }

        fn open_url(&mut self, url: &str) {
            self.opened.push(url.to_owned());
        }

        fn start_feedback_spinner(&mut self, label: &str) {
            self.spinner.push((true, label.to_owned()));
        }

        fn stop_feedback_spinner(&mut self, ok: bool, label: &str) {
            self.spinner.push((ok, label.to_owned()));
        }

        fn track(&mut self, event: &str) {
            self.tracked.push(event.to_owned());
        }

        async fn submit_feedback(
            &mut self,
            body: SubmitFeedbackBody,
        ) -> Result<FetchSubmitFeedbackResult, FeedbackCommandHostError> {
            self.submitted.push(body);
            if self.failure == FeedbackFailure::Submit {
                Err(FeedbackCommandHostError::new(FeedbackTestError(
                    "network down",
                )))
            } else {
                Ok(self.response.clone())
            }
        }

        async fn submit_feedback_attachments(
            &mut self,
            feedback_id: f64,
            level: FeedbackAttachmentLevel,
        ) -> Result<bool, FeedbackCommandHostError> {
            self.attachment_calls.push((feedback_id, level));
            if self.failure == FeedbackFailure::Attachments {
                Err(FeedbackCommandHostError::new(FeedbackTestError(
                    "attachment task failed",
                )))
            } else {
                Ok(self.attachment_failed)
            }
        }
    }

    #[tokio::test]
    async fn feedback_requires_managed_sign_in_and_opens_issue_fallback() {
        let mut host = FeedbackHost::managed();
        host.state.provider_key = Some("openai".to_owned());

        handle_feedback_command(&mut host).await.expect("fallback");

        assert_eq!(
            host.statuses,
            [FEEDBACK_STATUS_NOT_SIGNED_IN, FEEDBACK_ISSUE_URL]
        );
        assert_eq!(host.opened, [FEEDBACK_ISSUE_URL]);
        assert!(host.submitted.is_empty());
        assert_eq!(host.restored, 0);
    }

    #[tokio::test]
    async fn feedback_input_or_attachment_cancellation_stops_before_submission() {
        let mut input_cancelled = FeedbackHost::managed();
        input_cancelled.prompt_inputs = VecDeque::from([vec!["\u{1b}".to_owned()]]);
        handle_feedback_command(&mut input_cancelled)
            .await
            .expect("input cancellation");
        assert_eq!(input_cancelled.statuses, [FEEDBACK_STATUS_CANCELLED]);
        assert!(input_cancelled.submitted.is_empty());

        let mut attachment_cancelled = FeedbackHost::managed();
        attachment_cancelled.prompt_inputs = VecDeque::from([
            vec!["text".to_owned(), "\r".to_owned()],
            vec!["\u{1b}".to_owned()],
        ]);
        handle_feedback_command(&mut attachment_cancelled)
            .await
            .expect("attachment cancellation");
        assert_eq!(attachment_cancelled.statuses, [FEEDBACK_STATUS_CANCELLED]);
        assert!(attachment_cancelled.submitted.is_empty());
    }

    #[tokio::test]
    async fn successful_feedback_submits_exact_context_then_reports_and_tracks() {
        let mut host = FeedbackHost::managed();

        handle_feedback_command(&mut host).await.expect("feedback");

        assert_eq!(
            host.submitted,
            [SubmitFeedbackBody {
                session_id: "ses-1".to_owned(),
                content: "useful feedback".to_owned(),
                version: "kimi-code-1.2.3".to_owned(),
                os: "Windows_NT 10.0.26100".to_owned(),
                model: Some("kimi/model".to_owned()),
                contact: None,
                info: None,
            }]
        );
        assert_eq!(
            host.attachment_calls,
            [(3.0, FeedbackAttachmentLevel::None)]
        );
        assert_eq!(
            host.spinner,
            [
                (true, FEEDBACK_STATUS_SUBMITTING.to_owned()),
                (true, FEEDBACK_STATUS_SUCCESS.to_owned()),
            ]
        );
        assert_eq!(host.statuses, ["Session: ses-1", "Feedback ID: 3"]);
        assert_eq!(host.tracked, [FEEDBACK_TELEMETRY_EVENT]);
        assert_eq!(host.restored, 2);
    }

    #[tokio::test]
    async fn feedback_api_error_keeps_backend_label_then_uses_fallback() {
        let mut host = FeedbackHost::managed();
        host.response = FetchSubmitFeedbackResult::Error {
            status: Some(400),
            message: "feedback rejected".to_owned(),
        };

        handle_feedback_command(&mut host)
            .await
            .expect("handled API error");

        assert_eq!(
            host.spinner,
            [
                (true, FEEDBACK_STATUS_SUBMITTING.to_owned()),
                (false, "feedback rejected".to_owned()),
            ]
        );
        assert_eq!(
            host.statuses,
            [FEEDBACK_STATUS_FALLBACK, FEEDBACK_ISSUE_URL]
        );
        assert_eq!(host.opened, [FEEDBACK_ISSUE_URL]);
        assert!(host.attachment_calls.is_empty());
    }

    #[tokio::test]
    async fn partial_attachment_failure_keeps_feedback_success_and_adds_warning() {
        let mut host = FeedbackHost::managed();
        host.attachment_failed = true;
        host.prompt_inputs = VecDeque::from([
            vec!["text".to_owned(), "\r".to_owned()],
            vec!["\u{1b}[B".to_owned(), "\r".to_owned()],
        ]);

        handle_feedback_command(&mut host).await.expect("partial");

        assert_eq!(
            host.attachment_calls,
            [(3.0, FeedbackAttachmentLevel::Logs)]
        );
        assert_eq!(
            host.statuses,
            [
                "Session: ses-1",
                "Feedback ID: 3",
                FEEDBACK_STATUS_UPLOAD_FAILED
            ]
        );
        assert_eq!(host.tracked, [FEEDBACK_TELEMETRY_EVENT]);
    }

    #[tokio::test]
    async fn thrown_submission_or_attachment_error_stops_spinner_and_propagates() {
        for failure in [FeedbackFailure::Submit, FeedbackFailure::Attachments] {
            let mut host = FeedbackHost::managed();
            host.failure = failure;
            let error = handle_feedback_command(&mut host)
                .await
                .expect_err("propagated error");
            assert!(matches!(
                error.to_string().as_str(),
                "network down" | "attachment task failed"
            ));
            assert_eq!(
                host.spinner.last(),
                Some(&(false, FEEDBACK_STATUS_NETWORK_ERROR.to_owned()))
            );
            assert!(host.tracked.is_empty());
        }
    }
}
