//! Agent-scoped shell command implementation.
//!
//! Original:
//! `packages/agent-core-v2/src/agent/shellCommand/shellCommandService.ts`.

use parking_lot::Mutex;
use std::sync::Arc;
use std::{collections::HashMap, io};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::{
    _base::{
        di::{
            descriptors::SyncDescriptor,
            instantiation::ServicesAccessorExt,
            scope::{InstantiationType, LifecycleScope, register_scoped_service},
        },
        utils::{
            abort::{AbortController, user_cancellation_reason},
            xml_escape::escape_xml_text,
        },
    },
    agent::{
        context_memory::{
            AGENT_CONTEXT_MEMORY_SERVICE_ID, AgentContextMemoryServiceHandle, ContextMessage,
            PromptOrigin, ShellCommandPhase,
        },
        prompt::{AGENT_PROMPT_SERVICE_ID, AgentPromptServiceHandle},
        tool_registry::{AGENT_TOOL_REGISTRY_SERVICE_ID, AgentToolRegistryServiceHandle},
    },
    app::event::event_bus::{
        DomainEventPayload, EVENT_BUS_SERVICE_ID, EventBusHandle, TypedEventBusExt,
    },
    kosong::contract::message::{ContentPart, Message, Role},
    tool::{
        ExecutableToolContext, ExecutableToolOutput, ToolExecution, ToolUpdate, ToolUpdateKind,
    },
};

use super::{
    AGENT_SHELL_COMMAND_SERVICE_ID, AgentShellCommandServiceContract,
    AgentShellCommandServiceHandle, RunShellCommandInput, RunShellCommandResult,
    ShellCommandServiceError,
};

const SHELL_FOREGROUND_TIMEOUT_S: u64 = 2 * 60;

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShellOutputEvent {
    pub command_id: String,
    pub update: ToolUpdate,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
}

impl DomainEventPayload for ShellOutputEvent {
    const TYPE: &'static str = "shell.output";
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShellStartedEvent {
    pub command_id: String,
    pub task_id: String,
}

impl DomainEventPayload for ShellStartedEvent {
    const TYPE: &'static str = "shell.started";
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShellCompletedEvent {
    pub command_id: String,
    pub is_error: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
}

impl DomainEventPayload for ShellCompletedEvent {
    const TYPE: &'static str = "shell.completed";
}

pub struct AgentShellCommandService {
    tool_registry: AgentToolRegistryServiceHandle,
    context: AgentContextMemoryServiceHandle,
    prompt_service: AgentPromptServiceHandle,
    event_bus: EventBusHandle,
    controllers: Mutex<HashMap<String, AbortController>>,
    tasks: Arc<Mutex<HashMap<String, String>>>,
}

impl AgentShellCommandService {
    pub fn new(
        tool_registry: AgentToolRegistryServiceHandle,
        context: AgentContextMemoryServiceHandle,
        prompt_service: AgentPromptServiceHandle,
        event_bus: EventBusHandle,
    ) -> Self {
        Self {
            tool_registry,
            context,
            prompt_service,
            event_bus,
            controllers: Mutex::new(HashMap::new()),
            tasks: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    fn append_shell_input(&self, command: &str) -> Result<(), ShellCommandServiceError> {
        let text = format!("<bash-input>\n{}\n</bash-input>", escape_xml_text(command));
        self.context
            .append(vec![context_message(
                text,
                PromptOrigin::ShellCommand {
                    phase: ShellCommandPhase::Input,
                    is_error: None,
                },
            )])
            .map_err(|error| Box::new(error) as ShellCommandServiceError)
    }

    fn append_shell_output(
        &self,
        stdout: &str,
        stderr: &str,
        is_error: Option<bool>,
    ) -> Result<(), ShellCommandServiceError> {
        let text = format!(
            "<bash-stdout>{}</bash-stdout><bash-stderr>{}</bash-stderr>",
            escape_xml_text(stdout),
            escape_xml_text(stderr)
        );
        self.context
            .append(vec![context_message(
                text,
                PromptOrigin::ShellCommand {
                    phase: ShellCommandPhase::Output,
                    is_error: (is_error == Some(true)).then_some(true),
                },
            )])
            .map_err(|error| Box::new(error) as ShellCommandServiceError)
    }

    fn notify_backgrounded(&self, output: String) {
        let prompt = self.prompt_service.clone();
        tokio::spawn(async move {
            let _ = prompt
                .inject(context_message(
                    output,
                    PromptOrigin::Injection {
                        variant: "shell_command_backgrounded".into(),
                    },
                ))
                .await;
        });
    }

    fn task_id(&self, command_id: &str) -> Option<String> {
        self.tasks.lock().get(command_id).cloned()
    }

    fn publish_fallback_error(&self, command_id: &str, text: String) {
        if text.is_empty() {
            return;
        }
        self.event_bus.publish_typed(ShellOutputEvent {
            task_id: self.task_id(command_id),
            command_id: command_id.into(),
            update: ToolUpdate {
                kind: ToolUpdateKind::Stderr,
                text: Some(text),
                percent: None,
                custom_kind: None,
                custom_data: None,
            },
        });
    }

    fn publish_completed(&self, command_id: &str, is_error: bool) {
        self.event_bus.publish_typed(ShellCompletedEvent {
            task_id: self.task_id(command_id),
            command_id: command_id.into(),
            is_error,
        });
    }
}

#[async_trait]
impl AgentShellCommandServiceContract for AgentShellCommandService {
    async fn run(
        &self,
        input: RunShellCommandInput,
    ) -> Result<RunShellCommandResult, ShellCommandServiceError> {
        self.append_shell_input(&input.command)?;

        let controller = AbortController::new();
        if let Some(command_id) = &input.command_id {
            self.controllers
                .lock()
                .insert(command_id.clone(), controller.clone());
        }

        let result = self.run_inner(&input, controller).await;
        if let Some(command_id) = &input.command_id {
            self.controllers.lock().remove(command_id);
            self.tasks.lock().remove(command_id);
        }
        Ok(result)
    }

    fn cancel(&self, command_id: &str) {
        if let Some(controller) = self.controllers.lock().get(command_id) {
            controller.abort(Some(user_cancellation_reason()));
        }
    }
}

impl AgentShellCommandService {
    async fn run_inner(
        &self,
        input: &RunShellCommandInput,
        controller: AbortController,
    ) -> RunShellCommandResult {
        let mut stdout = String::new();
        let mut stderr = String::new();

        let execution_result = async {
            let bash = self.tool_registry.resolve("Bash").ok_or_else(|| {
                io::Error::new(io::ErrorKind::NotFound, "Bash tool is not registered.")
            })?;
            bash.resolve_execution_value(serde_json::json!({
                "command": input.command,
                "timeout": SHELL_FOREGROUND_TIMEOUT_S,
            }))
            .await
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))
        }
        .await;

        let execution = match execution_result {
            Ok(execution) => execution,
            Err(error) => {
                stderr.push_str(&error.to_string());
                if let Some(command_id) = &input.command_id {
                    self.publish_fallback_error(command_id, error.to_string());
                    self.publish_completed(command_id, true);
                }
                let _ = self.append_shell_output(&stdout, &stderr, Some(true));
                return shell_result(stdout, stderr, true, false);
            }
        };

        let runnable = match execution {
            ToolExecution::Error(result) => {
                let output = text_output(&result.output)
                    .unwrap_or("Command failed.")
                    .to_owned();
                let _ = self.append_shell_output("", &output, Some(true));
                return shell_result(String::new(), output, true, false);
            }
            ToolExecution::Runnable(runnable) => runnable,
        };

        let stdout_buffer = Arc::new(Mutex::new(String::new()));
        let stderr_buffer = Arc::new(Mutex::new(String::new()));
        let stdout_for_update = Arc::clone(&stdout_buffer);
        let stderr_for_update = Arc::clone(&stderr_buffer);
        let event_bus = self.event_bus.clone();
        let tasks = Arc::clone(&self.tasks);
        let command_id = input.command_id.clone();
        let on_update = Arc::new(move |update: ToolUpdate| {
            match update.kind {
                ToolUpdateKind::Stdout => stdout_for_update
                    .lock()
                    .push_str(update.text.as_deref().unwrap_or_default()),
                ToolUpdateKind::Stderr => stderr_for_update
                    .lock()
                    .push_str(update.text.as_deref().unwrap_or_default()),
                _ => return,
            }
            if let Some(command_id) = &command_id {
                event_bus.publish_typed(ShellOutputEvent {
                    command_id: command_id.clone(),
                    update,
                    task_id: tasks.lock().get(command_id).cloned(),
                });
            }
        });

        let event_bus = self.event_bus.clone();
        let tasks = Arc::clone(&self.tasks);
        let command_id = input.command_id.clone();
        let on_foreground_task_start = Arc::new(move |task_id: String| {
            if let Some(command_id) = &command_id {
                tasks.lock().insert(command_id.clone(), task_id.clone());
                event_bus.publish_typed(ShellStartedEvent {
                    command_id: command_id.clone(),
                    task_id,
                });
            }
        });

        let result = runnable
            .execute(ExecutableToolContext {
                // The source uses -1 as a sentinel. Rust's execution contract
                // uses u64, so MAX is the lossless sentinel adaptation.
                turn_id: crate::agent::TurnId::MAX,
                tool_call_id: "shell-command".into(),
                trace: None,
                metadata: None,
                signal: controller.signal(),
                on_update: Some(on_update),
                on_foreground_task_start: Some(on_foreground_task_start),
            })
            .await;

        stdout = stdout_buffer.lock().clone();
        stderr = stderr_buffer.lock().clone();
        if let Some(output) = text_output(&result.output)
            && output.starts_with("task_id: ")
        {
            self.notify_backgrounded(output.to_owned());
            return RunShellCommandResult {
                stdout: output.into(),
                stderr: String::new(),
                is_error: Some(false),
                backgrounded: Some(true),
            };
        }

        if result.is_error && stdout.is_empty() && stderr.is_empty() {
            stderr = text_output(&result.output)
                .unwrap_or("Command failed.")
                .to_owned();
            if let Some(command_id) = &input.command_id {
                self.publish_fallback_error(command_id, stderr.clone());
            }
        }
        if let Some(command_id) = &input.command_id {
            self.publish_completed(command_id, result.is_error);
        }
        let _ = self.append_shell_output(&stdout, &stderr, Some(result.is_error));
        shell_result(stdout, stderr, result.is_error, false)
    }
}

fn context_message(text: String, origin: PromptOrigin) -> ContextMessage {
    ContextMessage {
        message: Message::new(Role::User, vec![ContentPart::Text { text }], Vec::new()),
        id: None,
        provider_message_id: None,
        origin: Some(origin),
        is_error: None,
        note: None,
        attachments: Vec::new(),
    }
}

fn text_output(output: &ExecutableToolOutput) -> Option<&str> {
    match output {
        ExecutableToolOutput::Text(text) => Some(text),
        ExecutableToolOutput::Content(_) => None,
    }
}

fn shell_result(
    stdout: String,
    stderr: String,
    is_error: bool,
    backgrounded: bool,
) -> RunShellCommandResult {
    RunShellCommandResult {
        stdout,
        stderr,
        is_error: Some(is_error),
        backgrounded: backgrounded.then_some(true),
    }
}

// Original: registerScopedService(Agent, ..., Eager, "shellCommand").
pub fn register_agent_shell_command_service() {
    register_scoped_service(
        LifecycleScope::Agent,
        AGENT_SHELL_COMMAND_SERVICE_ID,
        SyncDescriptor::new(|accessor| {
            let service = AgentShellCommandService::new(
                (*accessor.get(AGENT_TOOL_REGISTRY_SERVICE_ID)?).clone(),
                (*accessor.get(AGENT_CONTEXT_MEMORY_SERVICE_ID)?).clone(),
                (*accessor.get(AGENT_PROMPT_SERVICE_ID)?).clone(),
                (*accessor.get(EVENT_BUS_SERVICE_ID)?).clone(),
            );
            let service: Arc<dyn AgentShellCommandServiceContract> = Arc::new(service);
            Ok(AgentShellCommandServiceHandle(service))
        }),
        InstantiationType::Eager,
        "shellCommand",
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::_base::di::scope::get_scoped_service_descriptors;

    #[test]
    fn typed_events_match_source_wire_shapes() {
        assert_eq!(
            serde_json::to_value(ShellCompletedEvent {
                command_id: "command-1".into(),
                is_error: false,
                task_id: Some("task-1".into()),
            })
            .unwrap(),
            serde_json::json!({
                "commandId": "command-1",
                "isError": false,
                "taskId": "task-1"
            })
        );
        assert_eq!(ShellCompletedEvent::TYPE, "shell.completed");
    }

    #[test]
    fn registration_is_eager_agent_scoped_with_source_domain() {
        register_agent_shell_command_service();
        let descriptors = get_scoped_service_descriptors(LifecycleScope::Agent);
        let descriptor = descriptors
            .iter()
            .find(|entry| entry.id.to_string() == AGENT_SHELL_COMMAND_SERVICE_ID.to_string())
            .expect("shell command service is registered");
        assert!(!descriptor.descriptor.supports_delayed_instantiation);
        assert_eq!(descriptor.domain, "shellCommand");
    }
}
