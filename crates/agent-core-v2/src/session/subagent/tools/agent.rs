//! The model-facing Agent collaboration tool.
//!
//! Original: `session/subagent/tools/agent.ts`.

use std::{error::Error, io, sync::Arc};

use async_trait::async_trait;
use futures_util::{FutureExt, future::BoxFuture};
use serde::{Deserialize, Deserializer};
use serde_json::{Map, Value, json};

use crate::{
    _base::{
        di::instantiation::ServicesAccessorExt,
        lifecycle::lifecycle_machine::BoxError,
        log::{LOG_SERVICE_ID, LogPayload, LogServiceHandle, Logger},
        utils::abort::{
            AbortController, AbortError, AbortSignal, is_abort_error, link_abort_signal,
        },
    },
    agent::{
        loop_::{AGENT_LOOP_SERVICE_ID, AgentLoopState},
        permission_mode::{AGENT_PERMISSION_MODE_SERVICE_ID, AgentPermissionModeServiceHandle},
        profile::{
            AGENT_PROFILE_SERVICE_ID, AgentProfileServiceHandle, BindAgentInput, ProfileUpdateData,
        },
        scope_context::{AGENT_SCOPE_CONTEXT_ID, AgentScopeContext},
        task::{
            AGENT_TASK_SERVICE_ID, AgentTaskServiceHandle, ForegroundTaskReleaseReason,
            RegisterAgentTaskOptions, types::AgentTaskStatus,
        },
        tool_policy::{
            AGENT_TOOL_POLICY_SERVICE_ID, AgentToolPolicyServiceHandle, ToolActivationPolicy,
            is_tool_active, resolve_active_tool_names,
        },
        tool_registry::{
            AGENT_TOOL_REGISTRY_SERVICE_ID, AgentToolRegistryServiceHandle,
            ToolContributionOptions, ToolReference, register_tool,
        },
        user_tool::AGENT_USER_TOOL_SERVICE_ID,
    },
    app::{
        agent_profile_catalog::{
            AgentProfile, AgentProfilePromptPrefixContext, apply_profile_prompt_prefix,
            subagent_allowlist_for, subagent_model_alias, subagent_type_not_allowed_message,
        },
        config::{CONFIG_SERVICE_ID, ConfigServiceHandle},
    },
    kosong::contract::tool::Tool,
    session::{
        agent_lifecycle::{
            AGENT_LIFECYCLE_SERVICE_ID, AgentLifecycleServiceHandle, CreateAgentOptions,
            is_subagent_meta, subagent_labels, subagent_parent_agent_id,
        },
        agent_profile_catalog::{
            SESSION_AGENT_PROFILE_CATALOG_ID, SessionAgentProfileCatalogHandle,
        },
        process::{SESSION_PROCESS_RUNNER_SERVICE_ID, SessionProcessRunnerHandle},
        session_metadata::{SESSION_METADATA_ID, SessionMetadataHandle},
        workspace_context::{SESSION_WORKSPACE_CONTEXT_ID, SessionWorkspaceContextHandle},
    },
    tool::{
        ExecutableTool, ExecutableToolContext, ExecutableToolResult, RunnableToolExecution,
        ToolAccess, ToolExecution, ToolInputDisplay, ToolSource,
        input_schema::to_input_json_schema, rule_match::matches_glob_rule_subject,
    },
};

use super::subagent_task::{
    SubagentCompletion, SubagentCompletionFuture, SubagentHandle, SubagentTask,
};
use crate::session::subagent::{
    AgentRunRequest, AgentRunSpawnedMeta, MirrorAgentRunOptions, RunAgentOptions,
    SESSION_SUBAGENT_SERVICE_ID, SessionSubagentServiceHandle, SharedAgentRunError,
    emit_agent_run_spawned, format_subagent_timeout_description, mirror_agent_run,
    resolve_subagent_timeout_ms,
};

pub const DEFAULT_PROFILE_NAME: &str = "coder";
const RESUMED_LABEL: &str = "subagent";
const BACKGROUND_AGENT_UNAVAILABLE: &str = "Background agent execution is not available for this agent because TaskList, TaskOutput, and TaskStop are not enabled.";
pub const RESUME_WITH_TYPE_UNAVAILABLE: &str =
    "Cannot set subagent_type when resuming an existing agent. Resume by agent id only.";
pub const USER_INTERRUPTED_SUBAGENT_MESSAGE: &str =
    "The subagent was stopped before it finished by user.";
pub const SUBAGENT_STOPPED_MESSAGE: &str = "The subagent was stopped before it finished.";
const AGENT_DESCRIPTION_BASE: &str = include_str!("agent.md");
const AGENT_BACKGROUND_DESCRIPTION: &str = include_str!("agent-background-enabled.md");
const AGENT_BACKGROUND_DISABLED_DESCRIPTION: &str = include_str!("agent-background-disabled.md");

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentToolInput {
    pub prompt: String,
    pub description: String,
    pub subagent_type: Option<String>,
    pub resume: Option<String>,
    pub run_in_background: Option<bool>,
}

impl<'de> Deserialize<'de> for AgentToolInput {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = Value::deserialize(deserializer)?;
        parse_agent_tool_input(&value).map_err(serde::de::Error::custom)
    }
}

pub fn parse_agent_tool_input(value: &Value) -> Result<AgentToolInput, String> {
    let object = value
        .as_object()
        .ok_or_else(|| "Agent input must be an object".to_owned())?;
    let required_string = |name: &str| {
        object
            .get(name)
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| format!("{name} must be a string"))
    };
    let optional_string = |name: &str| match object.get(name) {
        None => Ok(None),
        Some(Value::String(value)) => Ok(Some(value.clone())),
        Some(_) => Err(format!("{name} must be a string")),
    };
    let run_in_background = match object.get("run_in_background") {
        None => None,
        Some(Value::Bool(value)) => Some(*value),
        Some(_) => return Err("run_in_background must be a boolean".into()),
    };
    Ok(normalize_agent_tool_input(AgentToolInput {
        prompt: required_string("prompt")?,
        description: required_string("description")?,
        subagent_type: optional_string("subagent_type")?,
        resume: optional_string("resume")?,
        run_in_background,
    }))
}

pub fn normalize_agent_tool_input(mut input: AgentToolInput) -> AgentToolInput {
    let has_resume = input
        .resume
        .as_ref()
        .is_some_and(|value| !value.trim().is_empty());
    let has_type = input
        .subagent_type
        .as_ref()
        .is_some_and(|value| !value.is_empty());
    if !has_type && !has_resume {
        input.subagent_type = Some(DEFAULT_PROFILE_NAME.into());
    } else if !has_type {
        input.subagent_type = None;
    }
    input
}

pub fn validate_agent_tool_input(input: &AgentToolInput) -> Result<(), &'static str> {
    if resume_agent_id(input).is_some() && requested_profile_name(input).is_some() {
        Err(RESUME_WITH_TYPE_UNAVAILABLE)
    } else {
        Ok(())
    }
}

pub fn agent_parameters() -> Map<String, Value> {
    to_input_json_schema(
        json!({
            "type": "object",
            "properties": {
                "prompt": {
                    "type": "string",
                    "description": "Full task prompt for the subagent"
                },
                "description": {
                    "type": "string",
                    "description": "Short task description (3-5 words) for UI display"
                },
                "subagent_type": {
                    "type": "string",
                    "description": "One of the available agent types (see \"Available agent types\" in this tool description). Defaults to \"coder\" when omitted."
                },
                "resume": {
                    "type": "string",
                    "description": "Optional agent ID to resume instead of creating a new instance. When set, do not also pass subagent_type — the resumed agent keeps its own type, and supplying both is rejected."
                },
                "run_in_background": {
                    "type": "boolean",
                    "description": "If true, return immediately without waiting for completion. Prefer false unless the task can run independently and there is a clear benefit to not waiting."
                }
            },
            "required": ["prompt", "description"]
        })
        .as_object()
        .cloned()
        .expect("Agent schema is an object"),
    )
}

#[derive(Clone)]
pub struct AgentTool {
    lifecycle: AgentLifecycleServiceHandle,
    subagents: SessionSubagentServiceHandle,
    catalog: SessionAgentProfileCatalogHandle,
    caller_agent_id: String,
    tasks: AgentTaskServiceHandle,
    profile: AgentProfileServiceHandle,
    tool_policy: AgentToolPolicyServiceHandle,
    tool_registry: AgentToolRegistryServiceHandle,
    workspace: SessionWorkspaceContextHandle,
    process_runner: SessionProcessRunnerHandle,
    metadata: SessionMetadataHandle,
    log: LogServiceHandle,
    permission_mode: AgentPermissionModeServiceHandle,
    config: ConfigServiceHandle,
    definition: Tool,
}

impl AgentTool {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        lifecycle: AgentLifecycleServiceHandle,
        subagents: SessionSubagentServiceHandle,
        catalog: SessionAgentProfileCatalogHandle,
        scope_context: AgentScopeContext,
        tasks: AgentTaskServiceHandle,
        profile: AgentProfileServiceHandle,
        tool_policy: AgentToolPolicyServiceHandle,
        tool_registry: AgentToolRegistryServiceHandle,
        workspace: SessionWorkspaceContextHandle,
        process_runner: SessionProcessRunnerHandle,
        metadata: SessionMetadataHandle,
        log: LogServiceHandle,
        permission_mode: AgentPermissionModeServiceHandle,
        config: ConfigServiceHandle,
    ) -> Self {
        let mut tool = Self {
            lifecycle,
            subagents,
            catalog,
            caller_agent_id: scope_context.agent_id,
            tasks,
            profile,
            tool_policy,
            tool_registry,
            workspace,
            process_runner,
            metadata,
            log,
            permission_mode,
            config,
            definition: Tool {
                name: "Agent".into(),
                description: String::new(),
                parameters: agent_parameters(),
                deferred: None,
            },
        };
        tool.definition.description = tool.description();
        tool
    }

    fn can_run_in_background(&self) -> bool {
        ["TaskList", "TaskOutput", "TaskStop"].iter().all(|name| {
            self.tool_policy
                .is_tool_active(name, ToolSource::Builtin)
                .unwrap_or(false)
        })
    }

    fn description(&self) -> String {
        let background = if self.can_run_in_background() {
            AGENT_BACKGROUND_DESCRIPTION
        } else {
            AGENT_BACKGROUND_DISABLED_DESCRIPTION.trim_end_matches(['\r', '\n'])
        };
        let base = format!("{AGENT_DESCRIPTION_BASE}\n\n{background}");
        let Ok(own) = self.profile.data() else {
            return base;
        };
        let Ok(default_profile) = self.catalog.get_default() else {
            return base;
        };
        let allowlist = subagent_allowlist_for(
            default_profile.subagents.as_deref(),
            own.config.profile_name.as_deref(),
            own.subagents.as_deref(),
        );
        let profiles = self
            .catalog
            .list()
            .into_iter()
            .filter(|profile| allowlist.is_none_or(|names| names.contains(&profile.name)))
            .collect::<Vec<_>>();
        let type_lines = build_profile_descriptions(
            &profiles,
            &self.tool_registry.list_references(),
            |profile, name, source| {
                self.tool_policy
                    .is_tool_active_for_profile(profile, name, source)
            },
        );
        if type_lines.is_empty() {
            base
        } else {
            format!("{base}\n\nAvailable agent types (pass via subagent_type):\n{type_lines}")
        }
    }

    fn resume_profile_name(&self, agent_id: &str) -> Option<String> {
        self.lifecycle
            .get(agent_id)?
            .get(AGENT_PROFILE_SERVICE_ID)
            .ok()?
            .data()
            .ok()?
            .config
            .profile_name
    }

    async fn launch(
        &self,
        args: &AgentToolInput,
        tool_call_id: &str,
        controller: &AbortController,
    ) -> Result<SubagentHandle, BoxError> {
        let requester = self.lifecycle.get(&self.caller_agent_id).ok_or_else(|| {
            other_error(format!(
                "Caller agent \"{}\" does not exist",
                self.caller_agent_id
            ))
        })?;
        let (agent_id, profile_name, prompt_text) = if let Some(agent_id) = resume_agent_id(args) {
            let target = self.lifecycle.get(agent_id).ok_or_else(|| {
                other_error(format!("Agent instance \"{agent_id}\" does not exist"))
            })?;
            self.ensure_owned_idle_subagent(agent_id, &target).await?;
            self.realign_child_model(&target)?;
            let profile_name = target
                .get(AGENT_PROFILE_SERVICE_ID)?
                .data()?
                .config
                .profile_name
                .unwrap_or_else(|| RESUMED_LABEL.into());
            (target.id().to_owned(), profile_name, args.prompt.clone())
        } else {
            let requested_profile_name =
                requested_profile_name(args).unwrap_or(DEFAULT_PROFILE_NAME);
            self.catalog.ready().await?;
            let own = self.profile.data()?;
            let default_profile = self.catalog.get_default()?;
            let allowlist = subagent_allowlist_for(
                default_profile.subagents.as_deref(),
                own.config.profile_name.as_deref(),
                own.subagents.as_deref(),
            );
            if let Some(allowlist) = allowlist
                && !allowlist.iter().any(|name| name == requested_profile_name)
            {
                return Err(other_error(subagent_type_not_allowed_message(
                    requested_profile_name,
                    allowlist,
                )));
            }
            let profile = self.catalog.get(requested_profile_name).ok_or_else(|| {
                other_error(format!("Unknown agent type: \"{requested_profile_name}\""))
            })?;
            let caller_model = own
                .config
                .model_alias
                .clone()
                .ok_or_else(|| other_error("Caller agent has no model bound"))?;
            let model = subagent_model_alias(Some(profile.as_ref()), caller_model);
            let created = self
                .lifecycle
                .create(CreateAgentOptions {
                    binding: Some(BindAgentInput {
                        profile: profile.name.clone(),
                        model: Some(model),
                        thinking: Some(own.config.thinking_level),
                        strict_thinking: None,
                        cwd: Some(own.config.cwd),
                    }),
                    labels: Some(subagent_labels(&self.caller_agent_id, None)),
                    ..CreateAgentOptions::default()
                })
                .await?;
            created
                .get(AGENT_PERMISSION_MODE_SERVICE_ID)?
                .set_mode(self.permission_mode.mode())?;
            created
                .get(AGENT_USER_TOOL_SERVICE_ID)?
                .inherit_user_tools(&requester.get(AGENT_USER_TOOL_SERVICE_ID)?.0)
                .await
                .map_err(other_error)?;
            let logger: Arc<dyn Logger> = self.log.0.clone();
            let prompt = apply_profile_prompt_prefix(
                &profile,
                &args.prompt,
                AgentProfilePromptPrefixContext {
                    cwd: self.workspace.work_dir().to_string_lossy().into_owned(),
                    runner: self.process_runner.clone(),
                    log: Some(logger),
                },
            )
            .await;
            (created.id().to_owned(), profile.name.clone(), prompt)
        };

        let run_in_background = args.run_in_background == Some(true);
        emit_agent_run_spawned(
            &requester,
            &agent_id,
            &AgentRunSpawnedMeta {
                profile_name: profile_name.clone(),
                parent_tool_call_id: Some(tool_call_id.into()),
                parent_tool_call_uuid: None,
                description: Some(args.description.clone()),
                swarm_index: None,
                run_in_background: Some(run_in_background),
            },
        );
        let run = self
            .subagents
            .run(
                agent_id.clone(),
                AgentRunRequest::Prompt {
                    prompt: prompt_text.clone(),
                },
                RunAgentOptions::new(controller.signal()),
            )
            .await?;
        let completion_profile_name = profile_name.clone();
        let mirror_controller = controller.clone();
        let cancel_controller = controller.clone();
        let completion: SubagentCompletionFuture = async move {
            mirror_agent_run(
                &requester,
                run,
                MirrorAgentRunOptions {
                    profile_name: completion_profile_name,
                    prompt: Some(prompt_text),
                    suppress_rate_limit_failure_event: false,
                    signal: mirror_controller.signal(),
                    cancel: Some(Arc::new(move |reason| {
                        cancel_controller.abort(reason.map(abort_reason));
                    })),
                },
            )
            .await
            .map(|result| SubagentCompletion {
                result: result.summary,
                usage: result.usage,
            })
            .map_err(shared_error)
        }
        .boxed()
        .shared();
        Ok(SubagentHandle {
            agent_id,
            profile_name,
            completion,
        })
    }

    async fn ensure_owned_idle_subagent(
        &self,
        agent_id: &str,
        target: &crate::_base::di::scope::ScopeHandle,
    ) -> Result<(), BoxError> {
        let meta = self
            .metadata
            .read()
            .await?
            .agents
            .and_then(|agents| agents.get(agent_id).cloned());
        if !is_subagent_meta(meta.as_ref()) {
            return Err(other_error(format!(
                "Agent instance \"{agent_id}\" is not a subagent"
            )));
        }
        if subagent_parent_agent_id(meta.as_ref()).as_deref() != Some(&self.caller_agent_id) {
            return Err(other_error(format!(
                "Agent instance \"{agent_id}\" does not belong to this parent agent"
            )));
        }
        if target.get(AGENT_LOOP_SERVICE_ID)?.status().state == AgentLoopState::Running {
            return Err(other_error(format!(
                "Agent instance \"{agent_id}\" is already running and cannot run concurrently"
            )));
        }
        Ok(())
    }

    fn realign_child_model(
        &self,
        target: &crate::_base::di::scope::ScopeHandle,
    ) -> Result<(), BoxError> {
        let caller_model = self
            .profile
            .data()?
            .config
            .model_alias
            .ok_or_else(|| other_error("Caller agent has no model bound"))?;
        let child_profile_name = target
            .get(AGENT_PROFILE_SERVICE_ID)?
            .data()?
            .config
            .profile_name;
        let child_profile = child_profile_name
            .as_deref()
            .and_then(|name| self.catalog.get(name));
        target
            .get(AGENT_PROFILE_SERVICE_ID)?
            .update(ProfileUpdateData {
                model_alias: Some(subagent_model_alias(
                    child_profile.as_deref(),
                    caller_model,
                )),
                ..ProfileUpdateData::default()
            })?;
        Ok(())
    }

    async fn execution(
        &self,
        args: AgentToolInput,
        context: ExecutableToolContext,
    ) -> ExecutableToolResult {
        match self.execute_inner(args, &context).await {
            Ok(result) => result,
            Err(error) => ExecutableToolResult::error(format!(
                "subagent error: {}",
                launch_error_message(error.as_ref(), &context.signal)
            )),
        }
    }

    async fn execute_inner(
        &self,
        args: AgentToolInput,
        context: &ExecutableToolContext,
    ) -> Result<ExecutableToolResult, BoxError> {
        context
            .signal
            .throw_if_aborted()
            .map_err(|error| Box::new((*error).clone()) as BoxError)?;
        if let Err(message) = validate_agent_tool_input(&args) {
            return Ok(ExecutableToolResult::error(message));
        }

        let run_in_background = args.run_in_background == Some(true);
        let requested_profile = requested_profile_name(&args);
        let resume_id = resume_agent_id(&args);
        let allow_background = self.can_run_in_background();
        if run_in_background && !allow_background {
            return Ok(ExecutableToolResult::error(BACKGROUND_AGENT_UNAVAILABLE));
        }
        let timeout_ms = resolve_subagent_timeout_ms(self.config.0.as_ref());
        let controller = AbortController::new();
        let abort_before_register =
            (!run_in_background).then(|| link_abort_signal(&context.signal, controller.clone()));

        let handle = match self.launch(&args, &context.tool_call_id, &controller).await {
            Ok(handle) => handle,
            Err(error) => {
                drop(abort_before_register);
                self.log_launch_failure(
                    &context.tool_call_id,
                    run_in_background,
                    resume_id.is_some(),
                    requested_profile.unwrap_or(DEFAULT_PROFILE_NAME),
                    resume_id,
                    error.as_ref(),
                );
                return Err(error);
            }
        };

        let task_id = match self.tasks.register_task(
            Arc::new(SubagentTask::new(
                handle.clone(),
                args.description.clone(),
                controller.clone(),
            )),
            RegisterAgentTaskOptions {
                detached: Some(run_in_background),
                timeout_ms: Some(timeout_ms),
                signal: (!run_in_background).then_some(context.signal.clone()),
                ..RegisterAgentTaskOptions::default()
            },
        ) {
            Ok(task_id) => task_id,
            Err(error) => {
                controller.abort(None);
                let completion = handle.completion.clone();
                tokio::spawn(async move {
                    let _ = completion.await;
                });
                drop(abort_before_register);
                self.log_task_registration_failure(&context.tool_call_id, &handle, error.as_ref());
                let message = error.to_string();
                let message = if message == "Too many detached tasks are already running." {
                    "Too many background tasks are already running.".into()
                } else {
                    message
                };
                return Ok(ExecutableToolResult::error(message));
            }
        };
        drop(abort_before_register);

        if run_in_background {
            return Ok(ExecutableToolResult::success(
                format_background_agent_result(
                    &task_id,
                    &handle,
                    &args.description,
                    allow_background,
                ),
            ));
        }

        if matches!(
            self.tasks.wait_for_foreground_release(&task_id).await?,
            Some(
                ForegroundTaskReleaseReason::Detached
                    | ForegroundTaskReleaseReason::TimeoutDetached
            )
        ) {
            return Ok(ExecutableToolResult::success(
                format_background_agent_result(
                    &task_id,
                    &handle,
                    &args.description,
                    allow_background,
                ),
            ));
        }
        self.format_foreground_result(&task_id, &handle, timeout_ms)
            .await
    }

    async fn format_foreground_result(
        &self,
        task_id: &str,
        handle: &SubagentHandle,
        timeout_ms: u64,
    ) -> Result<ExecutableToolResult, BoxError> {
        let info = self.tasks.get_task(task_id);
        if info
            .as_ref()
            .is_some_and(|info| info.base.status == AgentTaskStatus::Completed)
        {
            let output = self.tasks.read_output(task_id, None).await?;
            return Ok(ExecutableToolResult::success(
                format_foreground_agent_success(handle, &output),
            ));
        }
        let timed_out = info
            .as_ref()
            .is_some_and(|info| info.base.status == AgentTaskStatus::TimedOut);
        let message = if timed_out {
            format!(
                "Agent timed out after {}.",
                format_subagent_timeout_description(timeout_ms)
            )
        } else {
            format_subagent_stopped_message(
                info.as_ref()
                    .and_then(|info| info.base.stop_reason.as_deref()),
            )
        };
        Ok(ExecutableToolResult::error(
            format_foreground_agent_failure(handle, &message, timed_out),
        ))
    }

    fn log_launch_failure(
        &self,
        tool_call_id: &str,
        run_in_background: bool,
        is_resume: bool,
        subagent_type: &str,
        resume_agent_id: Option<&str>,
        error: &(dyn Error + 'static),
    ) {
        let mut context = Map::from_iter([
            ("toolCallId".into(), Value::String(tool_call_id.into())),
            ("runInBackground".into(), Value::Bool(run_in_background)),
            (
                "operation".into(),
                Value::String(if is_resume { "resume" } else { "spawn" }.into()),
            ),
            ("subagentType".into(), Value::String(subagent_type.into())),
            ("error".into(), Value::String(error.to_string())),
        ]);
        if let Some(resume_agent_id) = resume_agent_id {
            context.insert(
                "resumeAgentId".into(),
                Value::String(resume_agent_id.into()),
            );
        }
        self.log
            .0
            .warn("subagent launch failed", Some(LogPayload::Context(context)));
    }

    fn log_task_registration_failure(
        &self,
        tool_call_id: &str,
        handle: &SubagentHandle,
        error: &(dyn Error + 'static),
    ) {
        self.log.0.warn(
            "background agent task registration failed",
            Some(LogPayload::Context(Map::from_iter([
                ("toolCallId".into(), Value::String(tool_call_id.into())),
                ("agentId".into(), Value::String(handle.agent_id.clone())),
                (
                    "subagentType".into(),
                    Value::String(handle.profile_name.clone()),
                ),
                ("error".into(), Value::String(error.to_string())),
            ]))),
        );
    }
}

#[async_trait]
impl ExecutableTool for AgentTool {
    type Input = AgentToolInput;

    fn tool(&self) -> &Tool {
        &self.definition
    }

    fn current_tool(&self) -> Tool {
        let mut definition = self.definition.clone();
        definition.description = self.description();
        definition
    }

    async fn resolve_execution(&self, args: AgentToolInput) -> ToolExecution {
        let args = normalize_agent_tool_input(args);
        if let Err(message) = validate_agent_tool_input(&args) {
            return ToolExecution::Error(ExecutableToolResult::error(message));
        }
        let resume_id = resume_agent_id(&args);
        let requested_profile = requested_profile_name(&args);
        let profile_name = resume_id
            .and_then(|agent_id| self.resume_profile_name(agent_id))
            .unwrap_or_else(|| {
                if resume_id.is_some() {
                    RESUMED_LABEL.into()
                } else {
                    requested_profile.unwrap_or(DEFAULT_PROFILE_NAME).into()
                }
            });
        let prefix = if args.run_in_background == Some(true) {
            "Launching background"
        } else {
            "Launching"
        };
        let this = self.clone();
        let execution_args = args.clone();
        let execute = Arc::new(move |context| {
            let this = this.clone();
            let args = execution_args.clone();
            Box::pin(async move { this.execution(args, context).await })
                as BoxFuture<'static, ExecutableToolResult>
        });
        let mut execution = RunnableToolExecution::new("Agent", execute);
        execution.description = Some(format!(
            "{prefix} {profile_name} agent: {}",
            args.description
        ));
        execution.accesses = Some(ToolAccess::none());
        execution.display = Some(ToolInputDisplay::AgentCall {
            agent_name: profile_name.clone(),
            prompt: args.prompt,
            background: args.run_in_background,
        });
        execution.matches_rule = Some(Arc::new(move |rule_args| {
            matches_glob_rule_subject(rule_args, &profile_name)
        }));
        ToolExecution::Runnable(execution)
    }
}

pub fn register_agent_tool() {
    register_tool(
        Arc::new(|accessor| {
            Ok(Arc::new(AgentTool::new(
                (*accessor.get(AGENT_LIFECYCLE_SERVICE_ID)?).clone(),
                (*accessor.get(SESSION_SUBAGENT_SERVICE_ID)?).clone(),
                (*accessor.get(SESSION_AGENT_PROFILE_CATALOG_ID)?).clone(),
                (*accessor.get(AGENT_SCOPE_CONTEXT_ID)?).clone(),
                (*accessor.get(AGENT_TASK_SERVICE_ID)?).clone(),
                (*accessor.get(AGENT_PROFILE_SERVICE_ID)?).clone(),
                (*accessor.get(AGENT_TOOL_POLICY_SERVICE_ID)?).clone(),
                (*accessor.get(AGENT_TOOL_REGISTRY_SERVICE_ID)?).clone(),
                (*accessor.get(SESSION_WORKSPACE_CONTEXT_ID)?).clone(),
                (*accessor.get(SESSION_PROCESS_RUNNER_SERVICE_ID)?).clone(),
                (*accessor.get(SESSION_METADATA_ID)?).clone(),
                (*accessor.get(LOG_SERVICE_ID)?).clone(),
                (*accessor.get(AGENT_PERMISSION_MODE_SERVICE_ID)?).clone(),
                (*accessor.get(CONFIG_SERVICE_ID)?).clone(),
            )) as Arc<dyn crate::tool::ErasedExecutableTool>)
        }),
        ToolContributionOptions::default(),
    );
}

fn requested_profile_name(input: &AgentToolInput) -> Option<&str> {
    input
        .subagent_type
        .as_deref()
        .filter(|value| !value.is_empty())
}

fn resume_agent_id(input: &AgentToolInput) -> Option<&str> {
    input
        .resume
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn build_profile_descriptions(
    profiles: &[Arc<AgentProfile>],
    tools: &[ToolReference],
    is_tool_active_for_profile: impl Fn(&ToolActivationPolicy, &str, ToolSource) -> bool,
) -> String {
    profiles
        .iter()
        .map(|profile| {
            let details = [
                profile.description.as_deref(),
                profile.when_to_use.as_deref(),
            ]
            .into_iter()
            .flatten()
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>();
            let header = if details.is_empty() {
                format!("- {}", profile.name)
            } else {
                format!("- {}: {}", profile.name, details.join(" "))
            };
            let model_line = profile
                .model
                .as_deref()
                .map(|model| format!("\n  Model: {model}"))
                .unwrap_or_default();
            let policy = ToolActivationPolicy {
                tools: profile.tools.clone(),
                disallowed_tools: profile.disallowed_tools.clone(),
            };
            let active_tools = resolve_active_tool_names(&policy);
            let externally_restricted = tools.iter().any(|tool| {
                is_tool_active(&policy, &tool.name, tool.source)
                    && !is_tool_active_for_profile(&policy, &tool.name, tool.source)
            });
            if externally_restricted {
                let effective_tools = tools
                    .iter()
                    .filter(|tool| is_tool_active_for_profile(&policy, &tool.name, tool.source))
                    .map(|tool| tool.name.as_str())
                    .collect::<Vec<_>>();
                return if effective_tools.is_empty() {
                    format!("{header}\n  Tools: none{model_line}")
                } else {
                    format!("{header}\n  Tools: {}{model_line}", effective_tools.join(", "))
                };
            }
            match active_tools {
                None if profile
                    .disallowed_tools
                    .as_ref()
                    .is_some_and(|tools| !tools.is_empty()) =>
                {
                    format!(
                        "{header}\n  Tools: all except {}{model_line}",
                        profile
                            .disallowed_tools
                            .as_ref()
                            .expect("guarded above")
                            .join(", ")
                    )
                }
                None => format!("{header}\n  Tools: all{model_line}"),
                Some(tools) if tools.is_empty() => format!("{header}\n  Tools: none{model_line}"),
                Some(tools) => format!("{header}\n  Tools: {}{model_line}", tools.join(", ")),
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn format_background_agent_result(
    task_id: &str,
    handle: &SubagentHandle,
    description: &str,
    allow_background: bool,
) -> String {
    let next = if allow_background {
        "next_step: The completion arrives automatically in a later turn — do NOT wait, poll, or call TaskOutput on it; continue with other work or hand back to the user. (If you have nothing to do until it finishes, run such tasks in the foreground next time.)"
    } else {
        "next_step: The completion arrives automatically in a later turn."
    };
    format!(
        "task_id: {task_id}\nstatus: running\nagent_id: {}\nactual_subagent_type: {}\nautomatic_notification: true\n\ndescription: {description}\n\n{next}\nresume_hint: To continue or recover this same subagent later, call Agent(resume=\"{}\", prompt=\"...\"). The parameter is agent_id (\"{}\"), NOT task_id (\"{task_id}\") or source_id from a later <notification>. Recovery cases: a later <notification type=\"task.lost\" | \"task.failed\" | \"task.killed\"> for this subagent — its conversation history is preserved across session restarts and resume will pick it up.",
        handle.agent_id, handle.profile_name, handle.agent_id, handle.agent_id
    )
}

pub fn format_foreground_agent_success(handle: &SubagentHandle, result: &str) -> String {
    format!(
        "agent_id: {}\nactual_subagent_type: {}\nstatus: completed\n\n[summary]\n{result}",
        handle.agent_id, handle.profile_name
    )
}

pub fn format_foreground_agent_failure(
    handle: &SubagentHandle,
    message: &str,
    timed_out: bool,
) -> String {
    let mut text = format!(
        "agent_id: {}\nactual_subagent_type: {}\nstatus: failed\n\nsubagent error: {message}",
        handle.agent_id, handle.profile_name
    );
    if timed_out {
        text.push_str(&format!(
            "\nresume_hint: Continue with Agent(resume=\"{}\", prompt=\"continue\"). Use agent_id only; do not set subagent_type. The subagent retains its prior context; redo any unfinished tool call if its result was lost.",
            handle.agent_id
        ));
    }
    text
}

pub fn format_subagent_stopped_message(reason: Option<&str>) -> String {
    match reason.map(str::trim).filter(|value| !value.is_empty()) {
        Some("Aborted by the user") => USER_INTERRUPTED_SUBAGENT_MESSAGE.into(),
        Some(reason) => format!("{SUBAGENT_STOPPED_MESSAGE} Reason: {reason}"),
        None => SUBAGENT_STOPPED_MESSAGE.into(),
    }
}

fn launch_error_message(error: &(dyn Error + 'static), signal: &AbortSignal) -> String {
    if signal
        .reason()
        .as_deref()
        .is_some_and(AbortError::is_user_cancellation)
    {
        return USER_INTERRUPTED_SUBAGENT_MESSAGE.into();
    }
    if is_abort_error(error) {
        let reason = signal.reason().map(|reason| reason.to_string());
        return format_subagent_stopped_message(reason.as_deref());
    }
    error.to_string()
}

fn shared_error(error: BoxError) -> Arc<dyn Error + Send + Sync> {
    match error.downcast::<SharedAgentRunError>() {
        Ok(error) => error.0,
        Err(error) => Arc::from(error),
    }
}

fn abort_reason(error: BoxError) -> AbortError {
    match error.downcast::<AbortError>() {
        Ok(error) => *error,
        Err(error) => AbortError::new(error.to_string()),
    }
}

fn other_error(message: impl Into<String>) -> BoxError {
    Box::new(io::Error::other(message.into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        app::agent_profile_catalog::AgentProfileContext, kosong::contract::usage::TokenUsage,
    };

    fn input(resume: Option<&str>, profile: Option<&str>) -> AgentToolInput {
        AgentToolInput {
            prompt: "p".into(),
            description: "d".into(),
            resume: resume.map(Into::into),
            subagent_type: profile.map(Into::into),
            run_in_background: None,
        }
    }

    fn profile(
        name: &str,
        description: Option<&str>,
        tools: Option<&[&str]>,
        disallowed_tools: Option<&[&str]>,
    ) -> Arc<AgentProfile> {
        Arc::new(AgentProfile {
            name: name.into(),
            description: description.map(Into::into),
            when_to_use: None,
            is_override: None,
            tools: tools.map(|tools| tools.iter().map(|tool| (*tool).into()).collect()),
            disallowed_tools: disallowed_tools
                .map(|tools| tools.iter().map(|tool| (*tool).into()).collect()),
            subagents: None,
            model: None,
            system_prompt: Arc::new(|_: &AgentProfileContext| String::new()),
            prompt_prefix: None,
            summary_policy: None,
        })
    }

    fn handle() -> SubagentHandle {
        SubagentHandle {
            agent_id: "agent-1".into(),
            profile_name: "coder".into(),
            completion: futures_util::future::ready(Ok(SubagentCompletion {
                result: "done".into(),
                usage: Some(TokenUsage::default()),
            }))
            .boxed()
            .shared(),
        }
    }

    #[test]
    fn schema_and_deserialization_match_source_preprocessing() {
        let schema = agent_parameters();
        assert_eq!(schema["additionalProperties"], false);
        assert!(schema["properties"].get("run_in_background").is_some());
        assert!(schema["properties"].get("runInBackground").is_none());
        assert!(schema["properties"].get("timeout").is_none());
        assert!(schema["properties"].get("model").is_none());

        let defaulted: AgentToolInput =
            serde_json::from_value(json!({"prompt": "p", "description": "d"})).unwrap();
        assert_eq!(defaulted.subagent_type.as_deref(), Some("coder"));
        let empty: AgentToolInput =
            serde_json::from_value(json!({"prompt": "p", "description": "d", "subagent_type": ""}))
                .unwrap();
        assert_eq!(empty.subagent_type.as_deref(), Some("coder"));
        let resumed: AgentToolInput =
            serde_json::from_value(json!({"prompt": "p", "description": "d", "resume": "agent-1"}))
                .unwrap();
        assert!(resumed.subagent_type.is_none());
        let with_unknown: AgentToolInput = serde_json::from_value(
            json!({"prompt": "p", "description": "d", "unknown": "stripped"}),
        )
        .unwrap();
        assert_eq!(with_unknown.subagent_type.as_deref(), Some("coder"));
        for invalid in [
            json!(null),
            json!({"prompt": "p"}),
            json!({"prompt": "p", "description": "d", "subagent_type": null}),
            json!({"prompt": "p", "description": "d", "resume": null}),
            json!({"prompt": "p", "description": "d", "run_in_background": null}),
        ] {
            assert!(
                serde_json::from_value::<AgentToolInput>(invalid.clone()).is_err(),
                "{invalid}"
            );
        }
    }

    #[test]
    fn validation_and_result_formatting_preserve_source_contract() {
        assert_eq!(
            validate_agent_tool_input(&input(Some("agent-1"), Some("coder"))),
            Err(RESUME_WITH_TYPE_UNAVAILABLE)
        );
        assert!(validate_agent_tool_input(&input(Some("agent-1"), None)).is_ok());
        assert_eq!(
            format_subagent_stopped_message(Some(" Aborted by the user ")),
            USER_INTERRUPTED_SUBAGENT_MESSAGE
        );
        assert_eq!(
            format_subagent_stopped_message(Some("lost")),
            "The subagent was stopped before it finished. Reason: lost"
        );
        let background = format_background_agent_result("agent-task", &handle(), "work", true);
        assert!(background.contains("automatic_notification: true"));
        assert!(background.contains("do NOT wait, poll, or call TaskOutput"));
        assert!(background.contains("Agent(resume=\"agent-1\""));
        let timed_out =
            format_foreground_agent_failure(&handle(), "Agent timed out after 2 hours.", true);
        assert!(timed_out.contains("do not set subagent_type"));
        assert!(timed_out.contains("retains its prior context"));
    }

    #[test]
    fn launch_errors_preserve_user_and_non_user_abort_reasons() {
        let user_controller = AbortController::new();
        let user_reason = crate::_base::utils::abort::user_cancellation_reason();
        user_controller.abort(Some(user_reason.clone()));
        assert_eq!(
            launch_error_message(&user_reason, &user_controller.signal()),
            USER_INTERRUPTED_SUBAGENT_MESSAGE
        );

        let stopped_controller = AbortController::new();
        let stopped_reason = AbortError::new("Session closed");
        stopped_controller.abort(Some(stopped_reason.clone()));
        assert_eq!(
            launch_error_message(&stopped_reason, &stopped_controller.signal()),
            "The subagent was stopped before it finished. Reason: Session closed"
        );

        let ordinary = io::Error::other("launch failed");
        assert_eq!(
            launch_error_message(&ordinary, &AbortController::new().signal()),
            "launch failed"
        );
    }

    #[test]
    fn profile_descriptions_apply_profile_and_external_tool_restrictions() {
        let profiles = vec![
            profile(
                "restricted",
                Some("Restricted agent"),
                Some(&["Bash", "Read", "mcp__github__*"]),
                Some(&["Bash", "mcp__github__*"]),
            ),
            profile(
                "allow-all-except",
                Some("Allow all except one"),
                None,
                Some(&["Bash"]),
            ),
        ];
        let tools = vec![
            ToolReference {
                name: "Bash".into(),
                source: ToolSource::Builtin,
            },
            ToolReference {
                name: "Read".into(),
                source: ToolSource::Builtin,
            },
        ];
        let descriptions = build_profile_descriptions(&profiles, &tools, |policy, name, source| {
            is_tool_active(policy, name, source)
        });
        assert!(descriptions.contains("- restricted: Restricted agent\n  Tools: Read"));
        assert!(
            descriptions
                .contains("- allow-all-except: Allow all except one\n  Tools: all except Bash")
        );

        let externally_restricted =
            build_profile_descriptions(&profiles[..1], &tools, |policy, name, source| {
                is_tool_active(policy, name, source) && name != "Read"
            });
        assert!(externally_restricted.contains("Tools: none"));
    }

    #[test]
    fn profile_descriptions_show_pinned_model() {
        let pinned = Arc::new(AgentProfile {
            name: "pinned".into(),
            description: Some("Pinned model agent".into()),
            when_to_use: None,
            is_override: None,
            tools: None,
            disallowed_tools: None,
            subagents: None,
            model: Some("fast-model".into()),
            system_prompt: Arc::new(|_: &AgentProfileContext| String::new()),
            prompt_prefix: None,
            summary_policy: None,
        });
        let descriptions = build_profile_descriptions(&[pinned], &[], |_, _, _| true);
        assert!(
            descriptions.contains("- pinned: Pinned model agent\n  Tools: all\n  Model: fast-model"),
            "{descriptions}"
        );
    }
}
