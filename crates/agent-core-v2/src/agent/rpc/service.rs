//! Agent-scoped RPC implementation.
//!
//! Original: `packages/agent-core-v2/src/agent/rpc/rpcService.ts`.

use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::{
    _base::{
        di::{
            descriptors::SyncDescriptor,
            instantiation::ServicesAccessorExt,
            scope::{InstantiationType, LifecycleScope, register_scoped_service},
        },
        errors::errors::Error2,
    },
    agent::{
        context_memory::{
            AGENT_CONTEXT_MEMORY_SERVICE_ID, AgentContextData, AgentContextMemoryServiceHandle,
            ContextMessage, PluginCommandTrigger, PromptOrigin,
        },
        context_size::{AGENT_CONTEXT_SIZE_SERVICE_ID, AgentContextSizeServiceHandle},
        full_compaction::{
            AGENT_FULL_COMPACTION_SERVICE_ID, AgentFullCompactionServiceHandle, CompactionSource,
            FullCompactionInput,
        },
        goal::{
            AGENT_GOAL_SERVICE_ID, AgentGoalServiceHandle, CreateGoalInput, GoalSnapshot,
            GoalToolResult,
        },
        loop_::{AGENT_LOOP_SERVICE_ID, AgentLoopServiceHandle, AgentLoopState},
        permission_gate::{AGENT_PERMISSION_GATE_ID, AgentPermissionGateHandle, PermissionData},
        permission_mode::{AGENT_PERMISSION_MODE_SERVICE_ID, AgentPermissionModeServiceHandle},
        permission_policy::PermissionMode,
        plan::{AGENT_PLAN_SERVICE_ID, AgentPlanServiceHandle, PlanData},
        profile::{
            AGENT_PROFILE_SERVICE_ID, AgentProfileServiceHandle, ProfileData, ProfileUpdateData,
        },
        prompt::{
            AGENT_PROMPT_SERVICE_ID, AgentPromptServiceHandle, PromptHandle, PromptInput,
            PromptState, errors::REQUEST_INVALID,
        },
        scope_context::{AGENT_SCOPE_CONTEXT_ID, AgentScopeContext},
        shell_command::{
            AGENT_SHELL_COMMAND_SERVICE_ID, AgentShellCommandServiceHandle, RunShellCommandInput,
        },
        skill::{AGENT_SKILL_SERVICE_ID, AgentSkillServiceHandle, SkillActivationInput},
        swarm::{AGENT_SWARM_SERVICE_ID, AgentSwarmServiceHandle},
        task::{AGENT_TASK_SERVICE_ID, AgentTaskInfo, AgentTaskServiceHandle},
        tool_policy::{AGENT_TOOL_POLICY_SERVICE_ID, AgentToolPolicyServiceHandle},
        tool_registry::{AGENT_TOOL_REGISTRY_SERVICE_ID, AgentToolRegistryServiceHandle},
        usage::{AGENT_USAGE_SERVICE_ID, AgentUsageServiceHandle, UsageStatus},
        user_tool::{AGENT_USER_TOOL_SERVICE_ID, AgentUserToolServiceHandle, UserToolRegistration},
    },
    app::{
        event::{
            EVENT_SERVICE_ID, EventServiceHandle,
            event_bus::{
                DomainEventPayload, EVENT_BUS_SERVICE_ID, EventBusHandle, TypedEventBusExt,
            },
        },
        file::{FILE_SERVICE_ID, FileServiceHandle},
        plugin::{PLUGIN_SERVICE_ID, PluginServiceHandle, expand_command_arguments},
        telemetry::{TELEMETRY_SERVICE_ID, TelemetryProperties, TelemetryServiceHandle},
    },
    kosong::contract::message::{ContentPart, Message, Role},
    os::interface::host_environment::{HOST_ENVIRONMENT_SERVICE_ID, HostEnvironmentHandle},
    session::{
        agent_lifecycle::{AGENT_LIFECYCLE_SERVICE_ID, AgentLifecycleServiceHandle, MAIN_AGENT_ID},
        btw::{SESSION_BTW_SERVICE_ID, SessionBtwServiceHandle},
        session_context::{SESSION_CONTEXT_ID, SessionContext},
        session_metadata::{SESSION_METADATA_ID, SessionMetadataHandle},
        todo::{SESSION_TODO_SERVICE_ID, SessionTodoServiceHandle, TodoItem},
    },
};

use super::{
    AGENT_RPC_SERVICE_ID, ActivatePluginCommandPayload, ActivateSkillPayload, AgentRpcError,
    AgentRpcResult, AgentRpcServiceContract, AgentRpcServiceHandle, AgentRpcToolInfo,
    BeginCompactionPayload, CancelPayload, CancelPlanPayload, CancelShellCommandPayload,
    CreateGoalPayload, DetachTaskPayload, EmptyPayload, EnterSwarmPayload, GetTaskOutputPayload,
    GetTasksPayload, PromptMetadataUpdateTarget, PromptPayload, PromptSubmitResult,
    PromptSubmitStatus, RegisterToolPayload, RenameSessionPayload, RunShellCommandPayload,
    SetActiveToolsPayload, SetModelPayload, SetModelResult, SetPermissionPayload,
    SetThinkingPayload, ShellCommandResult, SteerPayload, StopTaskPayload, UndoHistoryPayload,
    UnregisterToolPayload, apply_prompt_metadata_update, prompt_metadata_text_from_content_parts,
    prompt_metadata_text_from_plugin_command, prompt_metadata_text_from_skill,
    resolve_prompt_attachments,
};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginCommandActivatedEvent {
    pub activation_id: String,
    pub plugin_id: String,
    pub command_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command_args: Option<String>,
    pub trigger: PluginCommandTrigger,
}

impl DomainEventPayload for PluginCommandActivatedEvent {
    const TYPE: &'static str = "plugin_command.activated";
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationUndoneEvent {
    pub count: f64,
    pub undone: u64,
}

impl DomainEventPayload for ConversationUndoneEvent {
    const TYPE: &'static str = "conversation.undone";
}

#[allow(clippy::too_many_arguments)]
pub struct AgentRpcService {
    prompt_service: AgentPromptServiceHandle,
    shell_command: AgentShellCommandServiceHandle,
    loop_service: AgentLoopServiceHandle,
    profile: AgentProfileServiceHandle,
    tool_policy: AgentToolPolicyServiceHandle,
    permission_mode: AgentPermissionModeServiceHandle,
    permission: AgentPermissionGateHandle,
    plan_mode: AgentPlanServiceHandle,
    swarm_mode: AgentSwarmServiceHandle,
    full_compaction: AgentFullCompactionServiceHandle,
    user_tools: AgentUserToolServiceHandle,
    tool_registry: AgentToolRegistryServiceHandle,
    _host_env: HostEnvironmentHandle,
    tasks: AgentTaskServiceHandle,
    context: AgentContextMemoryServiceHandle,
    context_size: AgentContextSizeServiceHandle,
    skills: AgentSkillServiceHandle,
    usage: AgentUsageServiceHandle,
    telemetry: TelemetryServiceHandle,
    goal: AgentGoalServiceHandle,
    event_bus: EventBusHandle,
    event_service: EventServiceHandle,
    plugins: PluginServiceHandle,
    metadata: SessionMetadataHandle,
    files: FileServiceHandle,
    session_context: SessionContext,
    btw: SessionBtwServiceHandle,
    scope_context: AgentScopeContext,
    agent_lifecycle: AgentLifecycleServiceHandle,
    todo: SessionTodoServiceHandle,
}

impl AgentRpcService {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        prompt_service: AgentPromptServiceHandle,
        shell_command: AgentShellCommandServiceHandle,
        loop_service: AgentLoopServiceHandle,
        profile: AgentProfileServiceHandle,
        tool_policy: AgentToolPolicyServiceHandle,
        permission_mode: AgentPermissionModeServiceHandle,
        permission: AgentPermissionGateHandle,
        plan_mode: AgentPlanServiceHandle,
        swarm_mode: AgentSwarmServiceHandle,
        full_compaction: AgentFullCompactionServiceHandle,
        user_tools: AgentUserToolServiceHandle,
        tool_registry: AgentToolRegistryServiceHandle,
        host_env: HostEnvironmentHandle,
        tasks: AgentTaskServiceHandle,
        context: AgentContextMemoryServiceHandle,
        context_size: AgentContextSizeServiceHandle,
        skills: AgentSkillServiceHandle,
        usage: AgentUsageServiceHandle,
        telemetry: TelemetryServiceHandle,
        goal: AgentGoalServiceHandle,
        event_bus: EventBusHandle,
        event_service: EventServiceHandle,
        plugins: PluginServiceHandle,
        metadata: SessionMetadataHandle,
        files: FileServiceHandle,
        session_context: SessionContext,
        btw: SessionBtwServiceHandle,
        scope_context: AgentScopeContext,
        agent_lifecycle: AgentLifecycleServiceHandle,
        todo: SessionTodoServiceHandle,
    ) -> Self {
        Self {
            prompt_service,
            shell_command,
            loop_service,
            profile,
            tool_policy,
            permission_mode,
            permission,
            plan_mode,
            swarm_mode,
            full_compaction,
            user_tools,
            tool_registry,
            _host_env: host_env,
            tasks,
            context,
            context_size,
            skills,
            usage,
            telemetry,
            goal,
            event_bus,
            event_service,
            plugins,
            metadata,
            files,
            session_context,
            btw,
            scope_context,
            agent_lifecycle,
            todo,
        }
    }

    async fn update_prompt_metadata(&self, text: Option<&str>) -> AgentRpcResult<()> {
        apply_prompt_metadata_update(
            PromptMetadataUpdateTarget {
                metadata: self.metadata.0.as_ref(),
                event_service: self.event_service.0.as_ref(),
                session_id: &self.session_context.session_id,
            },
            text,
        )
        .await?;
        Ok(())
    }

    fn track(&self, event: &str, properties: TelemetryProperties) {
        self.telemetry.track(event, Some(&properties));
    }
}

#[async_trait]
impl AgentRpcServiceContract for AgentRpcService {
    async fn prompt(&self, payload: PromptPayload) -> AgentRpcResult<PromptSubmitResult> {
        if let Some(disabled_tools) = payload.disabled_tools.clone()
            && let Err(error) = self
                .tool_policy
                .set_session_disabled_tools(disabled_tools)
                .await
        {
            if error
                .downcast_ref::<Error2>()
                .is_some_and(|error| error.name == "ProfileError")
            {
                return Err(Box::new(Error2::new(REQUEST_INVALID, error.to_string())));
            }
            return Err(error);
        }

        let resolved = resolve_prompt_attachments(
            payload.input,
            self.files.0.as_ref(),
            &self.session_context.session_dir,
        )
        .await?;
        let metadata_text = prompt_metadata_text_from_content_parts(&resolved.content);
        let prepared_skills = if payload.skills.is_empty() {
            None
        } else {
            Some(
                self.skills
                    .prepare_prompt_skills(
                        payload
                            .skills
                            .into_iter()
                            .map(|skill| SkillActivationInput {
                                name: skill.name,
                                args: skill.args,
                            })
                            .collect(),
                        metadata_text.clone(),
                    )
                    .await?,
            )
        };
        self.update_prompt_metadata(metadata_text.as_deref())
            .await?;
        let mut content = resolved.content;
        let origin = if let Some(prepared) = prepared_skills.as_ref() {
            content.insert(
                0,
                ContentPart::Text {
                    text: prepared.content.clone(),
                },
            );
            PromptOrigin::from_skill_activations(&prepared.origins).unwrap_or(PromptOrigin::User)
        } else {
            PromptOrigin::User
        };
        let handle = self
            .prompt_service
            .enqueue(PromptInput {
                id: None,
                message: user_message_with_attachments(content, resolved.attachments, Some(origin)),
            })
            .await?;
        if let Some(prepared) = prepared_skills {
            self.skills.record_user_activations(&prepared.origins);
        }
        let turn_id = if handle.snapshot().state == PromptState::Pending {
            None
        } else {
            handle.launched().await.map(|turn| turn.id())
        };
        Ok(prompt_submit_result(&handle, turn_id))
    }

    async fn run_shell_command(
        &self,
        payload: RunShellCommandPayload,
    ) -> AgentRpcResult<ShellCommandResult> {
        let result = self
            .shell_command
            .run(RunShellCommandInput {
                command: payload.command,
                command_id: payload.command_id,
            })
            .await?;
        Ok(ShellCommandResult {
            stdout: result.stdout,
            stderr: result.stderr,
            is_error: result.is_error,
            backgrounded: result.backgrounded,
        })
    }

    async fn cancel_shell_command(&self, payload: CancelShellCommandPayload) -> AgentRpcResult<()> {
        self.shell_command.cancel(&payload.command_id);
        Ok(())
    }

    async fn steer(&self, payload: SteerPayload) -> AgentRpcResult<PromptSubmitResult> {
        self.track(
            "input_steer",
            TelemetryProperties::from_iter([(
                "parts".into(),
                Some(Value::from(payload.input.len() as u64)),
            )]),
        );
        let resolved = resolve_prompt_attachments(
            payload.input,
            self.files.0.as_ref(),
            &self.session_context.session_dir,
        )
        .await?;
        let metadata_text = prompt_metadata_text_from_content_parts(&resolved.content);
        self.update_prompt_metadata(metadata_text.as_deref())
            .await?;
        let queued = self
            .prompt_service
            .enqueue(PromptInput {
                id: None,
                message: user_message_with_attachments(
                    resolved.content,
                    resolved.attachments,
                    Some(PromptOrigin::User),
                ),
            })
            .await?;
        if queued.snapshot().state == PromptState::Pending
            && let Err(error) = self.prompt_service.steer(&[queued.snapshot().id]).await
            && queued.snapshot().state == PromptState::Pending
        {
            return Err(error);
        }
        let turn_id = queued.launched().await.map(|turn| turn.id());
        Ok(prompt_submit_result(&queued, turn_id))
    }

    async fn cancel(&self, payload: CancelPayload) -> AgentRpcResult<()> {
        let status = self.loop_service.status();
        if status.state == AgentLoopState::Running {
            self.track(
                "cancel",
                TelemetryProperties::from_iter([
                    ("from".into(), Some(Value::String("streaming".into()))),
                    ("trace_id".into(), status.active_trace_id.map(Value::String)),
                ]),
            );
        }
        self.loop_service.cancel(payload.turn_id, None);
        Ok(())
    }

    async fn undo_history(&self, payload: UndoHistoryPayload) -> AgentRpcResult<u64> {
        let undone = self.prompt_service.undo(payload.count)?;
        self.event_bus.publish_typed(ConversationUndoneEvent {
            count: payload.count,
            undone: undone as u64,
        });
        self.track(
            "conversation_undo",
            TelemetryProperties::from_iter([("count".into(), Some(Value::from(payload.count)))]),
        );
        Ok(undone as u64)
    }

    async fn set_thinking(&self, payload: SetThinkingPayload) -> AgentRpcResult<()> {
        self.profile.set_thinking(payload.level)?;
        Ok(())
    }

    async fn set_permission(&self, payload: SetPermissionPayload) -> AgentRpcResult<()> {
        let was_yolo = self.permission_mode.mode() == PermissionMode::Yolo;
        let was_auto = self.permission_mode.mode() == PermissionMode::Auto;
        self.permission_mode.set_mode(payload.mode)?;
        if self.scope_context.agent_id == MAIN_AGENT_ID {
            self.agent_lifecycle
                .broadcast_permission_mode(payload.mode)?;
        }
        let enabled = self.permission_mode.mode() == PermissionMode::Yolo;
        if enabled != was_yolo {
            self.track(
                "yolo_toggle",
                TelemetryProperties::from_iter([("enabled".into(), Some(Value::Bool(enabled)))]),
            );
        }
        let auto_enabled = self.permission_mode.mode() == PermissionMode::Auto;
        if auto_enabled != was_auto {
            self.track(
                "afk_toggle",
                TelemetryProperties::from_iter([(
                    "enabled".into(),
                    Some(Value::Bool(auto_enabled)),
                )]),
            );
        }
        Ok(())
    }

    async fn set_model(&self, payload: SetModelPayload) -> AgentRpcResult<SetModelResult> {
        let result = self.profile.set_model(payload.model).await?;
        Ok(SetModelResult {
            model: result.model,
            provider_name: result.provider_name,
        })
    }

    async fn rename_session(&self, payload: RenameSessionPayload) -> AgentRpcResult<()> {
        self.metadata.set_title(payload.title).await?;
        Ok(())
    }

    async fn get_model(&self, _payload: EmptyPayload) -> AgentRpcResult<String> {
        Ok(self.profile.get_model()?)
    }

    async fn enter_plan(&self, _payload: EmptyPayload) -> AgentRpcResult<()> {
        self.plan_mode.enter(None, false).await?;
        Ok(())
    }

    async fn cancel_plan(&self, payload: CancelPlanPayload) -> AgentRpcResult<()> {
        self.plan_mode.cancel(payload.id)?;
        Ok(())
    }

    async fn clear_plan(&self, _payload: EmptyPayload) -> AgentRpcResult<()> {
        self.plan_mode.clear().await?;
        Ok(())
    }

    async fn enter_swarm(&self, payload: EnterSwarmPayload) -> AgentRpcResult<()> {
        self.swarm_mode.enter(payload.trigger)?;
        Ok(())
    }

    async fn exit_swarm(&self, _payload: EmptyPayload) -> AgentRpcResult<()> {
        self.swarm_mode.exit()?;
        Ok(())
    }

    async fn get_swarm_mode(&self, _payload: EmptyPayload) -> AgentRpcResult<bool> {
        Ok(self.swarm_mode.is_active())
    }

    async fn start_btw(&self, _payload: EmptyPayload) -> AgentRpcResult<String> {
        self.btw.start().await
    }

    async fn begin_compaction(&self, payload: BeginCompactionPayload) -> AgentRpcResult<()> {
        self.full_compaction.begin(FullCompactionInput {
            source: CompactionSource::Manual,
            instruction: payload.instruction,
        })?;
        Ok(())
    }

    async fn cancel_compaction(&self, _payload: EmptyPayload) -> AgentRpcResult<()> {
        if let Some(active) = self.full_compaction.compacting() {
            self.track(
                "cancel",
                TelemetryProperties::from_iter([
                    ("from".into(), Some(Value::String("compacting".into()))),
                    ("trace_id".into(), active.trace_id().map(Value::String)),
                ]),
            );
            active.abort_controller.abort(None);
        }
        Ok(())
    }

    async fn register_tool(&self, payload: RegisterToolPayload) -> AgentRpcResult<()> {
        self.user_tools
            .register(UserToolRegistration {
                name: payload.name,
                description: payload.description,
                parameters: payload.parameters,
            })
            .await
            .map_err(agent_rpc_message_error)
    }

    async fn unregister_tool(&self, payload: UnregisterToolPayload) -> AgentRpcResult<()> {
        self.user_tools
            .unregister(&payload.name)
            .await
            .map_err(agent_rpc_message_error)
    }

    async fn set_active_tools(&self, payload: SetActiveToolsPayload) -> AgentRpcResult<()> {
        self.profile.update(ProfileUpdateData {
            active_tool_names: Some(payload.names),
            ..ProfileUpdateData::default()
        })?;
        Ok(())
    }

    async fn stop_task(&self, payload: StopTaskPayload) -> AgentRpcResult<()> {
        let tasks = self.tasks.clone();
        tokio::spawn(async move {
            if let Some(reason) = payload.reason {
                let _ = tasks.stop(&payload.task_id, Some(&reason)).await;
            } else {
                let _ = tasks.stop_by_user(&payload.task_id).await;
            }
        });
        Ok(())
    }

    async fn detach_task(
        &self,
        payload: DetachTaskPayload,
    ) -> AgentRpcResult<Option<AgentTaskInfo>> {
        Ok(self.tasks.detach(&payload.task_id))
    }

    async fn clear_context(&self, _payload: EmptyPayload) -> AgentRpcResult<()> {
        self.prompt_service.clear()?;
        Ok(())
    }

    async fn activate_skill(&self, payload: ActivateSkillPayload) -> AgentRpcResult<()> {
        let activation = SkillActivationInput {
            name: payload.name.clone(),
            args: payload.args.clone(),
        };
        let metadata_text = prompt_metadata_text_from_skill(&payload);
        self.update_prompt_metadata(metadata_text.as_deref())
            .await?;
        self.skills.activate(activation).await?;
        Ok(())
    }

    async fn activate_plugin_command(
        &self,
        payload: ActivatePluginCommandPayload,
    ) -> AgentRpcResult<()> {
        let commands = self.plugins.list_plugin_commands().await?;
        let definition = commands
            .into_iter()
            .find(|command| {
                command.plugin_id == payload.plugin_id && command.name == payload.command_name
            })
            .ok_or_else(|| {
                Box::new(Error2::new(
                    REQUEST_INVALID,
                    format!(
                        "Plugin command \"{}:{}\" was not found",
                        payload.plugin_id, payload.command_name
                    ),
                )) as AgentRpcError
            })?;
        let command_args = payload.args.clone().unwrap_or_default();
        let expanded = expand_command_arguments(&definition.body, &command_args);
        let activation_id = Uuid::new_v4().to_string();
        let origin = PromptOrigin::PluginCommand {
            activation_id: activation_id.clone(),
            plugin_id: payload.plugin_id.clone(),
            command_name: payload.command_name.clone(),
            command_args: payload.args.clone(),
            trigger: PluginCommandTrigger::UserSlash,
        };
        self.event_bus.publish_typed(PluginCommandActivatedEvent {
            activation_id,
            plugin_id: payload.plugin_id.clone(),
            command_name: payload.command_name.clone(),
            command_args: payload.args.clone(),
            trigger: PluginCommandTrigger::UserSlash,
        });
        self.prompt_service
            .enqueue(PromptInput {
                id: None,
                message: user_message(vec![ContentPart::Text { text: expanded }], Some(origin)),
            })
            .await?;
        let metadata_text = prompt_metadata_text_from_plugin_command(&payload);
        self.update_prompt_metadata(metadata_text.as_deref()).await
    }

    async fn create_goal(&self, payload: CreateGoalPayload) -> AgentRpcResult<GoalSnapshot> {
        Ok(self
            .goal
            .create_goal(
                CreateGoalInput {
                    objective: payload.objective,
                    completion_criterion: payload.completion_criterion,
                    replace: payload.replace,
                },
                None,
            )
            .await?)
    }

    async fn get_goal(&self, _payload: EmptyPayload) -> AgentRpcResult<GoalToolResult> {
        Ok(self.goal.get_goal()?)
    }

    async fn pause_goal(&self, _payload: EmptyPayload) -> AgentRpcResult<GoalSnapshot> {
        Ok(self.goal.pause_goal(None, None).await?)
    }

    async fn resume_goal(&self, _payload: EmptyPayload) -> AgentRpcResult<GoalSnapshot> {
        Ok(self.goal.resume_goal(None, None).await?)
    }

    async fn cancel_goal(&self, _payload: EmptyPayload) -> AgentRpcResult<GoalSnapshot> {
        Ok(self.goal.cancel_goal(None, None).await?)
    }

    async fn get_task_output(&self, payload: GetTaskOutputPayload) -> AgentRpcResult<String> {
        Ok(self
            .tasks
            .read_output(&payload.task_id, payload.tail)
            .await?)
    }

    async fn get_context(&self, _payload: EmptyPayload) -> AgentRpcResult<AgentContextData> {
        Ok(AgentContextData {
            history: self.context.get().to_vec(),
            token_count: self.context_size.get(None, None).measured,
        })
    }

    async fn get_config(&self, _payload: EmptyPayload) -> AgentRpcResult<ProfileData> {
        Ok(self.profile.data()?)
    }

    async fn get_permission(&self, _payload: EmptyPayload) -> AgentRpcResult<PermissionData> {
        Ok(self.permission.data())
    }

    async fn get_plan(&self, _payload: EmptyPayload) -> AgentRpcResult<Option<PlanData>> {
        Ok(self.plan_mode.status().await?)
    }

    async fn get_todos(&self, _payload: EmptyPayload) -> AgentRpcResult<Vec<TodoItem>> {
        Ok(self.todo.get_todos())
    }

    async fn get_usage(&self, _payload: EmptyPayload) -> AgentRpcResult<UsageStatus> {
        Ok(self.usage.status())
    }

    async fn get_tools(&self, _payload: EmptyPayload) -> AgentRpcResult<Vec<AgentRpcToolInfo>> {
        self.tool_registry
            .list()
            .into_iter()
            .map(|tool| {
                Ok(AgentRpcToolInfo {
                    active: self.tool_policy.is_tool_active(&tool.name, tool.source)?,
                    name: tool.name,
                    description: tool.description,
                    source: tool.source,
                })
            })
            .collect()
    }

    async fn get_tasks(&self, payload: GetTasksPayload) -> AgentRpcResult<Vec<AgentTaskInfo>> {
        Ok(self.tasks.list(
            Some(payload.active_only.unwrap_or(false)),
            rpc_task_limit(payload.limit),
        ))
    }
}

fn user_message(content: Vec<ContentPart>, origin: Option<PromptOrigin>) -> ContextMessage {
    user_message_with_attachments(content, Vec::new(), origin)
}

fn user_message_with_attachments(
    content: Vec<ContentPart>,
    attachments: Vec<crate::agent::context_memory::ContextFileAttachment>,
    origin: Option<PromptOrigin>,
) -> ContextMessage {
    ContextMessage {
        message: Message::new(Role::User, content, Vec::new()),
        id: None,
        provider_message_id: None,
        origin,
        is_error: None,
        note: None,
        attachments,
    }
}

fn prompt_submit_result(handle: &PromptHandle, turn_id: Option<i64>) -> PromptSubmitResult {
    let snapshot = handle.snapshot();
    PromptSubmitResult {
        prompt_id: snapshot.id,
        turn_id,
        status: match snapshot.state {
            PromptState::Pending => PromptSubmitStatus::Queued,
            PromptState::Running => PromptSubmitStatus::Running,
            PromptState::Steered => PromptSubmitStatus::Steered,
            PromptState::Completed => PromptSubmitStatus::Completed,
            PromptState::Failed => PromptSubmitStatus::Failed,
            PromptState::Cancelled => PromptSubmitStatus::Cancelled,
            PromptState::Blocked => PromptSubmitStatus::Blocked,
        },
    }
}

fn agent_rpc_message_error(message: String) -> AgentRpcError {
    Box::new(std::io::Error::other(message))
}

fn rpc_task_limit(limit: Option<f64>) -> Option<usize> {
    let limit = limit?;
    if !limit.is_finite() {
        return None;
    }
    Some(limit.ceil().max(0.0).min(usize::MAX as f64) as usize)
}

// Original: registerScopedService(Agent, ..., Eager, "rpc").
pub fn register_agent_rpc_service() {
    register_scoped_service(
        LifecycleScope::Agent,
        AGENT_RPC_SERVICE_ID,
        SyncDescriptor::new(|accessor| {
            let service = AgentRpcService::new(
                (*accessor.get(AGENT_PROMPT_SERVICE_ID)?).clone(),
                (*accessor.get(AGENT_SHELL_COMMAND_SERVICE_ID)?).clone(),
                (*accessor.get(AGENT_LOOP_SERVICE_ID)?).clone(),
                (*accessor.get(AGENT_PROFILE_SERVICE_ID)?).clone(),
                (*accessor.get(AGENT_TOOL_POLICY_SERVICE_ID)?).clone(),
                (*accessor.get(AGENT_PERMISSION_MODE_SERVICE_ID)?).clone(),
                (*accessor.get(AGENT_PERMISSION_GATE_ID)?).clone(),
                (*accessor.get(AGENT_PLAN_SERVICE_ID)?).clone(),
                (*accessor.get(AGENT_SWARM_SERVICE_ID)?).clone(),
                (*accessor.get(AGENT_FULL_COMPACTION_SERVICE_ID)?).clone(),
                (*accessor.get(AGENT_USER_TOOL_SERVICE_ID)?).clone(),
                (*accessor.get(AGENT_TOOL_REGISTRY_SERVICE_ID)?).clone(),
                (*accessor.get(HOST_ENVIRONMENT_SERVICE_ID)?).clone(),
                (*accessor.get(AGENT_TASK_SERVICE_ID)?).clone(),
                (*accessor.get(AGENT_CONTEXT_MEMORY_SERVICE_ID)?).clone(),
                (*accessor.get(AGENT_CONTEXT_SIZE_SERVICE_ID)?).clone(),
                (*accessor.get(AGENT_SKILL_SERVICE_ID)?).clone(),
                (*accessor.get(AGENT_USAGE_SERVICE_ID)?).clone(),
                (*accessor.get(TELEMETRY_SERVICE_ID)?).clone(),
                (*accessor.get(AGENT_GOAL_SERVICE_ID)?).clone(),
                (*accessor.get(EVENT_BUS_SERVICE_ID)?).clone(),
                (*accessor.get(EVENT_SERVICE_ID)?).clone(),
                (*accessor.get(PLUGIN_SERVICE_ID)?).clone(),
                (*accessor.get(SESSION_METADATA_ID)?).clone(),
                (*accessor.get(FILE_SERVICE_ID)?).clone(),
                (*accessor.get(SESSION_CONTEXT_ID)?).clone(),
                (*accessor.get(SESSION_BTW_SERVICE_ID)?).clone(),
                (*accessor.get(AGENT_SCOPE_CONTEXT_ID)?).clone(),
                (*accessor.get(AGENT_LIFECYCLE_SERVICE_ID)?).clone(),
                (*accessor.get(SESSION_TODO_SERVICE_ID)?).clone(),
            );
            let service: Arc<dyn AgentRpcServiceContract> = Arc::new(service);
            Ok(AgentRpcServiceHandle(service))
        }),
        InstantiationType::Eager,
        "rpc",
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::_base::di::scope::get_scoped_service_descriptors;

    #[test]
    fn plugin_activation_event_matches_source_wire_shape() {
        assert_eq!(
            serde_json::to_value(PluginCommandActivatedEvent {
                activation_id: "activation-1".into(),
                plugin_id: "demo".into(),
                command_name: "review".into(),
                command_args: None,
                trigger: PluginCommandTrigger::UserSlash,
            })
            .unwrap(),
            serde_json::json!({
                "activationId": "activation-1",
                "pluginId": "demo",
                "commandName": "review",
                "trigger": "user-slash"
            })
        );
        assert_eq!(
            PluginCommandActivatedEvent::TYPE,
            "plugin_command.activated"
        );
    }

    #[test]
    fn conversation_undone_event_is_broadcastable_to_all_clients() {
        assert_eq!(
            serde_json::to_value(ConversationUndoneEvent {
                count: 1.0,
                undone: 1,
            })
            .unwrap(),
            serde_json::json!({ "count": 1.0, "undone": 1 })
        );
        assert_eq!(ConversationUndoneEvent::TYPE, "conversation.undone");
    }

    #[test]
    fn registration_is_eager_agent_scoped_with_source_domain() {
        register_agent_rpc_service();
        let descriptors = get_scoped_service_descriptors(LifecycleScope::Agent);
        let descriptor = descriptors
            .iter()
            .find(|entry| entry.id.to_string() == AGENT_RPC_SERVICE_ID.to_string())
            .expect("agent RPC service is registered");
        assert!(!descriptor.descriptor.supports_delayed_instantiation);
        assert_eq!(descriptor.domain, "rpc");
    }

    #[test]
    fn task_limit_conversion_matches_javascript_length_comparison() {
        assert_eq!(rpc_task_limit(None), None);
        assert_eq!(rpc_task_limit(Some(1.2)), Some(2));
        assert_eq!(rpc_task_limit(Some(-1.0)), Some(0));
        assert_eq!(rpc_task_limit(Some(f64::INFINITY)), None);
        assert_eq!(rpc_task_limit(Some(f64::NAN)), None);
    }
}
