//! The model-facing Bash command runner.
//!
//! Original: `packages/agent-core-v2/src/os/backends/node-local/tools/bash.ts`.

use std::collections::HashMap;
use std::sync::{Arc, atomic::{AtomicBool, Ordering}};
use parking_lot::Mutex;

use async_trait::async_trait;
use futures_util::future::BoxFuture;
use regex::Regex;
use serde::Deserialize;
use serde_json::{Map, Value, json};
use tokio::io::AsyncWriteExt;

use crate::{
    _base::{
        di::instantiation::ServicesAccessorExt,
        utils::abort::{AbortSignal, user_cancellation_reason},
    },
    agent::{
        task::{
            AGENT_TASK_SERVICE_ID, AgentTaskServiceHandle, ForegroundTaskReleaseReason,
            RegisterAgentTaskOptions, config_section::resolve_agent_task_config,
            types::AgentTaskStatus,
        },
        tool_policy::{AGENT_TOOL_POLICY_SERVICE_ID, AgentToolPolicyServiceHandle},
        tool_registry::{ToolContributionOptions, register_tool},
    },
    app::config::{CONFIG_SERVICE_ID, ConfigServiceHandle},
    kosong::contract::tool::Tool,
    os::interface::{
        host_environment::{HOST_ENVIRONMENT_SERVICE_ID, HostEnvironmentHandle},
        host_process::{HostProcess, ProcessSignal},
    },
    session::{
        process::{
            ProcessExecOptions, SESSION_PROCESS_RUNNER_SERVICE_ID, SessionProcessRunnerHandle,
        },
        session_context::{SESSION_CONTEXT_ID, SessionContext},
    },
    tool::{
        ExecutableTool, ExecutableToolContext, ExecutableToolResult, RunnableToolExecution,
        ToolAccess, ToolExecution, ToolInputDisplay, ToolSource, ToolUpdate, ToolUpdateKind,
        input_schema::to_input_json_schema,
        result_builder::{ToolResultBuilder, ToolResultBuilderResult},
        rule_match::{literal_rule_pattern, matches_glob_rule_subject},
    },
};
use kimi_code_protocol::CommandLanguage;

use super::process_task::{ProcessTask, ProcessTaskOutputKind};

const MS_PER_SECOND: u64 = 1_000;
const DEFAULT_TIMEOUT_S: u64 = 60;
const MAX_TIMEOUT_S: u64 = 5 * 60;
const DEFAULT_BACKGROUND_TIMEOUT_S: u64 = 10 * 60;
const MAX_BACKGROUND_TIMEOUT_S: u64 = 24 * 60 * 60;
const BASH_DESCRIPTION_TEMPLATE: &str = include_str!("bash.md");

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub struct BashInput {
    pub command: String,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub timeout: Option<u64>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub run_in_background: Option<bool>,
    #[serde(default)]
    pub disable_timeout: Option<bool>,
}

pub fn bash_parameters() -> Map<String, Value> {
    to_input_json_schema(
        json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "minLength": 1,
                    "description": "The command to execute."
                },
                "cwd": {
                    "type": "string",
                    "description": "The working directory in which to run the command. When omitted, the command runs in the session's working directory."
                },
                "timeout": {
                    "type": "integer",
                    "minimum": 1,
                    "default": DEFAULT_TIMEOUT_S,
                    "description": format!(
                        "Optional timeout in seconds for the command to execute. Foreground default {DEFAULT_TIMEOUT_S}s, max {MAX_TIMEOUT_S}s. Background default {DEFAULT_BACKGROUND_TIMEOUT_S}s, max {MAX_BACKGROUND_TIMEOUT_S}s. Ignored for background commands when disable_timeout=true."
                    )
                },
                "description": {
                    "type": "string",
                    "description": "A short description for the background task. Required when run_in_background is true."
                },
                "run_in_background": {
                    "type": "boolean",
                    "description": "Whether to run the command as a background task."
                },
                "disable_timeout": {
                    "type": "boolean",
                    "description": "If true, do not apply a timeout to the command. Only applies when run_in_background is true."
                }
            },
            "required": ["command"],
            "allOf": [
                {
                    "if": {
                        "properties": { "run_in_background": { "const": true } },
                        "required": ["run_in_background"]
                    },
                    "then": {
                        "properties": { "timeout": { "maximum": MAX_BACKGROUND_TIMEOUT_S } }
                    },
                    "else": {
                        "properties": { "timeout": { "maximum": MAX_TIMEOUT_S } }
                    }
                }
            ]
        })
        .as_object()
        .cloned()
        .expect("Bash schema is an object"),
    )
}

#[derive(Clone)]
pub struct BashTool {
    runner: SessionProcessRunnerHandle,
    environment: HostEnvironmentHandle,
    context: SessionContext,
    tasks: AgentTaskServiceHandle,
    tool_policy: AgentToolPolicyServiceHandle,
    config: ConfigServiceHandle,
    is_windows_bash: bool,
    rendered_description: String,
    definition: Tool,
}

impl BashTool {
    pub fn new(
        runner: SessionProcessRunnerHandle,
        environment: HostEnvironmentHandle,
        context: SessionContext,
        tasks: AgentTaskServiceHandle,
        tool_policy: AgentToolPolicyServiceHandle,
        config: ConfigServiceHandle,
    ) -> Result<Self, crate::_base::errors::errors::BugIndicatingError> {
        let info = environment.info()?;
        let rendered_description = render_bash_description(info.shell_name.as_str());
        let mut this = Self {
            runner,
            environment,
            context,
            tasks,
            tool_policy,
            config,
            is_windows_bash: info.os_kind == "Windows",
            rendered_description,
            definition: Tool {
                name: "Bash".into(),
                description: String::new(),
                parameters: bash_parameters(),
                deferred: None,
            },
        };
        this.definition.description = this.description();
        Ok(this)
    }

    fn allow_background(&self) -> bool {
        ["TaskList", "TaskOutput", "TaskStop"].iter().all(|name| {
            self.tool_policy
                .is_tool_active(name, ToolSource::Builtin)
                .unwrap_or(false)
        })
    }

    fn auto_background_on_timeout(&self) -> bool {
        resolve_agent_task_config(&self.config)
            .and_then(|config| config.bash_auto_background_on_timeout)
            .unwrap_or(true)
    }

    fn description(&self) -> String {
        if !self.allow_background() {
            without_background_description(&self.rendered_description)
        } else if !self.auto_background_on_timeout() {
            without_auto_background_on_timeout(&self.rendered_description)
        } else {
            self.rendered_description.clone()
        }
    }

    async fn spawn(
        &self,
        effective_cwd: &str,
        command: &str,
    ) -> Result<Arc<dyn HostProcess>, Box<dyn std::error::Error + Send + Sync>> {
        let shell_cwd = if self.is_windows_bash {
            windows_path_to_posix_path(effective_cwd)
        } else {
            effective_cwd.to_owned()
        };
        let shell_path = self
            .environment
            .shell_path()
            .map_err(|error| Box::new(error) as Box<dyn std::error::Error + Send + Sync>)?;
        let args = vec![
            shell_path.clone(),
            "-c".into(),
            format!("cd {} && {command}", shell_quote(&shell_cwd)),
        ];
        let environment = HashMap::from([
            ("NO_COLOR".into(), "1".into()),
            ("TERM".into(), "dumb".into()),
            (
                "GIT_TERMINAL_PROMPT".into(),
                std::env::var("GIT_TERMINAL_PROMPT").unwrap_or_else(|_| "0".into()),
            ),
            ("SHELL".into(), shell_path),
        ]);
        self.runner
            .exec(
                &args,
                Some(ProcessExecOptions {
                    cwd: None,
                    env: Some(environment),
                }),
            )
            .await
    }

    async fn execute(
        &self,
        args: BashInput,
        context: ExecutableToolContext,
    ) -> ExecutableToolResult {
        if let Some(error) = self.validate_run_request(&args, &context.signal) {
            return error;
        }

        let starts_in_background = args.run_in_background == Some(true);
        let foreground_timeout_ms = normalize_timeout_ms(args.timeout, false);
        let command = if self.is_windows_bash {
            rewrite_windows_null_redirect(&args.command)
        } else {
            args.command.clone()
        };
        let effective_cwd = args.cwd.as_deref().unwrap_or(&self.context.cwd);
        let description = if starts_in_background {
            args.description
                .as_deref()
                .unwrap_or_default()
                .trim()
                .to_owned()
        } else {
            foreground_description(&args)
        };
        let timeout_ms = if starts_in_background {
            if args.disable_timeout == Some(true) {
                None
            } else {
                Some(normalize_timeout_ms(args.timeout, true))
            }
        } else {
            Some(foreground_timeout_ms)
        };

        let process = match self.spawn(effective_cwd, &command).await {
            Ok(process) => process,
            Err(error) => return ExecutableToolResult::error(error.to_string()),
        };
        close_process_stdin(&process).await;

        let builder = Arc::new(Mutex::new(ToolResultBuilder::default()));
        let collect_foreground_output = Arc::new(AtomicBool::new(!starts_in_background));
        let foreground_output_persisted = Arc::new(AtomicBool::new(false));
        let foreground_task_id = Arc::new(Mutex::new(None::<String>));
        let on_process_output = (!starts_in_background).then(|| {
            let builder = Arc::clone(&builder);
            let collecting = Arc::clone(&collect_foreground_output);
            let persisted = Arc::clone(&foreground_output_persisted);
            let task_id = Arc::clone(&foreground_task_id);
            let tasks = self.tasks.clone();
            let on_update = context.on_update.clone();
            Arc::new(move |kind, text: &str| {
                if !collecting.load(Ordering::Acquire) {
                    return;
                }
                if let Some(on_update) = &on_update {
                    on_update(ToolUpdate {
                        kind: match kind {
                            ProcessTaskOutputKind::Stdout => ToolUpdateKind::Stdout,
                            ProcessTaskOutputKind::Stderr => ToolUpdateKind::Stderr,
                        },
                        text: Some(text.to_owned()),
                        percent: None,
                        custom_kind: None,
                        custom_data: None,
                    });
                }
                let truncated = {
                    let mut builder = builder.lock();
                    builder.write(text);
                    builder.truncated()
                };
                if truncated
                    && !persisted.swap(true, Ordering::AcqRel)
                    && let Some(task_id) = task_id.lock().as_deref()
                {
                    tasks.persist_output(task_id);
                }
            }) as _
        });

        let process_task = Arc::new(ProcessTask::new(
            Arc::clone(&process),
            command,
            description.clone(),
            on_process_output,
        ));
        let task_id = match self.tasks.register_task(
            process_task,
            RegisterAgentTaskOptions {
                detached: Some(starts_in_background),
                timeout_ms,
                detach_timeout_ms: Some(DEFAULT_BACKGROUND_TIMEOUT_S * MS_PER_SECOND),
                auto_background_on_timeout: Some(
                    self.allow_background() && self.auto_background_on_timeout(),
                ),
                signal: (!starts_in_background).then_some(context.signal.clone()),
            },
        ) {
            Ok(task_id) => task_id,
            Err(error) => {
                collect_foreground_output.store(false, Ordering::Release);
                kill_spawned_process(&process).await;
                return ExecutableToolResult::error(error.to_string());
            }
        };
        if !starts_in_background {
            *foreground_task_id.lock() = Some(task_id.clone());
            if let Some(callback) = context.on_foreground_task_start {
                callback(task_id.clone());
            }
        }

        if starts_in_background {
            return self.background_started_result(
                &task_id,
                &process,
                &description,
                format!("Started {task_id}"),
                &builder,
                BackgroundScenario::Started,
            );
        }

        let result = match self.tasks.wait_for_foreground_release(&task_id).await {
            Ok(Some(
                reason @ (ForegroundTaskReleaseReason::Detached
                | ForegroundTaskReleaseReason::TimeoutDetached),
            )) => {
                collect_foreground_output.store(false, Ordering::Release);
                let brief = if reason == ForegroundTaskReleaseReason::TimeoutDetached {
                    format!("Backgrounded {task_id} after timeout")
                } else {
                    format!("Backgrounded {task_id}")
                };
                self.background_started_result(
                    &task_id,
                    &process,
                    &description,
                    brief,
                    &builder,
                    BackgroundScenario::ForegroundDetached,
                )
            }
            Ok(_) => {
                self.foreground_completion_result(
                    &task_id,
                    &process,
                    &builder,
                    foreground_timeout_ms,
                )
                .await
            }
            Err(error) => ExecutableToolResult::error(error.to_string()),
        };
        collect_foreground_output.store(false, Ordering::Release);
        result
    }

    fn validate_run_request(
        &self,
        args: &BashInput,
        signal: &AbortSignal,
    ) -> Option<ExecutableToolResult> {
        if signal.aborted() {
            return Some(ExecutableToolResult::error(
                "Aborted before command started",
            ));
        }
        if args.command.is_empty() {
            return Some(ExecutableToolResult::error("Command cannot be empty."));
        }
        if args.run_in_background != Some(true) {
            return None;
        }
        if !self.allow_background() {
            return Some(ExecutableToolResult::error(
                "Background execution is not available for this agent because TaskOutput and TaskStop are not enabled.",
            ));
        }
        if args
            .description
            .as_deref()
            .is_none_or(|value| value.trim().is_empty())
        {
            return Some(ExecutableToolResult::error(
                "description is required when run_in_background is true.",
            ));
        }
        None
    }

    async fn foreground_completion_result(
        &self,
        task_id: &str,
        process: &Arc<dyn HostProcess>,
        builder: &Mutex<ToolResultBuilder>,
        foreground_timeout_ms: u64,
    ) -> ExecutableToolResult {
        let current = self.tasks.get_task(task_id);
        let exit_code = current
            .as_ref()
            .filter(|task| task.kind == "process")
            .and_then(|task| task.details.get("exitCode"))
            .and_then(Value::as_i64)
            .map(|value| value as i32)
            .or_else(|| process.exit_code());
        let user_cancellation = user_cancellation_reason().to_string();
        let result = {
            let mut builder = builder.lock();
            match current.as_ref().map(|task| task.base.status) {
                Some(AgentTaskStatus::TimedOut) => {
                    let label = format_timeout_label(foreground_timeout_ms);
                    builder.error(
                        &format!("Command killed by timeout ({label})"),
                        Some(format!("Killed by timeout ({label})")),
                    )
                }
                Some(AgentTaskStatus::Killed)
                    if current
                        .as_ref()
                        .and_then(|task| task.base.stop_reason.as_deref())
                        == Some(user_cancellation.as_str()) =>
                {
                    builder.error("Interrupted by user", Some("Interrupted by user".into()))
                }
                Some(AgentTaskStatus::Failed | AgentTaskStatus::Killed)
                    if current
                        .as_ref()
                        .and_then(|task| task.base.stop_reason.as_ref())
                        .is_some() =>
                {
                    let reason = current
                        .as_ref()
                        .and_then(|task| task.base.stop_reason.as_ref())
                        .expect("guarded above");
                    builder.error(reason, Some(reason.clone()))
                }
                _ if exit_code == Some(0) => builder.ok("Command executed successfully.", None),
                _ => {
                    if builder.n_chars() == 0 {
                        builder.write(&format!(
                            "Process exited with code {}",
                            exit_code.map_or_else(|| "null".into(), |code| code.to_string())
                        ));
                    }
                    let code = exit_code.map_or_else(|| "null".into(), |code| code.to_string());
                    builder.error(
                        &format!("Command failed with exit code: {code}."),
                        Some(format!("Failed with exit code: {code}")),
                    )
                }
            }
        };
        self.add_foreground_output_reference(task_id, result).await
    }

    async fn add_foreground_output_reference(
        &self,
        task_id: &str,
        result: ToolResultBuilderResult,
    ) -> ExecutableToolResult {
        let mut executable = builder_result(result.clone());
        if !result.truncated {
            return executable;
        }
        let Ok(output) = self.tasks.get_output_snapshot(task_id, 0).await else {
            return executable;
        };
        let Some(output_path) = output
            .full_output_available
            .then_some(output.output_path)
            .flatten()
        else {
            return executable;
        };
        let task_output_hint = if self.allow_background() {
            format!(", or TaskOutput(task_id=\"{task_id}\", block=false)")
        } else {
            String::new()
        };
        let reference = format!(
            "\n\n[Full output saved]\ntask_id: {task_id}\noutput_path: {output_path}\noutput_size_bytes: {}\nnext_step: Use Read with output_path to page through the full log{task_output_hint}.",
            output.output_size_bytes
        );
        if let crate::tool::ExecutableToolOutput::Text(text) = &mut executable.output {
            text.push_str(&reference);
        }
        executable
    }

    fn background_started_result(
        &self,
        task_id: &str,
        process: &Arc<dyn HostProcess>,
        description: &str,
        brief: String,
        builder: &Mutex<ToolResultBuilder>,
        scenario: BackgroundScenario,
    ) -> ExecutableToolResult {
        let status = self
            .tasks
            .get_task(task_id)
            .map(|task| status_name(task.base.status))
            .unwrap_or("running");
        let metadata = format!(
            "task_id: {task_id}\npid: {}\ndescription: {description}\nstatus: {status}\nautomatic_notification: true\n{}human_shell_hint: Tell the human to run /tasks to open the interactive background-task panel.",
            process.pid(),
            self.next_step_lines(scenario)
        );
        let foreground = builder.lock().ok("", None);
        let output = if foreground.output.is_empty() {
            metadata
        } else {
            format!("{metadata}\n\nforeground_output:\n{}", foreground.output)
        };
        let mut result = ExecutableToolResult::success(output);
        result.truncated = foreground.truncated.then_some(true);
        result.note = Some(brief);
        result
    }

    fn next_step_lines(&self, scenario: BackgroundScenario) -> String {
        if scenario == BackgroundScenario::ForegroundDetached {
            let avoid = if self.allow_background() {
                "do NOT wait, poll, or call TaskOutput on it"
            } else {
                "do NOT wait or poll"
            };
            return format!(
                "next_step: The task now runs in the background. You will be automatically notified when it completes — {avoid}; continue with your current work.\n"
            );
        }
        if !self.allow_background() {
            return "next_step: You will be automatically notified when it completes.\n".into();
        }
        "next_step: The completion arrives automatically in a later turn — do NOT wait, poll, or call TaskOutput on it; continue with your current work.\nnext_step: Use TaskStop only if the task must be cancelled.\n".into()
    }
}

#[async_trait]
impl ExecutableTool for BashTool {
    type Input = BashInput;

    fn tool(&self) -> &Tool {
        &self.definition
    }

    async fn resolve_execution(&self, args: BashInput) -> ToolExecution {
        let preview = preview(&args.command, 50);
        let description = if args.run_in_background == Some(true) {
            format!("Starting background: {preview}")
        } else {
            format!("Running: {preview}")
        };
        let display = ToolInputDisplay::Command {
            command: args.command.clone(),
            cwd: Some(args.cwd.clone().unwrap_or_else(|| self.context.cwd.clone())),
            description: args.description.clone(),
            language: Some(CommandLanguage::Bash),
        };
        let command = args.command.clone();
        let this = self.clone();
        let execute = Arc::new(move |context| {
            let this = this.clone();
            let args = args.clone();
            Box::pin(async move { this.execute(args, context).await })
                as BoxFuture<'static, ExecutableToolResult>
        });
        let mut execution =
            RunnableToolExecution::new(literal_rule_pattern("Bash", &command), execute);
        execution.accesses = Some(ToolAccess::all());
        execution.description = Some(description);
        execution.display = Some(display);
        execution.matches_rule = Some(Arc::new(move |rule_args| {
            matches_glob_rule_subject(rule_args, &command)
        }));
        ToolExecution::Runnable(execution)
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum BackgroundScenario {
    Started,
    ForegroundDetached,
}

pub fn register_bash_tool() {
    register_tool(
        Arc::new(|accessor| {
            BashTool::new(
                (*accessor.get(SESSION_PROCESS_RUNNER_SERVICE_ID)?).clone(),
                (*accessor.get(HOST_ENVIRONMENT_SERVICE_ID)?).clone(),
                (*accessor.get(SESSION_CONTEXT_ID)?).clone(),
                (*accessor.get(AGENT_TASK_SERVICE_ID)?).clone(),
                (*accessor.get(AGENT_TOOL_POLICY_SERVICE_ID)?).clone(),
                (*accessor.get(CONFIG_SERVICE_ID)?).clone(),
            )
            .map(|tool| Arc::new(tool) as Arc<dyn crate::tool::ErasedExecutableTool>)
            .map_err(|error| crate::_base::di::errors::DiError::Factory(error.to_string()))
        }),
        ToolContributionOptions::default(),
    );
}

fn builder_result(result: ToolResultBuilderResult) -> ExecutableToolResult {
    let mut output = if result.is_error {
        ExecutableToolResult::error(result.output)
    } else {
        ExecutableToolResult::success(result.output)
    };
    output.truncated = result.truncated.then_some(true);
    output.note = result.brief;
    output
}

fn normalize_timeout_ms(timeout: Option<u64>, background: bool) -> u64 {
    let default = if background {
        DEFAULT_BACKGROUND_TIMEOUT_S
    } else {
        DEFAULT_TIMEOUT_S
    };
    timeout.unwrap_or(default).min(timeout_cap_s(background)) * MS_PER_SECOND
}

fn timeout_cap_s(background: bool) -> u64 {
    if background {
        MAX_BACKGROUND_TIMEOUT_S
    } else {
        MAX_TIMEOUT_S
    }
}

fn render_bash_description(shell_name: &str) -> String {
    BASH_DESCRIPTION_TEMPLATE
        .replace("${SHELL_NAME}", shell_name)
        .replace("${DEFAULT_TIMEOUT_S}", &DEFAULT_TIMEOUT_S.to_string())
        .replace(
            "${DEFAULT_BACKGROUND_TIMEOUT_S}",
            &DEFAULT_BACKGROUND_TIMEOUT_S.to_string(),
        )
        .replace("${MAX_TIMEOUT_S}", &MAX_TIMEOUT_S.to_string())
        .replace(
            "${MAX_BACKGROUND_TIMEOUT_S}",
            &MAX_BACKGROUND_TIMEOUT_S.to_string(),
        )
}

fn without_background_description(description: &str) -> String {
    let background = Regex::new(
        r"(?s)\r?\n\r?\nIf `run_in_background=true`,.*?point them to the `/tasks` command, which opens an interactive panel; it has no subcommands\.",
    )
    .expect("static regex");
    let safety = format!(
        " For possibly long-running foreground commands, set the `timeout` argument in seconds. Foreground commands default to {DEFAULT_TIMEOUT_S}s and allow up to {MAX_TIMEOUT_S}s. When a foreground command hits its timeout it is moved to the background instead of being killed, and you will be automatically notified when it completes."
    );
    let no_background_safety = format!(
        " For possibly long-running commands, set the `timeout` argument in seconds. The default is {DEFAULT_TIMEOUT_S}s; foreground commands allow up to {MAX_TIMEOUT_S}s; a foreground command that hits its timeout is killed."
    );
    let efficiency = Regex::new(
        r"(?s)\r?\n- Prefer `run_in_background=true`.*?conversation to continue before the command finishes\.",
    )
    .expect("static regex");
    let description = background.replace(
        description,
        "\n\nBackground execution is disabled for this agent. Do not set `run_in_background=true`.",
    );
    let description = description.replace(&safety, &no_background_safety);
    efficiency
        .replace(
            &description,
            "\n- Do not set `run_in_background=true`; background task management tools are not available.",
        )
        .into_owned()
}

fn without_auto_background_on_timeout(description: &str) -> String {
    description.replace(
        " When a foreground command hits its timeout it is moved to the background instead of being killed, and you will be automatically notified when it completes.",
        " A foreground command that hits its timeout is killed.",
    )
}

fn format_timeout_label(timeout_ms: u64) -> String {
    if timeout_ms.is_multiple_of(MS_PER_SECOND) {
        format!("{}s", timeout_ms / MS_PER_SECOND)
    } else {
        format!("{timeout_ms}ms")
    }
}

fn foreground_description(args: &BashInput) -> String {
    if let Some(description) = args.description.as_deref().map(str::trim)
        && !description.is_empty()
    {
        return description.into();
    }
    format!("Bash: {}", preview(&args.command, 60))
}

fn preview(value: &str, max_units: usize) -> String {
    let units = value.encode_utf16().collect::<Vec<_>>();
    if units.len() <= max_units {
        value.into()
    } else {
        format!("{}…", String::from_utf16_lossy(&units[..max_units]))
    }
}

async fn close_process_stdin(process: &Arc<dyn HostProcess>) {
    let stdin = process.stdin();
    let _ = stdin.lock().await.shutdown().await;
}

async fn kill_spawned_process(process: &Arc<dyn HostProcess>) {
    let _ = process.kill(Some(ProcessSignal::Terminate)).await;
    process.dispose();
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn windows_path_to_posix_path(path: &str) -> String {
    if path.starts_with(r"\\") {
        return path.replace('\\', "/");
    }
    let bytes = path.as_bytes();
    if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
        let drive = (bytes[0] as char).to_ascii_lowercase();
        let rest = path[2..].replace('\\', "/");
        return format!(
            "/{drive}{}",
            if rest.starts_with('/') {
                rest
            } else {
                format!("/{rest}")
            }
        );
    }
    path.replace('\\', "/")
}

fn rewrite_windows_null_redirect(command: &str) -> String {
    Regex::new(r"(?i)(\d?&?>+\s*)nul(\s|$|[|&;)\n])")
        .expect("static regex")
        .replace_all(command, "${1}/dev/null$2")
        .into_owned()
}

fn status_name(status: AgentTaskStatus) -> &'static str {
    match status {
        AgentTaskStatus::Running => "running",
        AgentTaskStatus::Completed => "completed",
        AgentTaskStatus::Failed => "failed",
        AgentTaskStatus::TimedOut => "timed_out",
        AgentTaskStatus::Killed => "killed",
        AgentTaskStatus::Lost => "lost",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_and_timeout_defaults_match_typescript() {
        let schema = bash_parameters();
        assert_eq!(schema["additionalProperties"], false);
        assert_eq!(schema["properties"]["timeout"]["default"], 60);
        assert_eq!(normalize_timeout_ms(Some(1), false), 1_000);
        assert_eq!(normalize_timeout_ms(Some(999), false), 300_000);
        assert_eq!(normalize_timeout_ms(None, true), 600_000);
    }

    #[test]
    fn path_quoting_and_windows_redirects_match_git_bash() {
        assert_eq!(shell_quote("a'b"), "'a'\\''b'");
        assert_eq!(
            windows_path_to_posix_path(r"C:\work dir\a"),
            "/c/work dir/a"
        );
        assert_eq!(
            windows_path_to_posix_path(r"\\server\share\a"),
            "//server/share/a"
        );
        assert_eq!(
            rewrite_windows_null_redirect("thing >NUL 2>> nul && echo nul"),
            "thing >/dev/null 2>> /dev/null && echo nul"
        );
    }

    #[test]
    fn description_variants_preserve_prompt_contract() {
        let full = render_bash_description("bash");
        assert!(full.contains("Execute a `bash` command"));
        assert!(full.contains("Background commands default to a 600s timeout"));
        let disabled = without_background_description(&full);
        assert!(disabled.contains("Background execution is disabled for this agent"));
        assert!(!disabled.contains("Use `TaskOutput` only"));
        let no_auto = without_auto_background_on_timeout(&full);
        assert!(no_auto.contains("A foreground command that hits its timeout is killed."));
    }

    #[test]
    fn previews_count_utf16_units_like_javascript() {
        assert_eq!(preview("abc", 3), "abc");
        assert_eq!(preview("😀x", 2), "😀…");
    }
}
