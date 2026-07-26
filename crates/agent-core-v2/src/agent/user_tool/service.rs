//! Live host-defined user-tool registration and execution.
//!
//! Original: `agent/userTool/userToolService.ts`, `AgentUserToolService`.

use std::{collections::HashMap, ops::Deref, sync::Arc};

use async_trait::async_trait;
use futures_util::future::BoxFuture;
use serde_json::Value;
use tokio::sync::Mutex;

use crate::{
    _base::{
        di::{
            descriptors::SyncDescriptor,
            instantiation::{ServiceIdentifier, ServicesAccessorExt},
            lifecycle::{Disposable, DisposableHandle, DisposeResult},
            scope::{InstantiationType, LifecycleScope, register_scoped_service},
        },
        utils::abort::abortable,
    },
    agent::{
        profile::contract::AgentProfileServiceHandle,
        tool_registry::contract::{AgentToolRegistryServiceHandle, ToolRegistrationOptions},
    },
    kosong::contract::{message::ContentPart, tool::Tool},
    session::interaction::{
        InteractionKind, InteractionOrigin, InteractionRequest, SESSION_INTERACTION_SERVICE_ID,
        SessionInteractionService, SessionInteractionServiceHandle,
    },
    tool::{
        ExecutableTool, ExecutableToolContext, ExecutableToolOutput, ExecutableToolResult,
        RunnableToolExecution, ToolExecution, ToolSource,
    },
    wire::{
        contract::{WIRE_SERVICE_ID, WireServiceHandle},
        wire_service::WireService,
    },
};

use super::{USER_TOOL_MODEL, UserToolRegistration, register_user_tool, unregister_user_tool};

struct UserToolExecutable {
    definition: Tool,
    interaction: Arc<SessionInteractionService>,
}

#[async_trait]
impl ExecutableTool for UserToolExecutable {
    type Input = Value;

    fn tool(&self) -> &Tool {
        &self.definition
    }

    async fn resolve_execution(&self, args: Value) -> ToolExecution {
        let name = self.definition.name.clone();
        let interaction = Arc::clone(&self.interaction);
        let approval_rule = name.clone();
        let execute = Arc::new(move |context: ExecutableToolContext| {
            let name = name.clone();
            let interaction = Arc::clone(&interaction);
            let args = args.clone();
            Box::pin(async move {
                let request = InteractionRequest {
                    id: Some(context.tool_call_id.clone()),
                    kind: InteractionKind::UserTool,
                    payload: serde_json::json!({
                        "turnId": context.turn_id,
                        "toolCallId": context.tool_call_id,
                        "name": name,
                        "args": args,
                    }),
                    origin: Some(InteractionOrigin {
                        agent_id: None,
                        turn_id: Some(context.turn_id as f64),
                    }),
                };
                match abortable(interaction.request(request), &context.signal).await {
                    Ok(response) => executable_result_from_value(response),
                    Err(error) => {
                        interaction
                            .respond(
                                &context.tool_call_id,
                                serde_json::json!({
                                    "output": format!("User tool \"{}\" was aborted.", name),
                                    "isError": true,
                                }),
                            )
                            .await;
                        ExecutableToolResult::error(error.to_string())
                    }
                }
            }) as BoxFuture<'static, ExecutableToolResult>
        });
        ToolExecution::Runnable(RunnableToolExecution::new(approval_rule, execute))
    }
}

pub struct AgentUserToolService {
    registry: AgentToolRegistryServiceHandle,
    profile: AgentProfileServiceHandle,
    interaction: Arc<SessionInteractionService>,
    wire: Arc<WireService>,
    registrations: Mutex<HashMap<String, DisposableHandle>>,
    restore_subscription: DisposableHandle,
}

impl AgentUserToolService {
    // Original: AgentUserToolService.constructor().
    pub fn new(
        registry: AgentToolRegistryServiceHandle,
        profile: AgentProfileServiceHandle,
        interaction: Arc<SessionInteractionService>,
        wire: Arc<WireService>,
    ) -> Arc<Self> {
        Arc::new_cyclic(|weak: &std::sync::Weak<Self>| {
            let weak_for_hook = weak.clone();
            let subscription = wire
                .hooks()
                .on_did_restore
                .register(
                    "user-tool",
                    Arc::new(move |context, next| {
                        let weak = weak_for_hook.clone();
                        Box::pin(async move {
                            if let Some(service) = weak.upgrade() {
                                service.restore_registered_tools().await;
                            }
                            next(context).await
                        })
                    }),
                    Default::default(),
                )
                .expect("user-tool restore hook registration must be valid");
            Self {
                registry,
                profile,
                interaction,
                wire,
                registrations: Mutex::new(HashMap::new()),
                restore_subscription: subscription,
            }
        })
    }

    // Original: list().
    pub fn list(&self) -> Vec<UserToolRegistration> {
        self.wire
            .get_model(&USER_TOOL_MODEL)
            .into_values()
            .collect()
    }

    // Original: inheritUserTools().
    pub async fn inherit_user_tools(&self, parent: &Self) -> Result<(), String> {
        for registration in parent.list() {
            self.register(registration).await?;
        }
        Ok(())
    }

    // Original: register().
    pub async fn register(&self, input: UserToolRegistration) -> Result<(), String> {
        self.wire
            .dispatch([register_user_tool(input.clone()).map_err(|error| error.to_string())?])
            .map_err(|error| error.to_string())?;
        self.apply_register(input, true).await;
        Ok(())
    }

    // Original: unregister().
    pub async fn unregister(&self, name: &str) -> Result<(), String> {
        self.wire
            .dispatch([unregister_user_tool(name).map_err(|error| error.to_string())?])
            .map_err(|error| error.to_string())?;
        self.apply_unregister(name).await;
        Ok(())
    }

    async fn restore_registered_tools(&self) {
        let active = self.profile.get_active_tool_names();
        for registration in self.list() {
            let activate = active
                .as_ref()
                .is_none_or(|names| names.contains(&registration.name));
            self.apply_register(registration, activate).await;
        }
    }

    async fn apply_register(&self, input: UserToolRegistration, activate: bool) {
        self.apply_unregister(&input.name).await;
        let tool = UserToolExecutable {
            definition: Tool {
                name: input.name.clone(),
                description: input.description,
                parameters: input.parameters,
                deferred: None,
            },
            interaction: Arc::clone(&self.interaction),
        };
        let disposable = self.registry.register(
            Arc::new(tool),
            ToolRegistrationOptions {
                source: Some(ToolSource::User),
            },
        );
        self.registrations
            .lock()
            .await
            .insert(input.name.clone(), disposable);
        if activate {
            self.profile.add_active_tool(input.name);
        }
    }

    async fn apply_unregister(&self, name: &str) {
        if let Some(disposable) = self.registrations.lock().await.remove(name) {
            let _ = disposable.dispose();
        }
        self.profile.remove_active_tool(name);
    }
}

impl Drop for AgentUserToolService {
    fn drop(&mut self) {
        let _ = self.restore_subscription.dispose();
    }
}

#[derive(Clone)]
pub struct AgentUserToolServiceHandle(pub Arc<AgentUserToolService>);

impl Deref for AgentUserToolServiceHandle {
    type Target = AgentUserToolService;

    fn deref(&self) -> &Self::Target {
        self.0.as_ref()
    }
}

impl Disposable for AgentUserToolServiceHandle {
    fn dispose(&self) -> DisposeResult {
        self.0.restore_subscription.dispose()
    }
}

pub const AGENT_USER_TOOL_SERVICE_ID: ServiceIdentifier<AgentUserToolServiceHandle> =
    ServiceIdentifier::new("agentUserToolService");

pub fn register_agent_user_tool_service() {
    register_scoped_service(
        LifecycleScope::Agent,
        AGENT_USER_TOOL_SERVICE_ID,
        SyncDescriptor::new(|accessor| {
            let registry = (*accessor
                .get(crate::agent::tool_registry::contract::AGENT_TOOL_REGISTRY_SERVICE_ID)?)
            .clone();
            let profile =
                (*accessor.get(crate::agent::profile::contract::AGENT_PROFILE_SERVICE_ID)?).clone();
            let SessionInteractionServiceHandle(interaction) =
                (*accessor.get(SESSION_INTERACTION_SERVICE_ID)?).clone();
            let WireServiceHandle(wire) = (*accessor.get(WIRE_SERVICE_ID)?).clone();
            Ok(AgentUserToolServiceHandle(AgentUserToolService::new(
                registry,
                profile,
                interaction,
                wire,
            )))
        })
        .disposable(),
        InstantiationType::Eager,
        "agentUserTool",
    );
}

fn executable_result_from_value(response: Value) -> ExecutableToolResult {
    let is_error = response
        .get("isError")
        .or_else(|| response.get("is_error"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let output = match response.get("output").cloned() {
        Some(Value::String(text)) => ExecutableToolOutput::Text(text),
        Some(Value::Array(_)) => {
            serde_json::from_value::<Vec<ContentPart>>(response["output"].clone())
                .map(ExecutableToolOutput::Content)
                .unwrap_or_else(|_| ExecutableToolOutput::Text(response["output"].to_string()))
        }
        Some(value) => ExecutableToolOutput::Text(value.to_string()),
        None => ExecutableToolOutput::Text(String::new()),
    };
    ExecutableToolResult {
        output,
        is_error,
        stop_turn: response
            .get("stopTurn")
            .or_else(|| response.get("stop_turn"))
            .and_then(Value::as_bool),
        truncated: response.get("truncated").and_then(Value::as_bool),
        note: response
            .get("note")
            .and_then(Value::as_str)
            .map(str::to_owned),
        delivery: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_host_response_wire_fields_to_executable_results() {
        let result = executable_result_from_value(serde_json::json!({
            "output": "done",
            "isError": true,
            "stopTurn": true,
            "truncated": false,
            "note": "host note",
        }));
        assert_eq!(result.output, ExecutableToolOutput::Text("done".into()));
        assert!(result.is_error);
        assert_eq!(result.stop_turn, Some(true));
        assert_eq!(result.truncated, Some(false));
        assert_eq!(result.note.as_deref(), Some("host note"));
    }
}
