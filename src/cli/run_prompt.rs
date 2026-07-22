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
    sync::{mpsc, oneshot, watch},
    task::{JoinError, JoinHandle},
};

use super::{
    goal_prompt::{
        HeadlessGoalCreate, format_goal_summary_text, goal_exit_code, goal_summary_json,
    },
    options::{CliOptions, PromptOutputFormat},
    prompt_render::{PromptJsonWriter, PromptOutput, PromptTranscriptWriter, PromptTurnWriter},
    prompt_session::{
        ApprovalDecision, ApprovalResponse, CreateGoalInput, CreateSessionOptions,
        ListSessionsOptions, PrintTurnAction, PromptEvent, PromptEventKind, PromptHarness,
        PromptInput, PromptSession, PromptSessionError, ResumeSessionInput,
    },
};
use crate::sdk::types::{GoalSnapshot, GoalStatus, PermissionMode};

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

// Original:
//   apps/kimi-code/src/cli/run-prompt.ts
//   runHeadlessGoal()
pub async fn run_headless_goal(
    session: Arc<dyn PromptSession>,
    goal: &HeadlessGoalCreate,
    model: Option<&str>,
    output_format: PromptOutputFormat,
    stdout: &mut dyn PromptOutput,
    stderr: &mut dyn PromptOutput,
    process_exit_code: &mut Option<i32>,
) -> Result<(), PromptSessionError> {
    require_configured_model([model])?;
    session
        .create_goal(CreateGoalInput {
            objective: goal.objective.clone(),
            replace: goal.replace,
        })
        .await?;

    let completed_snapshot = Arc::new(std::sync::Mutex::new(None::<GoalSnapshot>));
    let snapshot_from_events = Arc::clone(&completed_snapshot);
    let unsubscribe = session.on_event(Arc::new(move |event| {
        if event.agent_id != PROMPT_MAIN_AGENT_ID {
            return;
        }
        if let PromptEventKind::GoalUpdated {
            snapshot: Some(snapshot),
            completion: true,
        } = event.kind
        {
            let mut snapshot_slot = snapshot_from_events
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            *snapshot_slot = Some(snapshot);
        }
    }));

    let turn_result = run_prompt_turn(
        Arc::clone(&session),
        &goal.objective,
        output_format,
        stdout,
        stderr,
    )
    .await;
    unsubscribe();
    let event_snapshot = completed_snapshot
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    let snapshot = match event_snapshot {
        Some(snapshot) => Some(snapshot),
        None => session.get_goal().await?,
    };
    match output_format {
        PromptOutputFormat::StreamJson => {
            let summary = serde_json::to_string(&goal_summary_json(snapshot.as_ref()))
                .expect("goal summary is serializable");
            stdout.write(&(summary + "\n"));
        }
        PromptOutputFormat::Text => {
            stderr.write(&(format_goal_summary_text(snapshot.as_ref()) + "\n"));
        }
    }
    if let Some(snapshot) = &snapshot
        && snapshot.status != GoalStatus::Complete
    {
        *process_exit_code = Some(goal_exit_code(Some(snapshot.status.as_str())));
    }
    turn_result
}

#[derive(Clone)]
pub struct PermissionRestore {
    session: Option<Arc<dyn PromptSession>>,
    previous_permission: PermissionMode,
    override_completed: Option<watch::Receiver<bool>>,
    restored: Arc<std::sync::atomic::AtomicBool>,
}

impl PermissionRestore {
    fn noop() -> Self {
        Self {
            session: None,
            previous_permission: PermissionMode::Auto,
            override_completed: None,
            restored: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }

    pub async fn restore(mut self) -> Result<(), PromptSessionError> {
        if self.restored.swap(true, Ordering::SeqCst) {
            return Ok(());
        }
        if let Some(completed) = &mut self.override_completed {
            while !*completed.borrow() {
                if completed.changed().await.is_err() {
                    break;
                }
            }
        }
        if self.previous_permission != PermissionMode::Auto
            && let Some(session) = self.session
        {
            session.set_permission(self.previous_permission).await?;
        }
        Ok(())
    }
}

// Original: forcePromptPermission()
pub async fn force_prompt_permission<F>(
    session: Arc<dyn PromptSession>,
    previous_permission: PermissionMode,
    set_restore_permission: F,
) -> Result<PermissionRestore, PromptSessionError>
where
    F: FnOnce(PermissionRestore) + Send,
{
    if previous_permission == PermissionMode::Auto {
        let restore = PermissionRestore::noop();
        set_restore_permission(restore.clone());
        return Ok(restore);
    }

    let (completed_tx, completed_rx) = watch::channel(false);
    let (result_tx, result_rx) = oneshot::channel();
    let override_session = Arc::clone(&session);
    tokio::spawn(async move {
        let result = override_session.set_permission(PermissionMode::Auto).await;
        let _ = completed_tx.send(true);
        let _ = result_tx.send(result);
    });
    let restore = PermissionRestore {
        session: Some(session),
        previous_permission,
        override_completed: Some(completed_rx),
        restored: Arc::new(std::sync::atomic::AtomicBool::new(false)),
    };
    set_restore_permission(restore.clone());
    result_rx
        .await
        .map_err(|_| std::io::Error::other("permission override task stopped"))??;
    Ok(restore)
}

fn install_headless_handlers(session: &Arc<dyn PromptSession>) {
    session.set_approval_handler(Some(Arc::new(|_| {
        Box::pin(async {
            ApprovalResponse {
                decision: ApprovalDecision::Approved,
                scope: None,
                feedback: None,
                selected_label: None,
            }
        })
    })));
    session.set_question_handler(Some(Arc::new(|_| Box::pin(async { None }))));
}

pub struct ResolvedPromptSession {
    pub session: Arc<dyn PromptSession>,
    pub resumed: bool,
    pub restore_permission: PermissionRestore,
    pub telemetry_model: Option<String>,
    pub goal_model: Option<String>,
}

// Original: resolvePromptSession()
pub async fn resolve_prompt_session<F>(
    harness: &dyn PromptHarness,
    options: &CliOptions,
    work_dir: &str,
    default_model: Option<&str>,
    stderr: &mut dyn PromptOutput,
    set_restore_permission: F,
) -> Result<ResolvedPromptSession, PromptSessionError>
where
    F: Fn(PermissionRestore) + Send,
{
    if let Some(session_id) = &options.session {
        let sessions = harness
            .list_sessions(ListSessionsOptions {
                work_dir: Some(work_dir.to_owned()),
                session_id: Some(session_id.clone()),
            })
            .await?;
        let target = sessions.first().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("Session \"{session_id}\" not found."),
            )
        })?;
        if !paths_equivalent(&target.work_dir, work_dir) {
            stderr.write(&format!(
                "Session \"{session_id}\" was created under a different directory.\n  cd \"{}\" && kimi -r {session_id}\n\n",
                target.work_dir
            ));
            return Err(std::io::Error::other(format!(
                "Session \"{session_id}\" was created under a different directory."
            ))
            .into());
        }
        return resume_prompt_session(
            harness,
            options,
            session_id,
            default_model,
            set_restore_permission,
        )
        .await;
    }

    if options.continue_previous {
        let sessions = harness
            .list_sessions(ListSessionsOptions {
                work_dir: Some(work_dir.to_owned()),
                session_id: None,
            })
            .await?;
        if let Some(previous) = sessions.first() {
            return resume_prompt_session(
                harness,
                options,
                &previous.id,
                default_model,
                set_restore_permission,
            )
            .await;
        }
        stderr.write(&format!(
            "No sessions to continue under \"{work_dir}\"; starting a fresh session.\n"
        ));
    }

    let model = require_configured_model([options.model.as_deref(), default_model])?.to_owned();
    let session = harness
        .create_session(CreateSessionOptions {
            work_dir: work_dir.to_owned(),
            model: Some(model.clone()),
            permission: Some(PermissionMode::Auto),
            additional_dirs: non_empty_directories(&options.add_dirs),
            drain_agent_tasks_on_stop: true,
        })
        .await?;
    install_headless_handlers(&session);
    let restore_permission = PermissionRestore::noop();
    set_restore_permission(restore_permission.clone());
    Ok(ResolvedPromptSession {
        session,
        resumed: false,
        restore_permission,
        telemetry_model: Some(model.clone()),
        goal_model: Some(model),
    })
}

async fn resume_prompt_session<F>(
    harness: &dyn PromptHarness,
    options: &CliOptions,
    session_id: &str,
    default_model: Option<&str>,
    set_restore_permission: F,
) -> Result<ResolvedPromptSession, PromptSessionError>
where
    F: Fn(PermissionRestore) + Send,
{
    let session = harness
        .resume_session(ResumeSessionInput {
            id: session_id.to_owned(),
            additional_dirs: non_empty_directories(&options.add_dirs),
        })
        .await?;
    let status = session.get_status().await?;
    let restore_permission = force_prompt_permission(
        Arc::clone(&session),
        status.permission,
        set_restore_permission,
    )
    .await?;
    if let Some(model) = &options.model {
        session.set_model(model).await?;
    }
    install_headless_handlers(&session);
    Ok(ResolvedPromptSession {
        session,
        resumed: true,
        restore_permission,
        telemetry_model: configured_model([
            options.model.as_deref(),
            status.model.as_deref(),
            default_model,
        ])
        .map(str::to_owned),
        goal_model: configured_model([options.model.as_deref(), status.model.as_deref()])
            .map(str::to_owned),
    })
}

fn non_empty_directories(directories: &[String]) -> Option<Vec<String>> {
    (!directories.is_empty()).then(|| directories.to_vec())
}

fn paths_equivalent(left: &str, right: &str) -> bool {
    normalize_comparison_path(left) == normalize_comparison_path(right)
}

fn normalize_comparison_path(path: &str) -> String {
    let path = path.replace('\\', "/");
    let has_windows_drive = path.as_bytes().get(1) == Some(&b':');
    let prefix = if has_windows_drive {
        path[..2].to_ascii_lowercase()
    } else if path.starts_with('/') {
        "/".to_owned()
    } else {
        String::new()
    };
    let start = if has_windows_drive { 2 } else { 0 };
    let mut parts = Vec::new();
    for part in path[start..].split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            part => parts.push(part),
        }
    }
    let normalized = if prefix == "/" {
        format!("/{0}", parts.join("/"))
    } else if prefix.is_empty() {
        parts.join("/")
    } else {
        format!("{prefix}/{}", parts.join("/"))
    };
    if has_windows_drive {
        normalized.to_ascii_lowercase()
    } else {
        normalized
    }
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
                ApprovalHandler, ConfigDiagnostics, CreateGoalInput, CreateSessionOptions,
                EventListener, ListSessionsOptions, PrintTurnAction, PromptConfig, PromptEvent,
                PromptEventKind, PromptHarness, PromptInput, PromptSession, PromptSessionError,
                QuestionHandler, ResumeSessionInput, TelemetryProperties, Unsubscribe,
            },
        },
        sdk::types::{
            CronTaskSnapshot, GoalBudgetReport, GoalSnapshot, PermissionMode, SessionStatus,
            SessionSummary,
        },
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
        listeners: Mutex<Vec<EventListener>>,
        events: Vec<PromptEvent>,
        prompted: Mutex<Vec<PromptInput>>,
        status: SessionStatus,
        permissions: Mutex<Vec<PermissionMode>>,
        models: Mutex<Vec<String>>,
        approval_handler: Mutex<Option<ApprovalHandler>>,
        question_handler: Mutex<Option<QuestionHandler>>,
        auto_permission_gate: Option<Arc<tokio::sync::Notify>>,
        create_goal_result: Option<GoalSnapshot>,
        current_goal: Mutex<Option<GoalSnapshot>>,
        created_goals: Mutex<Vec<CreateGoalInput>>,
    }

    impl PrintSessionMock {
        fn new(events: Vec<PromptEvent>) -> Self {
            Self {
                listeners: Mutex::new(Vec::new()),
                events,
                prompted: Mutex::new(Vec::new()),
                status: SessionStatus {
                    model: Some("k2".to_owned()),
                    thinking_effort: "on".to_owned(),
                    permission: PermissionMode::Manual,
                    plan_mode: false,
                    swarm_mode: None,
                    context_tokens: 0,
                    max_context_tokens: 0,
                    context_usage: 0.0,
                    usage: None,
                },
                permissions: Mutex::new(Vec::new()),
                models: Mutex::new(Vec::new()),
                approval_handler: Mutex::new(None),
                question_handler: Mutex::new(None),
                auto_permission_gate: None,
                create_goal_result: None,
                current_goal: Mutex::new(None),
                created_goals: Mutex::new(Vec::new()),
            }
        }

        fn with_status(mut self, permission: PermissionMode, model: Option<&str>) -> Self {
            self.status.permission = permission;
            self.status.model = model.map(str::to_owned);
            self
        }

        fn with_auto_permission_gate(mut self, gate: Arc<tokio::sync::Notify>) -> Self {
            self.auto_permission_gate = Some(gate);
            self
        }

        fn with_goals(
            mut self,
            create_goal_result: GoalSnapshot,
            current_goal: Option<GoalSnapshot>,
        ) -> Self {
            self.create_goal_result = Some(create_goal_result);
            *self.current_goal.lock().expect("current goal") = current_goal;
            self
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
            Ok(self.status.clone())
        }

        async fn set_model(&self, model: &str) -> Result<(), PromptSessionError> {
            self.models.lock().expect("models").push(model.to_owned());
            Ok(())
        }

        async fn set_permission(&self, mode: PermissionMode) -> Result<(), PromptSessionError> {
            self.permissions.lock().expect("permissions").push(mode);
            if mode == PermissionMode::Auto
                && let Some(gate) = &self.auto_permission_gate
            {
                gate.notified().await;
            }
            Ok(())
        }

        fn set_approval_handler(&self, handler: Option<ApprovalHandler>) {
            *self.approval_handler.lock().expect("approval handler") = handler;
        }

        fn set_question_handler(&self, handler: Option<QuestionHandler>) {
            *self.question_handler.lock().expect("question handler") = handler;
        }

        fn on_event(&self, listener: EventListener) -> Unsubscribe {
            self.listeners.lock().expect("listeners").push(listener);
            Box::new(|| {})
        }

        async fn prompt(&self, input: PromptInput) -> Result<(), PromptSessionError> {
            self.prompted.lock().expect("prompts").push(input);
            let listeners = self.listeners.lock().expect("listeners").clone();
            for event in &self.events {
                for listener in &listeners {
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
            input: CreateGoalInput,
        ) -> Result<GoalSnapshot, PromptSessionError> {
            self.created_goals
                .lock()
                .expect("created goals")
                .push(input);
            self.create_goal_result
                .clone()
                .ok_or_else(|| std::io::Error::other("unused").into())
        }

        async fn get_goal(&self) -> Result<Option<GoalSnapshot>, PromptSessionError> {
            Ok(self.current_goal.lock().expect("current goal").clone())
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

    fn goal_snapshot(status: GoalStatus, turns_used: u64, tokens_used: u64) -> GoalSnapshot {
        GoalSnapshot {
            goal_id: "goal_1".to_owned(),
            objective: "ship the migration".to_owned(),
            completion_criterion: None,
            status,
            turns_used,
            tokens_used,
            wall_clock_ms: 1_500,
            budget: GoalBudgetReport {
                token_budget: Some(1_000),
                turn_budget: Some(10),
                wall_clock_budget_ms: None,
                remaining_tokens: Some(1_000_u64.saturating_sub(tokens_used)),
                remaining_turns: Some(10_u64.saturating_sub(turns_used)),
                remaining_wall_clock_ms: None,
                token_budget_reached: false,
                turn_budget_reached: false,
                wall_clock_budget_reached: false,
                over_budget: false,
            },
            terminal_reason: None,
        }
    }

    struct HarnessMock {
        sessions: Vec<SessionSummary>,
        session: Arc<PrintSessionMock>,
        listed: Mutex<Vec<ListSessionsOptions>>,
        created: Mutex<Vec<CreateSessionOptions>>,
        resumed: Mutex<Vec<ResumeSessionInput>>,
    }

    impl HarnessMock {
        fn new(sessions: Vec<SessionSummary>, session: Arc<PrintSessionMock>) -> Self {
            Self {
                sessions,
                session,
                listed: Mutex::new(Vec::new()),
                created: Mutex::new(Vec::new()),
                resumed: Mutex::new(Vec::new()),
            }
        }
    }

    #[async_trait]
    impl PromptHarness for HarnessMock {
        fn home_dir(&self) -> &str {
            "/home"
        }

        fn track(&self, _: &str, _: Option<&TelemetryProperties>) {}

        async fn ensure_config_file(&self) -> Result<(), PromptSessionError> {
            Ok(())
        }

        async fn get_config(&self) -> Result<PromptConfig, PromptSessionError> {
            Ok(PromptConfig {
                default_model: Some("default".to_owned()),
                telemetry: true,
            })
        }

        async fn get_config_diagnostics(&self) -> Result<ConfigDiagnostics, PromptSessionError> {
            Ok(ConfigDiagnostics::default())
        }

        async fn list_sessions(
            &self,
            options: ListSessionsOptions,
        ) -> Result<Vec<SessionSummary>, PromptSessionError> {
            self.listed.lock().expect("listed").push(options);
            Ok(self.sessions.clone())
        }

        async fn create_session(
            &self,
            options: CreateSessionOptions,
        ) -> Result<Arc<dyn PromptSession>, PromptSessionError> {
            self.created.lock().expect("created").push(options);
            Ok(Arc::clone(&self.session) as Arc<dyn PromptSession>)
        }

        async fn resume_session(
            &self,
            input: ResumeSessionInput,
        ) -> Result<Arc<dyn PromptSession>, PromptSessionError> {
            self.resumed.lock().expect("resumed").push(input);
            Ok(Arc::clone(&self.session) as Arc<dyn PromptSession>)
        }

        async fn close(&self) -> Result<(), PromptSessionError> {
            Ok(())
        }
    }

    fn summary(id: &str, work_dir: &str) -> SessionSummary {
        SessionSummary {
            id: id.to_owned(),
            title: None,
            last_prompt: None,
            work_dir: work_dir.to_owned(),
            session_dir: "/sessions/one".to_owned(),
            created_at: Some(1.0),
            updated_at: Some(2.0),
            archived: None,
            metadata: None,
            additional_dirs: None,
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

    #[tokio::test]
    async fn runs_headless_goal_and_prefers_the_completion_event_snapshot() {
        let active = goal_snapshot(GoalStatus::Active, 0, 0);
        let complete = goal_snapshot(GoalStatus::Complete, 4, 240);
        let session = Arc::new(
            PrintSessionMock::new(vec![
                prompt_event("main", PromptEventKind::TurnStarted { turn_id: 7 }),
                prompt_event(
                    "main",
                    PromptEventKind::AssistantDelta {
                        turn_id: 7,
                        delta: "done".to_owned(),
                    },
                ),
                prompt_event(
                    "main",
                    PromptEventKind::GoalUpdated {
                        snapshot: Some(complete),
                        completion: true,
                    },
                ),
                prompt_event(
                    "main",
                    PromptEventKind::TurnEnded {
                        turn_id: 7,
                        reason: TurnEndReason::Completed,
                        error: None,
                    },
                ),
            ])
            .with_goals(active, None),
        );
        let session_trait: Arc<dyn PromptSession> = session.clone();
        let goal = HeadlessGoalCreate {
            objective: "ship the migration".to_owned(),
            replace: true,
        };
        let mut stdout = Capture::default();
        let mut stderr = Capture::default();
        let mut exit_code = None;

        run_headless_goal(
            session_trait,
            &goal,
            Some("k2"),
            PromptOutputFormat::StreamJson,
            &mut stdout,
            &mut stderr,
            &mut exit_code,
        )
        .await
        .expect("headless goal");

        assert_eq!(
            session
                .created_goals
                .lock()
                .expect("created goals")
                .as_slice(),
            [CreateGoalInput {
                objective: "ship the migration".to_owned(),
                replace: true,
            }]
        );
        let summary: serde_json::Value = serde_json::from_str(
            stdout
                .text
                .lines()
                .last()
                .expect("goal summary output line"),
        )
        .expect("goal summary json");
        assert_eq!(summary["type"], "goal.summary");
        assert_eq!(summary["status"], "complete");
        assert_eq!(summary["turnsUsed"], 4);
        assert_eq!(summary["tokensUsed"], 240);
        assert!(stderr.text.is_empty());
        assert_eq!(exit_code, None);
    }

    #[tokio::test]
    async fn headless_goal_falls_back_to_current_goal_and_sets_blocked_exit_code() {
        let active = goal_snapshot(GoalStatus::Active, 0, 0);
        let mut blocked = goal_snapshot(GoalStatus::Blocked, 2, 80);
        blocked.terminal_reason = Some("waiting for input".to_owned());
        let session: Arc<dyn PromptSession> = Arc::new(
            PrintSessionMock::new(vec![
                prompt_event("main", PromptEventKind::TurnStarted { turn_id: 8 }),
                prompt_event(
                    "main",
                    PromptEventKind::TurnEnded {
                        turn_id: 8,
                        reason: TurnEndReason::Completed,
                        error: None,
                    },
                ),
            ])
            .with_goals(active, Some(blocked)),
        );
        let mut stdout = Capture::default();
        let mut stderr = Capture::default();
        let mut exit_code = None;

        run_headless_goal(
            session,
            &HeadlessGoalCreate {
                objective: "ship the migration".to_owned(),
                replace: false,
            },
            Some("k2"),
            PromptOutputFormat::Text,
            &mut stdout,
            &mut stderr,
            &mut exit_code,
        )
        .await
        .expect("blocked headless goal turn completes");

        assert!(stdout.text.is_empty());
        assert!(stderr.text.contains("Goal [blocked]"));
        assert!(stderr.text.contains("waiting for input"));
        assert_eq!(exit_code, Some(3));
    }

    #[tokio::test]
    async fn headless_goal_reports_summary_even_when_the_turn_fails() {
        let active = goal_snapshot(GoalStatus::Active, 0, 0);
        let paused = goal_snapshot(GoalStatus::Paused, 1, 10);
        let session: Arc<dyn PromptSession> = Arc::new(
            PrintSessionMock::new(vec![
                prompt_event("main", PromptEventKind::TurnStarted { turn_id: 9 }),
                prompt_event(
                    "main",
                    PromptEventKind::TurnEnded {
                        turn_id: 9,
                        reason: TurnEndReason::Failed,
                        error: None,
                    },
                ),
            ])
            .with_goals(active, Some(paused)),
        );
        let mut stdout = Capture::default();
        let mut stderr = Capture::default();
        let mut exit_code = None;

        let error = run_headless_goal(
            session,
            &HeadlessGoalCreate {
                objective: "ship the migration".to_owned(),
                replace: false,
            },
            Some("k2"),
            PromptOutputFormat::Text,
            &mut stdout,
            &mut stderr,
            &mut exit_code,
        )
        .await
        .expect_err("turn failure is preserved");

        assert_eq!(error.to_string(), "Prompt turn ended with reason: failed");
        assert!(stderr.text.contains("Goal [paused]"));
        assert_eq!(exit_code, Some(6));
    }

    #[tokio::test]
    async fn headless_goal_requires_a_model_before_creation() {
        let session = Arc::new(PrintSessionMock::new(Vec::new()));
        let session_trait: Arc<dyn PromptSession> = session.clone();
        let mut stdout = Capture::default();
        let mut stderr = Capture::default();
        let mut exit_code = None;

        let error = run_headless_goal(
            session_trait,
            &HeadlessGoalCreate {
                objective: "ship the migration".to_owned(),
                replace: false,
            },
            None,
            PromptOutputFormat::Text,
            &mut stdout,
            &mut stderr,
            &mut exit_code,
        )
        .await
        .expect_err("missing model");

        assert!(error.to_string().contains("No model configured"));
        assert!(
            session
                .created_goals
                .lock()
                .expect("created goals")
                .is_empty()
        );
        assert!(stdout.text.is_empty());
        assert!(stderr.text.is_empty());
        assert_eq!(exit_code, None);
    }

    #[tokio::test]
    async fn resumes_explicit_session_across_windows_path_separators() {
        let session = Arc::new(
            PrintSessionMock::new(Vec::new())
                .with_status(PermissionMode::Manual, Some("session-model")),
        );
        let harness = HarnessMock::new(
            vec![summary("ses_existing", "C:/Users/kimi/project")],
            Arc::clone(&session),
        );
        let options = CliOptions {
            session: Some("ses_existing".to_owned()),
            model: Some("cli-model".to_owned()),
            add_dirs: vec!["../shared".to_owned()],
            ..CliOptions::default()
        };
        let mut stderr = Capture::default();

        let resolved = resolve_prompt_session(
            &harness,
            &options,
            r"C:\Users\kimi\project",
            Some("default-model"),
            &mut stderr,
            |_| {},
        )
        .await
        .expect("resume session");

        assert!(resolved.resumed);
        assert_eq!(resolved.telemetry_model.as_deref(), Some("cli-model"));
        assert_eq!(resolved.goal_model.as_deref(), Some("cli-model"));
        assert_eq!(
            harness.resumed.lock().expect("resumed").as_slice(),
            [ResumeSessionInput {
                id: "ses_existing".to_owned(),
                additional_dirs: Some(vec!["../shared".to_owned()]),
            }]
        );
        assert_eq!(
            session.models.lock().expect("models").as_slice(),
            ["cli-model"]
        );
        assert_eq!(
            session.permissions.lock().expect("permissions").as_slice(),
            [PermissionMode::Auto]
        );
        let approval = session
            .approval_handler
            .lock()
            .expect("approval handler")
            .clone()
            .expect("approval handler installed");
        let response = approval(super::super::prompt_session::ApprovalRequest {
            turn_id: Some(1),
            tool_call_id: "tc".to_owned(),
            tool_name: "Shell".to_owned(),
            action: "run".to_owned(),
            display: serde_json::json!({}),
        })
        .await;
        assert_eq!(response.decision, ApprovalDecision::Approved);
        let question = session
            .question_handler
            .lock()
            .expect("question handler")
            .clone()
            .expect("question handler installed");
        assert_eq!(
            question(super::super::prompt_session::QuestionRequest {
                turn_id: Some(1),
                tool_call_id: None,
                questions: Vec::new(),
            })
            .await,
            None
        );

        resolved
            .restore_permission
            .restore()
            .await
            .expect("restore permission");
        assert_eq!(
            session.permissions.lock().expect("permissions").as_slice(),
            [PermissionMode::Auto, PermissionMode::Manual]
        );
        assert!(stderr.text.is_empty());
    }

    #[tokio::test]
    async fn rejects_an_explicit_session_from_another_directory() {
        let session = Arc::new(PrintSessionMock::new(Vec::new()));
        let harness = HarnessMock::new(vec![summary("ses_elsewhere", "/other/project")], session);
        let options = CliOptions {
            session: Some("ses_elsewhere".to_owned()),
            ..CliOptions::default()
        };
        let mut stderr = Capture::default();

        let error = resolve_prompt_session(
            &harness,
            &options,
            "/current/project",
            Some("model"),
            &mut stderr,
            |_| {},
        )
        .await
        .err()
        .expect("directory mismatch");

        assert!(error.to_string().contains("different directory"));
        assert!(
            stderr
                .text
                .contains("cd \"/other/project\" && kimi -r ses_elsewhere")
        );
        assert!(harness.resumed.lock().expect("resumed").is_empty());
    }

    #[tokio::test]
    async fn continue_without_history_creates_a_fresh_auto_session() {
        let session = Arc::new(PrintSessionMock::new(Vec::new()));
        let harness = HarnessMock::new(Vec::new(), Arc::clone(&session));
        let options = CliOptions {
            continue_previous: true,
            add_dirs: vec!["/extra".to_owned()],
            ..CliOptions::default()
        };
        let mut stderr = Capture::default();

        let resolved = resolve_prompt_session(
            &harness,
            &options,
            "/work",
            Some("default-model"),
            &mut stderr,
            |_| {},
        )
        .await
        .expect("fresh session");

        assert!(!resolved.resumed);
        assert_eq!(resolved.telemetry_model.as_deref(), Some("default-model"));
        assert_eq!(
            harness.created.lock().expect("created").as_slice(),
            [CreateSessionOptions {
                work_dir: "/work".to_owned(),
                model: Some("default-model".to_owned()),
                permission: Some(PermissionMode::Auto),
                additional_dirs: Some(vec!["/extra".to_owned()]),
                drain_agent_tasks_on_stop: true,
            }]
        );
        assert!(stderr.text.contains("No sessions to continue"));
        assert!(
            session
                .approval_handler
                .lock()
                .expect("approval handler")
                .is_some()
        );
    }

    #[tokio::test]
    async fn refuses_to_create_a_session_without_any_configured_model() {
        let session = Arc::new(PrintSessionMock::new(Vec::new()));
        let harness = HarnessMock::new(Vec::new(), session);
        let mut stderr = Capture::default();

        let error = resolve_prompt_session(
            &harness,
            &CliOptions::default(),
            "/work",
            None,
            &mut stderr,
            |_| {},
        )
        .await
        .err()
        .expect("missing model");

        assert!(error.to_string().contains("No model configured"));
        assert!(harness.created.lock().expect("created").is_empty());
    }

    #[tokio::test]
    async fn cleanup_waits_for_pending_auto_override_before_restoring() {
        let gate = Arc::new(tokio::sync::Notify::new());
        let session = Arc::new(
            PrintSessionMock::new(Vec::new())
                .with_status(PermissionMode::Manual, None)
                .with_auto_permission_gate(Arc::clone(&gate)),
        );
        let session_trait: Arc<dyn PromptSession> = session.clone();
        let restore_slot = Arc::new(Mutex::new(None));
        let restore_slot_for_callback = Arc::clone(&restore_slot);
        let force =
            force_prompt_permission(session_trait, PermissionMode::Manual, move |restore| {
                *restore_slot_for_callback.lock().expect("restore slot") = Some(restore);
            });
        tokio::pin!(force);
        assert!(futures_util::poll!(&mut force).is_pending());
        tokio::task::yield_now().await;
        assert_eq!(
            session.permissions.lock().expect("permissions").as_slice(),
            [PermissionMode::Auto]
        );

        let restore = restore_slot
            .lock()
            .expect("restore slot")
            .take()
            .expect("restore registered before await");
        let restoring = restore.restore();
        tokio::pin!(restoring);
        assert!(futures_util::poll!(&mut restoring).is_pending());
        gate.notify_waiters();
        force.await.expect("override permission");
        restoring.await.expect("restore permission");

        assert_eq!(
            session.permissions.lock().expect("permissions").as_slice(),
            [PermissionMode::Auto, PermissionMode::Manual]
        );
    }
}
