use std::{
    error::Error,
    fmt,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use tokio::{
    sync::mpsc,
    task::{JoinError, JoinHandle},
};

use super::{
    options::PromptOutputFormat,
    prompt_render::{PromptJsonWriter, PromptOutput, PromptTranscriptWriter, PromptTurnWriter},
    prompt_session::{
        PrintTurnAction, PromptEvent, PromptEventKind, PromptInput, PromptSession,
        PromptSessionError,
    },
};

pub const PROMPT_CLEANUP_TIMEOUT: Duration = Duration::from_millis(8_000);

#[derive(Debug)]
pub enum CleanupTaskError<E> {
    Cleanup(E),
    Join(JoinError),
}

impl<E: fmt::Display> fmt::Display for CleanupTaskError<E> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Cleanup(error) => write!(formatter, "cleanup failed: {error}"),
            Self::Join(error) => write!(formatter, "cleanup task failed: {error}"),
        }
    }
}

impl<E> Error for CleanupTaskError<E>
where
    E: Error + 'static,
{
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Cleanup(error) => Some(error),
            Self::Join(error) => Some(error),
        }
    }
}

// Original:
//   apps/kimi-code/src/cli/run-prompt.ts
//   raceWithTimeout()
//
// Rust adaptation:
//   Cleanup is supplied as a spawned task. Dropping a Tokio JoinHandle detaches
//   rather than cancels the task, matching the original Promise continuing
//   after the caller gives up waiting. A result that arrives in time is still
//   propagated exactly.
pub async fn race_with_timeout<E>(
    mut cleanup: JoinHandle<Result<(), E>>,
    timeout: Duration,
) -> Result<(), CleanupTaskError<E>> {
    tokio::select! {
        biased;
        result = &mut cleanup => {
            result.map_err(CleanupTaskError::Join)?.map_err(CleanupTaskError::Cleanup)
        }
        () = tokio::time::sleep(timeout) => Ok(()),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelNotConfiguredError;

impl fmt::Display for ModelNotConfiguredError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(
            "No model configured. Run `kimi` and use /login to sign in, then retry; or set default_model in config.toml.",
        )
    }
}

impl Error for ModelNotConfiguredError {}

// Original: configuredModel()
pub fn configured_model<'a>(models: impl IntoIterator<Item = Option<&'a str>>) -> Option<&'a str> {
    models
        .into_iter()
        .flatten()
        .find(|model| !model.trim().is_empty())
}

// Original: requireConfiguredModel()
pub fn require_configured_model<'a>(
    models: impl IntoIterator<Item = Option<&'a str>>,
) -> Result<&'a str, ModelNotConfiguredError> {
    configured_model(models).ok_or(ModelNotConfiguredError)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminationSignal {
    Sigint,
    Sighup,
    Sigterm,
}

// Original: signalExitCode()
pub const fn signal_exit_code(signal: TerminationSignal) -> i32 {
    match signal {
        TerminationSignal::Sigint => 130,
        TerminationSignal::Sighup => 129,
        TerminationSignal::Sigterm => 143,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnErrorPayload {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnEndReason {
    Completed,
    Cancelled,
    Failed,
    Blocked,
}

impl TurnEndReason {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::Cancelled => "cancelled",
            Self::Failed => "failed",
            Self::Blocked => "blocked",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnEndedFailure {
    pub reason: TurnEndReason,
    pub error: Option<TurnErrorPayload>,
}

// Original: formatTurnEndedFailure()
pub fn format_turn_ended_failure(event: &TurnEndedFailure) -> String {
    if event
        .error
        .as_ref()
        .is_some_and(|error| error.code == "provider.filtered")
    {
        return "Provider safety policy blocked the response.".to_owned();
    }
    if let Some(error) = &event.error {
        return format!("{}: {}", error.code, error.message);
    }
    if event.reason == TurnEndReason::Blocked {
        return "Prompt hook blocked the request.".to_owned();
    }
    format!("Prompt turn ended with reason: {}", event.reason.as_str())
}

const PROMPT_MAIN_AGENT_ID: &str = "main";

enum RunPromptWriter<'a> {
    Text(PromptTranscriptWriter<'a>),
    Json {
        writer: PromptJsonWriter<'a>,
        stderr: &'a mut dyn PromptOutput,
    },
}

impl RunPromptWriter<'_> {
    fn write_assistant_delta(&mut self, delta: &str) {
        match self {
            Self::Text(writer) => writer.write_assistant_delta(delta),
            Self::Json { writer, .. } => writer.write_assistant_delta(delta),
        }
    }

    fn write_hook_result(&mut self, event: &super::prompt_render::HookResultEvent) {
        match self {
            Self::Text(writer) => writer.write_hook_result(event),
            Self::Json { writer, .. } => writer.write_hook_result(event),
        }
    }

    fn write_thinking_delta(&mut self, delta: &str) {
        match self {
            Self::Text(writer) => writer.write_thinking_delta(delta),
            Self::Json { writer, .. } => writer.write_thinking_delta(delta),
        }
    }

    fn write_tool_call(&mut self, id: &str, name: &str, args: &super::prompt_render::PromptValue) {
        match self {
            Self::Text(writer) => writer.write_tool_call(id, name, args),
            Self::Json { writer, .. } => writer.write_tool_call(id, name, args),
        }
    }

    fn write_tool_call_delta(&mut self, id: &str, name: Option<&str>, arguments: Option<&str>) {
        match self {
            Self::Text(writer) => writer.write_tool_call_delta(id, name, arguments),
            Self::Json { writer, .. } => writer.write_tool_call_delta(id, name, arguments),
        }
    }

    fn write_tool_result(&mut self, id: &str, output: &super::prompt_render::PromptValue) {
        match self {
            Self::Text(writer) => writer.write_tool_result(id, output),
            Self::Json { writer, .. } => writer.write_tool_result(id, output),
        }
    }

    fn write_retrying(&mut self, event: &super::prompt_render::RetryingEvent) {
        match self {
            Self::Text(writer) => writer.write_retrying(event),
            Self::Json { writer, .. } => writer.write_retrying(event),
        }
    }

    fn write_progress(&mut self, text: &str) {
        let terminated = if text.ends_with('\n') {
            text.to_owned()
        } else {
            format!("{text}\n")
        };
        match self {
            Self::Text(writer) => writer.write_raw_stderr(&terminated),
            Self::Json { stderr, .. } => {
                stderr.write(&terminated);
            }
        }
    }

    fn flush_assistant(&mut self) {
        match self {
            Self::Text(writer) => writer.flush_assistant(),
            Self::Json { writer, .. } => writer.flush_assistant(),
        }
    }

    fn discard_assistant(&mut self) {
        match self {
            Self::Text(writer) => writer.discard_assistant(),
            Self::Json { writer, .. } => writer.discard_assistant(),
        }
    }

    fn finish(&mut self) {
        match self {
            Self::Text(writer) => writer.finish(),
            Self::Json { writer, .. } => writer.finish(),
        }
    }
}

enum CompletionEvaluation {
    Hold,
    Continue,
    Finish,
}

async fn evaluate_run_completion(
    session: Arc<dyn PromptSession>,
    generation: u64,
    current_generation: Arc<AtomicU64>,
) -> (u64, Result<CompletionEvaluation, PromptSessionError>) {
    let result = async {
        if session
            .get_goal()
            .await?
            .is_some_and(|goal| goal.status.as_str() == "active")
        {
            return Ok(CompletionEvaluation::Hold);
        }
        if current_generation.load(Ordering::SeqCst) != generation {
            return Ok(CompletionEvaluation::Hold);
        }
        if session
            .get_cron_tasks()
            .await?
            .iter()
            .any(|task| task.next_fire_at.is_some())
        {
            return Ok(CompletionEvaluation::Hold);
        }
        if current_generation.load(Ordering::SeqCst) != generation {
            return Ok(CompletionEvaluation::Hold);
        }
        // The original logs and ignores failures from this policy hook. At this
        // boundary, a failure therefore has the same outcome as `finish`.
        Ok(match session.handle_print_main_turn_completed().await {
            Ok(PrintTurnAction::Continue) => CompletionEvaluation::Continue,
            Ok(PrintTurnAction::Finish) | Err(_) => CompletionEvaluation::Finish,
        })
    }
    .await;
    (generation, result)
}

// Original:
//   apps/kimi-code/src/cli/run-prompt.ts
//   runPromptTurn()
pub async fn run_prompt_turn(
    session: Arc<dyn PromptSession>,
    prompt: &str,
    output_format: PromptOutputFormat,
    stdout: &mut dyn PromptOutput,
    stderr: &mut dyn PromptOutput,
) -> Result<(), PromptSessionError> {
    let mut writer = match output_format {
        PromptOutputFormat::Text => {
            RunPromptWriter::Text(PromptTranscriptWriter::new(stdout, stderr))
        }
        PromptOutputFormat::StreamJson => RunPromptWriter::Json {
            writer: PromptJsonWriter::new(stdout),
            stderr,
        },
    };
    let (event_tx, mut event_rx) = mpsc::unbounded_channel::<(PromptEvent, u64)>();
    let generation = Arc::new(AtomicU64::new(0));
    let listener_generation = Arc::clone(&generation);
    let unsubscribe = session.on_event(Arc::new(move |event| {
        let stamp = if event.agent_id == PROMPT_MAIN_AGENT_ID
            && matches!(
                event.kind,
                PromptEventKind::TurnStarted { .. } | PromptEventKind::TurnEnded { .. }
            ) {
            listener_generation.fetch_add(1, Ordering::SeqCst) + 1
        } else {
            listener_generation.load(Ordering::SeqCst)
        };
        let _ = event_tx.send((event, stamp));
    }));

    let prompt_session = Arc::clone(&session);
    let prompt_text = prompt.to_owned();
    let mut prompt_task =
        tokio::spawn(async move { prompt_session.prompt(PromptInput::Text(prompt_text)).await });
    let mut prompt_finished = false;
    let mut active_turn_id = None;
    let mut active_agent_id: Option<String> = None;
    let mut evaluation: Option<
        JoinHandle<(u64, Result<CompletionEvaluation, PromptSessionError>)>,
    > = None;
    let result = loop {
        tokio::select! {
            prompt_result = &mut prompt_task, if !prompt_finished => {
                prompt_finished = true;
                match prompt_result {
                    Ok(Ok(())) => {}
                    Ok(Err(error)) => break Err(error),
                    Err(error) => break Err(Box::new(error)),
                }
            }
            evaluated = async {
                match evaluation.as_mut() {
                    Some(task) => Some(task.await),
                    None => futures_util::future::pending().await,
                }
            } => {
                evaluation = None;
                let Some(evaluated) = evaluated else { continue };
                let (stamp, outcome) = match evaluated {
                    Ok(evaluated) => evaluated,
                    Err(error) => break Err(Box::new(error)),
                };
                if generation.load(Ordering::SeqCst) != stamp || active_turn_id.is_some() {
                    continue;
                }
                let outcome = match outcome {
                    Ok(outcome) => outcome,
                    Err(error) => break Err(error),
                };
                match outcome {
                    CompletionEvaluation::Hold | CompletionEvaluation::Continue => {}
                    CompletionEvaluation::Finish => break Ok(()),
                }
            }
            event = event_rx.recv() => {
                let Some((event, stamp)) = event else {
                    break Err(std::io::Error::other("prompt event stream closed").into());
                };
                if let PromptEventKind::Error { code, message } = &event.kind {
                    if event.agent_id == PROMPT_MAIN_AGENT_ID {
                        break Err(std::io::Error::other(format!("{code}: {message}")).into());
                    }
                    continue;
                }
                if let PromptEventKind::TurnStarted { turn_id } = &event.kind {
                    if event.agent_id == PROMPT_MAIN_AGENT_ID {
                        active_turn_id = Some(*turn_id);
                        active_agent_id = Some(event.agent_id);
                    }
                    continue;
                }
                if let PromptEventKind::GoalUpdated { ref snapshot, .. } = event.kind {
                    if event.agent_id == PROMPT_MAIN_AGENT_ID
                        && active_turn_id.is_none()
                        && snapshot.as_ref().is_some_and(|goal| goal.status.as_str() != "active")
                    {
                        let eval_session = Arc::clone(&session);
                        evaluation = Some(tokio::spawn(evaluate_run_completion(
                            eval_session,
                            stamp,
                            Arc::clone(&generation),
                        )));
                    }
                    continue;
                }
                if active_turn_id.is_none()
                    || active_agent_id.as_deref() != Some(event.agent_id.as_str())
                    || event.kind.turn_id() != active_turn_id
                {
                    continue;
                }
                match event.kind {
                    PromptEventKind::TurnStepStarted { .. }
                    | PromptEventKind::TurnStepInterrupted { .. } => writer.flush_assistant(),
                    PromptEventKind::TurnStepRetrying { event, .. } => {
                        writer.discard_assistant();
                        writer.write_retrying(&event);
                    }
                    PromptEventKind::AssistantDelta { delta, .. } => writer.write_assistant_delta(&delta),
                    PromptEventKind::HookResult { event, .. } => writer.write_hook_result(&event),
                    PromptEventKind::ThinkingDelta { delta, .. } => writer.write_thinking_delta(&delta),
                    PromptEventKind::ToolCallStarted { tool_call_id, name, args, .. } => {
                        writer.write_tool_call(&tool_call_id, &name, &args);
                    }
                    PromptEventKind::ToolCallDelta { tool_call_id, name, arguments_part, .. } => {
                        writer.write_tool_call_delta(&tool_call_id, name.as_deref(), arguments_part.as_deref());
                    }
                    PromptEventKind::ToolResult { tool_call_id, output, .. } => {
                        writer.write_tool_result(&tool_call_id, &output);
                    }
                    PromptEventKind::ToolProgress { text: Some(text), .. } if !text.is_empty() => {
                        writer.write_progress(&text);
                    }
                    PromptEventKind::TurnEnded { reason: TurnEndReason::Completed, .. } => {
                        writer.flush_assistant();
                        active_turn_id = None;
                        active_agent_id = None;
                        let eval_session = Arc::clone(&session);
                        evaluation = Some(tokio::spawn(evaluate_run_completion(
                            eval_session,
                            stamp,
                            Arc::clone(&generation),
                        )));
                    }
                    PromptEventKind::TurnEnded { reason, error, .. } => {
                        break Err(std::io::Error::other(format_turn_ended_failure(&TurnEndedFailure { reason, error })).into());
                    }
                    PromptEventKind::Error { .. }
                    | PromptEventKind::TurnStarted { .. }
                    | PromptEventKind::GoalUpdated { .. }
                    | PromptEventKind::ToolProgress { .. }
                    | PromptEventKind::Ignored { .. } => {}
                }
            }
        }
    };

    unsubscribe();
    writer.finish();
    result
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    };

    use async_trait::async_trait;

    use crate::{
        cli::{
            prompt_render::{PromptOutput, PromptValue, RetryingEvent},
            prompt_session::{
                ApprovalHandler, CreateGoalInput, EventListener, PrintTurnAction, PromptEvent,
                PromptEventKind, PromptInput, PromptSession, PromptSessionError, QuestionHandler,
                Unsubscribe,
            },
        },
        sdk::types::{CronTaskSnapshot, GoalSnapshot, PermissionMode, SessionStatus},
    };

    use super::*;

    #[derive(Default)]
    struct Capture {
        text: String,
    }

    impl PromptOutput for Capture {
        fn write(&mut self, chunk: &str) -> bool {
            self.text.push_str(chunk);
            true
        }
    }

    struct PrintSessionMock {
        listener: Mutex<Option<EventListener>>,
        events: Vec<PromptEvent>,
        prompted: Mutex<Vec<PromptInput>>,
    }

    impl PrintSessionMock {
        fn new(events: Vec<PromptEvent>) -> Self {
            Self {
                listener: Mutex::new(None),
                events,
                prompted: Mutex::new(Vec::new()),
            }
        }
    }

    #[async_trait]
    impl PromptSession for PrintSessionMock {
        fn id(&self) -> &str {
            "ses_prompt"
        }

        fn work_dir(&self) -> &str {
            "/work"
        }

        async fn get_status(&self) -> Result<SessionStatus, PromptSessionError> {
            Err(std::io::Error::other("unused").into())
        }

        async fn set_model(&self, _: &str) -> Result<(), PromptSessionError> {
            Ok(())
        }

        async fn set_permission(&self, _: PermissionMode) -> Result<(), PromptSessionError> {
            Ok(())
        }

        fn set_approval_handler(&self, _: Option<ApprovalHandler>) {}

        fn set_question_handler(&self, _: Option<QuestionHandler>) {}

        fn on_event(&self, listener: EventListener) -> Unsubscribe {
            *self.listener.lock().expect("listener") = Some(listener);
            Box::new(|| {})
        }

        async fn prompt(&self, input: PromptInput) -> Result<(), PromptSessionError> {
            self.prompted.lock().expect("prompts").push(input);
            let listener = self.listener.lock().expect("listener").clone();
            for event in &self.events {
                if let Some(listener) = &listener {
                    listener(event.clone());
                }
                tokio::task::yield_now().await;
            }
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

    fn prompt_event(agent_id: &str, kind: PromptEventKind) -> PromptEvent {
        PromptEvent {
            session_id: "ses_prompt".to_owned(),
            agent_id: agent_id.to_owned(),
            kind,
        }
    }

    #[tokio::test(start_paused = true)]
    async fn timeout_detaches_cleanup_instead_of_cancelling_it() {
        let completed = Arc::new(AtomicBool::new(false));
        let completed_in_task = Arc::clone(&completed);
        let task = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(200)).await;
            completed_in_task.store(true, Ordering::SeqCst);
            Ok::<_, std::io::Error>(())
        });

        let wait = race_with_timeout(task, Duration::from_millis(100));
        tokio::pin!(wait);
        tokio::task::yield_now().await;
        assert!(futures_util::poll!(&mut wait).is_pending());
        tokio::time::advance(Duration::from_millis(100)).await;
        assert!(wait.await.is_ok());
        assert!(!completed.load(Ordering::SeqCst));
        tokio::time::advance(Duration::from_millis(100)).await;
        tokio::task::yield_now().await;
        assert!(completed.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn propagates_cleanup_errors_that_arrive_before_timeout() {
        let task = tokio::spawn(async { Err::<(), _>(std::io::Error::other("close failed")) });
        let error = race_with_timeout(task, Duration::from_secs(1))
            .await
            .expect_err("cleanup failure");
        assert!(error.to_string().contains("close failed"));
    }

    #[test]
    fn chooses_the_first_non_blank_model_without_trimming_it() {
        assert_eq!(
            configured_model([None, Some("  "), Some(" model-a "), Some("model-b")]),
            Some(" model-a ")
        );
        assert_eq!(configured_model([None, Some("\t")]), None);
        assert_eq!(
            require_configured_model([None, Some("")])
                .unwrap_err()
                .to_string(),
            "No model configured. Run `kimi` and use /login to sign in, then retry; or set default_model in config.toml."
        );
    }

    #[test]
    fn maps_process_signals_to_shell_exit_codes() {
        assert_eq!(signal_exit_code(TerminationSignal::Sigint), 130);
        assert_eq!(signal_exit_code(TerminationSignal::Sighup), 129);
        assert_eq!(signal_exit_code(TerminationSignal::Sigterm), 143);
    }

    #[test]
    fn formats_turn_failure_precedence() {
        assert_eq!(
            format_turn_ended_failure(&TurnEndedFailure {
                reason: TurnEndReason::Failed,
                error: Some(TurnErrorPayload {
                    code: "provider.filtered".to_owned(),
                    message: "details".to_owned(),
                }),
            }),
            "Provider safety policy blocked the response."
        );
        assert_eq!(
            format_turn_ended_failure(&TurnEndedFailure {
                reason: TurnEndReason::Failed,
                error: Some(TurnErrorPayload {
                    code: "provider.error".to_owned(),
                    message: "model failed".to_owned(),
                }),
            }),
            "provider.error: model failed"
        );
        assert_eq!(
            format_turn_ended_failure(&TurnEndedFailure {
                reason: TurnEndReason::Blocked,
                error: None,
            }),
            "Prompt hook blocked the request."
        );
        assert_eq!(
            format_turn_ended_failure(&TurnEndedFailure {
                reason: TurnEndReason::Cancelled,
                error: None,
            }),
            "Prompt turn ended with reason: cancelled"
        );
    }

    #[tokio::test]
    async fn runs_main_turn_to_completion_and_ignores_subagent_output() {
        let session: Arc<dyn PromptSession> = Arc::new(PrintSessionMock::new(vec![
            prompt_event(
                "worker",
                PromptEventKind::AssistantDelta {
                    turn_id: 99,
                    delta: "hidden".to_owned(),
                },
            ),
            prompt_event("main", PromptEventKind::TurnStarted { turn_id: 1 }),
            prompt_event(
                "main",
                PromptEventKind::AssistantDelta {
                    turn_id: 1,
                    delta: "hello".to_owned(),
                },
            ),
            prompt_event(
                "main",
                PromptEventKind::ToolProgress {
                    turn_id: 1,
                    text: Some("working".to_owned()),
                },
            ),
            prompt_event(
                "main",
                PromptEventKind::AssistantDelta {
                    turn_id: 1,
                    delta: " world".to_owned(),
                },
            ),
            prompt_event(
                "main",
                PromptEventKind::TurnEnded {
                    turn_id: 1,
                    reason: TurnEndReason::Completed,
                    error: None,
                },
            ),
        ]));
        let mut stdout = Capture::default();
        let mut stderr = Capture::default();

        run_prompt_turn(
            session,
            "say hello",
            PromptOutputFormat::Text,
            &mut stdout,
            &mut stderr,
        )
        .await
        .expect("prompt turn");

        assert_eq!(stdout.text, "• hello world\n\n");
        assert_eq!(stderr.text, "working\n");
    }

    #[tokio::test]
    async fn stream_json_discards_failed_attempt_and_emits_retry_metadata() {
        let retry = RetryingEvent {
            failed_attempt: 1,
            next_attempt: 2,
            max_attempts: 3,
            delay_ms: 300,
            error_name: "RateLimit".to_owned(),
            error_message: "status=429".to_owned(),
            status_code: Some(429),
        };
        let session: Arc<dyn PromptSession> = Arc::new(PrintSessionMock::new(vec![
            prompt_event("main", PromptEventKind::TurnStarted { turn_id: 2 }),
            prompt_event(
                "main",
                PromptEventKind::AssistantDelta {
                    turn_id: 2,
                    delta: "partial".to_owned(),
                },
            ),
            prompt_event(
                "main",
                PromptEventKind::TurnStepRetrying {
                    turn_id: 2,
                    event: retry,
                },
            ),
            prompt_event(
                "main",
                PromptEventKind::AssistantDelta {
                    turn_id: 2,
                    delta: "final".to_owned(),
                },
            ),
            prompt_event(
                "main",
                PromptEventKind::ToolCallStarted {
                    turn_id: 2,
                    tool_call_id: "tc_1".to_owned(),
                    name: "Shell".to_owned(),
                    args: PromptValue::Json(serde_json::json!({ "command": "ls" })),
                },
            ),
            prompt_event(
                "main",
                PromptEventKind::TurnEnded {
                    turn_id: 2,
                    reason: TurnEndReason::Completed,
                    error: None,
                },
            ),
        ]));
        let mut stdout = Capture::default();
        let mut stderr = Capture::default();

        run_prompt_turn(
            session,
            "work",
            PromptOutputFormat::StreamJson,
            &mut stdout,
            &mut stderr,
        )
        .await
        .expect("prompt turn");

        assert!(stdout.text.contains("\"type\":\"turn.step.retrying\""));
        assert!(stdout.text.contains("\"content\":\"final\""));
        assert!(stdout.text.contains("\"id\":\"tc_1\""));
        assert!(!stdout.text.contains("partial"));
        assert!(stderr.text.is_empty());
    }

    #[tokio::test]
    async fn follows_a_continuation_turn_before_finishing() {
        let session: Arc<dyn PromptSession> = Arc::new(PrintSessionMock::new(vec![
            prompt_event("main", PromptEventKind::TurnStarted { turn_id: 1 }),
            prompt_event(
                "main",
                PromptEventKind::AssistantDelta {
                    turn_id: 1,
                    delta: "one".to_owned(),
                },
            ),
            prompt_event(
                "main",
                PromptEventKind::TurnEnded {
                    turn_id: 1,
                    reason: TurnEndReason::Completed,
                    error: None,
                },
            ),
            prompt_event("main", PromptEventKind::TurnStarted { turn_id: 2 }),
            prompt_event(
                "main",
                PromptEventKind::AssistantDelta {
                    turn_id: 2,
                    delta: "two".to_owned(),
                },
            ),
            prompt_event(
                "main",
                PromptEventKind::TurnEnded {
                    turn_id: 2,
                    reason: TurnEndReason::Completed,
                    error: None,
                },
            ),
        ]));
        let mut stdout = Capture::default();
        let mut stderr = Capture::default();

        run_prompt_turn(
            session,
            "continue",
            PromptOutputFormat::Text,
            &mut stdout,
            &mut stderr,
        )
        .await
        .expect("continued turns");

        assert_eq!(stdout.text, "• one\n\n• two\n\n");
    }

    #[tokio::test]
    async fn reports_structured_non_completed_turn_failures() {
        let session: Arc<dyn PromptSession> = Arc::new(PrintSessionMock::new(vec![
            prompt_event("main", PromptEventKind::TurnStarted { turn_id: 3 }),
            prompt_event(
                "main",
                PromptEventKind::TurnEnded {
                    turn_id: 3,
                    reason: TurnEndReason::Failed,
                    error: Some(TurnErrorPayload {
                        code: "provider.filtered".to_owned(),
                        message: "blocked".to_owned(),
                    }),
                },
            ),
        ]));
        let mut stdout = Capture::default();
        let mut stderr = Capture::default();

        let error = run_prompt_turn(
            session,
            "unsafe",
            PromptOutputFormat::Text,
            &mut stdout,
            &mut stderr,
        )
        .await
        .expect_err("turn failure");

        assert_eq!(
            error.to_string(),
            "Provider safety policy blocked the response."
        );
    }
}
