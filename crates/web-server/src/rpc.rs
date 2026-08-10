use std::sync::Arc;

use kimi_code_agent_core_v2::app::desktop_client::{
    DesktopAgentSettingsPatch, DesktopDeleteCustomAgentInput, DesktopPrepareSessionRequest,
    DesktopSaveCustomAgentInput, DesktopSaveProviderInput, KimiCodeDesktopClient,
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;

use crate::{
    AgentRpcRequest, DesktopStateChange, RpcError, app_events::ApplicationEventBus,
    dispatch_agent_rpc, load_plugin_marketplace, server::RpcConnection,
};

fn decode<T: DeserializeOwned>(value: Value) -> Result<T, RpcError> {
    serde_json::from_value(value).map_err(|error| RpcError::invalid_request(error.to_string()))
}

fn encode<T: Serialize>(value: T) -> Result<Value, RpcError> {
    serde_json::to_value(value).map_err(|error| RpcError::transport(error.to_string()))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SessionArgs {
    session_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SessionsArgs {
    session_ids: Vec<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GoalModeArgs {
    session_id: String,
    enabled: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WorkspaceArgs {
    workspace_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateWorkspaceArgs {
    root: String,
    #[serde(default)]
    name: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SkillArgs {
    session_id: String,
    name: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WorkspaceAgentArgs {
    workspace_id: String,
}

#[derive(Deserialize)]
struct SaveCustomAgentArgs {
    input: DesktopSaveCustomAgentInput,
}

#[derive(Deserialize)]
struct DeleteCustomAgentArgs {
    input: DesktopDeleteCustomAgentInput,
}

#[derive(Deserialize)]
struct ModelArgs {
    model: String,
}

#[derive(Deserialize)]
struct AgentSettingsArgs {
    patch: DesktopAgentSettingsPatch,
}

#[derive(Deserialize)]
struct PrepareSessionArgs {
    request: DesktopPrepareSessionRequest,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UploadFileArgs {
    filename: String,
    media_type: String,
    bytes: Vec<u8>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SubscribeArgs {
    session_id: String,
    agent_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UnsubscribeArgs {
    subscription_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct InteractionArgs {
    session_id: String,
    interaction_id: String,
    response: Value,
}

#[derive(Deserialize)]
struct BrowseFoldersArgs {
    #[serde(default)]
    path: Option<String>,
}

#[derive(Deserialize)]
struct PluginIdArgs {
    id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct InstallPluginArgs {
    source: String,
    operation_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PluginInstallOperationArgs {
    operation_id: String,
}

#[derive(Deserialize)]
struct SetPluginEnabledArgs {
    id: String,
    enabled: bool,
}

#[derive(Deserialize)]
struct SetPluginMcpServerEnabledArgs {
    id: String,
    server: String,
    enabled: bool,
}

#[derive(Deserialize)]
struct SaveProviderArgs {
    input: DesktopSaveProviderInput,
}

pub(crate) async fn dispatch_rpc(
    client: &Arc<KimiCodeDesktopClient>,
    events: &ApplicationEventBus,
    connection: &Arc<RpcConnection>,
    command: &str,
    args: Value,
) -> Result<Value, RpcError> {
    match command {
        "get_goal_mode" => {
            let args: SessionArgs = decode(args)?;
            encode(events.goal_mode(&args.session_id))
        }
        "set_goal_mode" => {
            let args: GoalModeArgs = decode(args)?;
            events.set_goal_mode(args.session_id, args.enabled);
            Ok(Value::Null)
        }
        "auth_status" => encode(client.auth_status().await.map_err(RpcError::transport)?),
        "account_usage" => encode(client.managed_usage().await.map_err(RpcError::transport)?),
        "account_profile" => encode(
            client
                .managed_user_info()
                .await
                .map_err(RpcError::transport)?,
        ),
        "login" => {
            let events = Arc::clone(connection);
            encode(
                client
                    .login(Arc::new(move |code| {
                        if let Ok(payload) = serde_json::to_value(code) {
                            events.emit("auth-device-code", payload);
                        }
                    }))
                    .await
                    .map_err(RpcError::transport)?,
            )
        }
        "logout" => encode(client.logout().await.map_err(RpcError::transport)?),
        "list_models" => encode(client.list_models().await.map_err(RpcError::transport)?),
        "refresh_models" => encode(client.refresh_models().await.map_err(RpcError::transport)?),
        "list_providers" => encode(client.list_providers().await.map_err(RpcError::transport)?),
        "save_provider" => {
            let args: SaveProviderArgs = decode(args)?;
            encode(
                client
                    .save_provider(args.input)
                    .await
                    .map_err(RpcError::transport)?,
            )
        }
        "delete_provider" => {
            let args: PluginIdArgs = decode(args)?;
            client
                .delete_provider(args.id)
                .await
                .map_err(RpcError::transport)?;
            Ok(Value::Null)
        }
        "list_plugins" => encode(client.list_plugins().await.map_err(RpcError::transport)?),
        "list_capabilities" => encode(
            client
                .list_capabilities()
                .await
                .map_err(RpcError::transport)?,
        ),
        "get_capability" => {
            let args: PluginIdArgs = decode(args)?;
            encode(
                client
                    .get_capability(args.id)
                    .await
                    .map_err(RpcError::transport)?,
            )
        }
        "install_capability" => {
            let args: PluginIdArgs = decode(args)?;
            encode(
                client
                    .install_capability(args.id)
                    .await
                    .map_err(RpcError::transport)?,
            )
        }
        "install_plugin" => {
            let args: InstallPluginArgs = decode(args)?;
            client
                .install_plugin_in_background(args.source, args.operation_id)
                .await
                .map_err(RpcError::transport)?;
            Ok(Value::Null)
        }
        "get_plugin_install_progress" => {
            let args: PluginInstallOperationArgs = decode(args)?;
            encode(
                client
                    .plugin_install_progress(args.operation_id)
                    .await
                    .map_err(RpcError::transport)?,
            )
        }
        "list_plugin_install_operations" => encode(
            client
                .list_plugin_install_operations()
                .await
                .map_err(RpcError::transport)?,
        ),
        "set_plugin_enabled" => {
            let args: SetPluginEnabledArgs = decode(args)?;
            client
                .set_plugin_enabled(args.id, args.enabled)
                .await
                .map_err(RpcError::transport)?;
            Ok(Value::Null)
        }
        "set_plugin_mcp_server_enabled" => {
            let args: SetPluginMcpServerEnabledArgs = decode(args)?;
            client
                .set_plugin_mcp_server_enabled(args.id, args.server, args.enabled)
                .await
                .map_err(RpcError::transport)?;
            Ok(Value::Null)
        }
        "remove_plugin" => {
            let args: PluginIdArgs = decode(args)?;
            client
                .remove_plugin(args.id)
                .await
                .map_err(RpcError::transport)?;
            Ok(Value::Null)
        }
        "reload_plugins" => encode(client.reload_plugins().await.map_err(RpcError::transport)?),
        "get_plugin_info" => {
            let args: PluginIdArgs = decode(args)?;
            encode(
                client
                    .get_plugin_info(args.id)
                    .await
                    .map_err(RpcError::transport)?,
            )
        }
        "check_plugin_updates" => encode(
            client
                .check_plugin_updates()
                .await
                .map_err(RpcError::transport)?,
        ),
        "get_plugin_marketplace" => encode(
            load_plugin_marketplace()
                .await
                .map_err(RpcError::transport)?,
        ),
        "list_skills" => {
            let args: SessionArgs = decode(args)?;
            encode(
                client
                    .list_session_skills(&args.session_id)
                    .await
                    .map_err(RpcError::transport)?,
            )
        }
        "get_skill_content" => {
            let args: SkillArgs = decode(args)?;
            encode(
                client
                    .get_session_skill_content(&args.session_id, &args.name)
                    .await
                    .map_err(RpcError::transport)?,
            )
        }
        "list_custom_agents" => {
            let args: WorkspaceAgentArgs = decode(args)?;
            encode(
                client
                    .list_custom_agents(&args.workspace_id)
                    .await
                    .map_err(RpcError::transport)?,
            )
        }
        "save_custom_agent" => {
            let args: SaveCustomAgentArgs = decode(args)?;
            encode(
                client
                    .save_custom_agent(args.input)
                    .await
                    .map_err(RpcError::transport)?,
            )
        }
        "delete_custom_agent" => {
            let args: DeleteCustomAgentArgs = decode(args)?;
            client
                .delete_custom_agent(args.input)
                .await
                .map_err(RpcError::transport)?;
            Ok(Value::Null)
        }
        "upload_file" => {
            let args: UploadFileArgs = decode(args)?;
            encode(
                client
                    .upload_file(&args.filename, &args.media_type, args.bytes)
                    .await
                    .map_err(RpcError::transport)?,
            )
        }
        "set_default_model" => {
            let args: ModelArgs = decode(args)?;
            client
                .set_default_model(&args.model)
                .await
                .map_err(RpcError::transport)?;
            Ok(Value::Null)
        }
        "get_agent_settings" => encode(
            client
                .get_agent_settings()
                .await
                .map_err(RpcError::transport)?,
        ),
        "update_agent_settings" => {
            let args: AgentSettingsArgs = decode(args)?;
            encode(
                client
                    .update_agent_settings(args.patch)
                    .await
                    .map_err(RpcError::transport)?,
            )
        }
        "list_workspaces" => encode(
            client
                .list_workspaces()
                .await
                .map_err(RpcError::transport)?,
        ),
        "create_or_touch_workspace" => {
            let args: CreateWorkspaceArgs = decode(args)?;
            let workspace = client
                .create_or_touch_workspace(&args.root, args.name.as_deref())
                .await
                .map_err(RpcError::transport)?;
            events.desktop_state_changed(DesktopStateChange::WorkspaceUpserted {
                workspace_id: workspace.id.clone(),
            });
            encode(workspace)
        }
        "remove_workspace" => {
            let args: WorkspaceArgs = decode(args)?;
            client
                .remove_workspace(&args.workspace_id)
                .await
                .map_err(RpcError::transport)?;
            events.desktop_state_changed(DesktopStateChange::WorkspaceRemoved {
                workspace_id: args.workspace_id,
            });
            Ok(Value::Null)
        }
        "list_workspace_sessions" => {
            let args: WorkspaceArgs = decode(args)?;
            encode(
                client
                    .list_workspace_sessions(&args.workspace_id)
                    .await
                    .map_err(RpcError::transport)?,
            )
        }
        "list_archived_sessions" => encode(
            client
                .list_archived_sessions()
                .await
                .map_err(RpcError::transport)?,
        ),
        "delete_archived_sessions" => {
            let args: SessionsArgs = decode(args)?;
            let session_ids = client
                .delete_archived_sessions(&args.session_ids)
                .await
                .map_err(RpcError::transport)?;
            if !session_ids.is_empty() {
                events.desktop_state_changed(DesktopStateChange::SessionsDeleted {
                    session_ids: session_ids.clone(),
                });
            }
            encode(session_ids)
        }
        "fork_session" => {
            let args: SessionArgs = decode(args)?;
            let session_id = client
                .fork_session(&args.session_id)
                .await
                .map_err(RpcError::transport)?;
            events.desktop_state_changed(DesktopStateChange::SessionForked {
                session_id: session_id.clone(),
            });
            encode(session_id)
        }
        "archive_session" => {
            let args: SessionArgs = decode(args)?;
            client
                .archive_session(&args.session_id)
                .await
                .map_err(RpcError::transport)?;
            events.desktop_state_changed(DesktopStateChange::SessionArchived {
                session_id: args.session_id,
            });
            Ok(Value::Null)
        }
        "restore_session" => {
            let args: SessionArgs = decode(args)?;
            let session = client
                .restore_session(&args.session_id)
                .await
                .map_err(RpcError::transport)?;
            events.desktop_state_changed(DesktopStateChange::SessionRestored {
                session_id: session.id.clone(),
            });
            encode(session)
        }
        "prepare_session" => {
            let args: PrepareSessionArgs = decode(args)?;
            let creating = args
                .request
                .session_id
                .as_deref()
                .is_none_or(|session_id| session_id.trim().is_empty());
            let workspace_root = args.request.work_dir.clone();
            let session = client
                .prepare_session(args.request)
                .await
                .map_err(RpcError::transport)?;
            if creating {
                events.desktop_state_changed(DesktopStateChange::SessionCreated {
                    session_id: session.session_id.clone(),
                    workspace_root,
                });
            }
            encode(session)
        }
        "conversation_context_usage" => {
            let args: SessionArgs = decode(args)?;
            encode(
                client
                    .context_usage(&args.session_id)
                    .await
                    .map_err(RpcError::transport)?,
            )
        }
        "list_conversation_messages" => {
            let args: SessionArgs = decode(args)?;
            encode(
                client
                    .list_messages(&args.session_id)
                    .await
                    .map_err(RpcError::transport)?,
            )
        }
        "agent_rpc" => {
            #[derive(Deserialize)]
            struct Args {
                request: AgentRpcRequest,
            }
            let args: Args = decode(args)?;
            dispatch_agent_rpc(client, args.request).await
        }
        "subscribe_agent_events" => {
            let args: SubscribeArgs = decode(args)?;
            let session_id = args.session_id.clone();
            let events = Arc::clone(connection);
            let interactions = Arc::clone(connection);
            let interaction_session_id = args.session_id.clone();
            let subscription = client
                .subscribe_agent_events_with_replay(
                    &args.session_id,
                    &args.agent_id,
                    Arc::new(move |agent_id, event| {
                        events.emit(
                            "agent-event",
                            serde_json::json!({
                                "sessionId": session_id,
                                "agentId": agent_id,
                                "event": event,
                            }),
                        );
                    }),
                    Arc::new(move |pending| {
                        interactions.emit(
                            "agent-interactions",
                            serde_json::json!({
                                "sessionId": interaction_session_id,
                                "interactions": pending,
                            }),
                        );
                    }),
                )
                .await
                .map_err(RpcError::transport)?;
            encode(connection.add_subscription(subscription)?)
        }
        "unsubscribe_agent_events" => {
            let args: UnsubscribeArgs = decode(args)?;
            connection.remove_subscription(&args.subscription_id)?;
            Ok(Value::Null)
        }
        "respond_interaction" => {
            let args: InteractionArgs = decode(args)?;
            client
                .respond_interaction(&args.session_id, &args.interaction_id, args.response)
                .await
                .map_err(RpcError::transport)?;
            Ok(Value::Null)
        }
        "fs_home" => encode(client.folder_home().await.map_err(RpcError::transport)?),
        "fs_browse" => {
            let args: BrowseFoldersArgs = decode(args)?;
            encode(
                client
                    .browse_folders(args.path.as_deref())
                    .await
                    .map_err(RpcError::transport)?,
            )
        }
        _ => Err(RpcError {
            code: "request.unknown_command".into(),
            message: format!("unknown RPC command `{command}`"),
            details: None,
        }),
    }
}
