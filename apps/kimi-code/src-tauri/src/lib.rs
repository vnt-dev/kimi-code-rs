use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

mod plugin_marketplace;

use kimi_code_agent_core_v2::{
    _base::di::lifecycle::DisposableHandle,
    app::{
        bootstrap::resolve_kimi_home,
        desktop_client::{
            DesktopAuthStatus, DesktopContextUsage, DesktopDeviceCode, DesktopInteraction,
            DesktopManagedUsage, DesktopMessagePage, DesktopModel, DesktopPrepareSessionRequest,
            DesktopPreparedSession, DesktopSkill, DesktopSkillContent, DesktopWorkspace,
            KimiCodeDesktopClient,
        },
        file::FileMeta,
        plugin::{
            PluginInfo, PluginInstallProgress, PluginInstallProgressCallback, PluginSummary,
            PluginUpdateStatus, ReloadSummary,
        },
        session_index::SessionSummary,
    },
};
use kimi_code_web_server::{
    AgentRpcRequest, AssetProvider, DesktopStateChange, RpcError, WebAsset, WebServerController,
    WebServerSettings, WebServerStatus, dispatch_agent_rpc,
};
use serde::Serialize;
use serde_json::Value;
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_opener::OpenerExt;

use crate::plugin_marketplace::{PluginMarketplace, load_plugin_marketplace};

struct AppState {
    client: Arc<KimiCodeDesktopClient>,
    web_server: Arc<WebServerController>,
    subscriptions: Mutex<HashMap<String, DisposableHandle>>,
    next_subscription_id: AtomicU64,
    _application_event_subscription: DisposableHandle,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct DeviceCodeEvent {
    user_code: String,
    verification_uri: String,
    verification_uri_complete: String,
    expires_in: Option<f64>,
}

impl From<DesktopDeviceCode> for DeviceCodeEvent {
    fn from(value: DesktopDeviceCode) -> Self {
        Self {
            user_code: value.user_code,
            verification_uri: value.verification_uri,
            verification_uri_complete: value.verification_uri_complete,
            expires_in: value.expires_in,
        }
    }
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct AgentEvent {
    session_id: String,
    agent_id: String,
    event: Value,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct AgentInteractionsEvent {
    session_id: String,
    interactions: Vec<DesktopInteraction>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct PluginInstallProgressEvent {
    operation_id: String,
    #[serde(flatten)]
    progress: PluginInstallProgress,
}

#[tauri::command]
async fn auth_status(state: State<'_, AppState>) -> Result<DesktopAuthStatus, String> {
    state.client.auth_status().await
}

#[tauri::command]
async fn account_usage(state: State<'_, AppState>) -> Result<DesktopManagedUsage, String> {
    state.client.managed_usage().await
}

#[tauri::command]
async fn login(app: AppHandle, state: State<'_, AppState>) -> Result<DesktopAuthStatus, String> {
    let app_for_event = app.clone();
    state
        .client
        .login(Arc::new(move |code| {
            let authorization_url = if code.verification_uri_complete.trim().is_empty() {
                code.verification_uri.clone()
            } else {
                code.verification_uri_complete.clone()
            };
            let _ = app_for_event.emit("auth-device-code", DeviceCodeEvent::from(code));
            if let Err(error) = app_for_event
                .opener()
                .open_url(authorization_url, None::<String>)
            {
                let _ = app_for_event.emit("auth-browser-open-failed", error.to_string());
            }
        }))
        .await
}

#[tauri::command]
async fn logout(state: State<'_, AppState>) -> Result<DesktopAuthStatus, String> {
    state.client.logout().await
}

#[tauri::command]
async fn list_models(state: State<'_, AppState>) -> Result<Vec<DesktopModel>, String> {
    state.client.list_models().await
}

#[tauri::command]
async fn refresh_models(state: State<'_, AppState>) -> Result<Vec<DesktopModel>, String> {
    state.client.refresh_models().await
}

#[tauri::command]
async fn list_skills(
    state: State<'_, AppState>,
    session_id: String,
) -> Result<Vec<DesktopSkill>, String> {
    state.client.list_session_skills(&session_id).await
}

#[tauri::command]
async fn get_skill_content(
    state: State<'_, AppState>,
    session_id: String,
    name: String,
) -> Result<DesktopSkillContent, String> {
    state
        .client
        .get_session_skill_content(&session_id, &name)
        .await
}

#[tauri::command]
async fn upload_file(
    state: State<'_, AppState>,
    filename: String,
    media_type: String,
    bytes: Vec<u8>,
) -> Result<FileMeta, String> {
    state
        .client
        .upload_file(&filename, &media_type, bytes)
        .await
}

#[tauri::command]
async fn set_default_model(state: State<'_, AppState>, model: String) -> Result<(), String> {
    state.client.set_default_model(&model).await
}

#[tauri::command]
async fn list_workspaces(state: State<'_, AppState>) -> Result<Vec<DesktopWorkspace>, String> {
    state.client.list_workspaces().await
}

#[tauri::command]
async fn create_or_touch_workspace(
    state: State<'_, AppState>,
    root: String,
    name: Option<String>,
) -> Result<DesktopWorkspace, String> {
    let workspace = state
        .client
        .create_or_touch_workspace(&root, name.as_deref())
        .await?;
    state
        .web_server
        .desktop_state_changed(DesktopStateChange::WorkspaceUpserted {
            workspace_id: workspace.id.clone(),
        });
    Ok(workspace)
}

#[tauri::command]
async fn remove_workspace(state: State<'_, AppState>, workspace_id: String) -> Result<(), String> {
    state.client.remove_workspace(&workspace_id).await?;
    state
        .web_server
        .desktop_state_changed(DesktopStateChange::WorkspaceRemoved { workspace_id });
    Ok(())
}

#[tauri::command]
async fn list_workspace_sessions(
    state: State<'_, AppState>,
    workspace_id: String,
) -> Result<Vec<SessionSummary>, String> {
    state.client.list_workspace_sessions(&workspace_id).await
}

#[tauri::command]
async fn fork_session(state: State<'_, AppState>, session_id: String) -> Result<String, String> {
    let session_id = state.client.fork_session(&session_id).await?;
    state
        .web_server
        .desktop_state_changed(DesktopStateChange::SessionForked {
            session_id: session_id.clone(),
        });
    Ok(session_id)
}

#[tauri::command]
async fn archive_session(state: State<'_, AppState>, session_id: String) -> Result<(), String> {
    state.client.archive_session(&session_id).await?;
    state
        .web_server
        .desktop_state_changed(DesktopStateChange::SessionArchived { session_id });
    Ok(())
}

#[tauri::command]
async fn prepare_session(
    state: State<'_, AppState>,
    request: DesktopPrepareSessionRequest,
) -> Result<DesktopPreparedSession, String> {
    let creating = request
        .session_id
        .as_deref()
        .is_none_or(|session_id| session_id.trim().is_empty());
    let workspace_root = request.work_dir.clone();
    let session = state.client.prepare_session(request).await?;
    if creating {
        state
            .web_server
            .desktop_state_changed(DesktopStateChange::SessionCreated {
                session_id: session.session_id.clone(),
                workspace_root,
            });
    }
    Ok(session)
}

#[tauri::command]
async fn conversation_context_usage(
    state: State<'_, AppState>,
    session_id: String,
) -> Result<Option<DesktopContextUsage>, String> {
    state.client.context_usage(&session_id).await
}

#[tauri::command]
async fn list_conversation_messages(
    state: State<'_, AppState>,
    session_id: String,
) -> Result<DesktopMessagePage, String> {
    state.client.list_messages(&session_id).await
}

#[tauri::command]
async fn agent_rpc(
    state: State<'_, AppState>,
    request: AgentRpcRequest,
) -> Result<Value, RpcError> {
    dispatch_agent_rpc(&state.client, request).await
}

#[tauri::command]
fn get_goal_mode(state: State<'_, AppState>, session_id: String) -> bool {
    state.web_server.goal_mode(&session_id)
}

#[tauri::command]
fn set_goal_mode(state: State<'_, AppState>, session_id: String, enabled: bool) {
    state.web_server.set_goal_mode(session_id, enabled);
}

#[tauri::command]
async fn get_web_server_status(state: State<'_, AppState>) -> Result<WebServerStatus, String> {
    Ok(state.web_server.status().await)
}

#[tauri::command]
async fn set_web_server_settings(
    state: State<'_, AppState>,
    settings: WebServerSettings,
) -> Result<WebServerStatus, String> {
    state.web_server.set_settings(settings).await
}

#[tauri::command]
async fn list_plugins(state: State<'_, AppState>) -> Result<Vec<PluginSummary>, String> {
    state.client.list_plugins().await
}

#[tauri::command]
async fn install_plugin(
    app: AppHandle,
    state: State<'_, AppState>,
    source: String,
    operation_id: String,
) -> Result<PluginSummary, String> {
    let progress_app = app.clone();
    let progress_operation_id = operation_id.clone();
    let progress: PluginInstallProgressCallback = Arc::new(move |progress| {
        let _ = progress_app.emit(
            "plugin-install-progress",
            PluginInstallProgressEvent {
                operation_id: progress_operation_id.clone(),
                progress,
            },
        );
    });
    state
        .client
        .install_plugin_with_progress(source, progress)
        .await
}

#[tauri::command]
async fn set_plugin_enabled(
    state: State<'_, AppState>,
    id: String,
    enabled: bool,
) -> Result<(), String> {
    state.client.set_plugin_enabled(id, enabled).await
}

#[tauri::command]
async fn set_plugin_mcp_server_enabled(
    state: State<'_, AppState>,
    id: String,
    server: String,
    enabled: bool,
) -> Result<(), String> {
    state
        .client
        .set_plugin_mcp_server_enabled(id, server, enabled)
        .await
}

#[tauri::command]
async fn remove_plugin(state: State<'_, AppState>, id: String) -> Result<(), String> {
    state.client.remove_plugin(id).await
}

#[tauri::command]
async fn reload_plugins(state: State<'_, AppState>) -> Result<ReloadSummary, String> {
    state.client.reload_plugins().await
}

#[tauri::command]
async fn get_plugin_info(state: State<'_, AppState>, id: String) -> Result<PluginInfo, String> {
    state.client.get_plugin_info(id).await
}

#[tauri::command]
async fn check_plugin_updates(
    state: State<'_, AppState>,
) -> Result<Vec<PluginUpdateStatus>, String> {
    state.client.check_plugin_updates().await
}

#[tauri::command]
async fn get_plugin_marketplace() -> Result<PluginMarketplace, String> {
    load_plugin_marketplace().await
}

#[tauri::command]
async fn subscribe_agent_events(
    app: AppHandle,
    state: State<'_, AppState>,
    session_id: String,
    agent_id: String,
) -> Result<String, String> {
    let subscription_id = state
        .next_subscription_id
        .fetch_add(1, Ordering::Relaxed)
        .to_string();
    let app_for_event = app.clone();
    let session_for_event = session_id.clone();
    let app_for_interactions = app.clone();
    let session_for_interactions = session_id.clone();
    let subscription = state
        .client
        .subscribe_agent_events(
            &session_id,
            &agent_id,
            Arc::new(move |event_agent_id, event| {
                let _ = app_for_event.emit(
                    "agent-event",
                    AgentEvent {
                        session_id: session_for_event.clone(),
                        agent_id: event_agent_id,
                        event,
                    },
                );
            }),
            Arc::new(move |interactions| {
                let _ = app_for_interactions.emit(
                    "agent-interactions",
                    AgentInteractionsEvent {
                        session_id: session_for_interactions.clone(),
                        interactions,
                    },
                );
            }),
        )
        .await?;
    state
        .subscriptions
        .lock()
        .map_err(|_| "agent subscription registry is unavailable".to_owned())?
        .insert(subscription_id.clone(), subscription);
    Ok(subscription_id)
}

#[tauri::command]
fn unsubscribe_agent_events(
    state: State<'_, AppState>,
    subscription_id: String,
) -> Result<(), String> {
    let subscription = state
        .subscriptions
        .lock()
        .map_err(|_| "agent subscription registry is unavailable".to_owned())?
        .remove(&subscription_id);
    if let Some(subscription) = subscription {
        subscription.dispose().map_err(|error| error.to_string())?;
    }
    Ok(())
}

#[tauri::command]
async fn respond_interaction(
    state: State<'_, AppState>,
    session_id: String,
    interaction_id: String,
    response: Value,
) -> Result<(), String> {
    state
        .client
        .respond_interaction(&session_id, &interaction_id, response)
        .await
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .setup(|app| {
            let client = Arc::new(
                KimiCodeDesktopClient::bootstrap(env!("CARGO_PKG_VERSION")).map_err(|error| {
                    format!("failed to initialize Kimi Code agent core: {error}")
                })?,
            );
            let app_handle = app.handle().clone();
            let assets: Arc<dyn AssetProvider> = Arc::new(move |path: &str| {
                app_handle
                    .asset_resolver()
                    .get(path.to_owned())
                    .map(|asset| WebAsset {
                        bytes: asset.bytes,
                        mime_type: asset.mime_type,
                        csp_header: asset.csp_header,
                    })
            });
            let app_config_dir = app
                .path()
                .app_config_dir()
                .map_err(|error| format!("failed to resolve app config directory: {error}"))?;
            let kimi_home = resolve_kimi_home(None)
                .map_err(|error| format!("failed to resolve Kimi Code home: {error}"))?;
            let web_server = Arc::new(WebServerController::new(
                Arc::clone(&client),
                assets,
                env!("CARGO_PKG_VERSION"),
                app_config_dir.join("web-server.json"),
                kimi_home.join("server.token"),
            ));
            let app_for_events = app.handle().clone();
            let application_event_subscription =
                web_server.subscribe_application_events(Arc::new(move |event, payload| {
                    let _ = app_for_events.emit(event, payload);
                }));
            app.manage(AppState {
                client,
                web_server: Arc::clone(&web_server),
                subscriptions: Mutex::new(HashMap::new()),
                next_subscription_id: AtomicU64::new(1),
                _application_event_subscription: application_event_subscription,
            });
            tauri::async_runtime::spawn(async move {
                web_server.restore().await;
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            auth_status,
            account_usage,
            login,
            logout,
            list_models,
            refresh_models,
            list_skills,
            get_skill_content,
            upload_file,
            set_default_model,
            list_workspaces,
            create_or_touch_workspace,
            remove_workspace,
            list_workspace_sessions,
            fork_session,
            archive_session,
            prepare_session,
            conversation_context_usage,
            list_conversation_messages,
            agent_rpc,
            get_goal_mode,
            set_goal_mode,
            get_web_server_status,
            set_web_server_settings,
            list_plugins,
            install_plugin,
            set_plugin_enabled,
            set_plugin_mcp_server_enabled,
            remove_plugin,
            reload_plugins,
            get_plugin_info,
            check_plugin_updates,
            get_plugin_marketplace,
            subscribe_agent_events,
            unsubscribe_agent_events,
            respond_interaction
        ])
        .build(tauri::generate_context!())
        .expect("error while building Kimi Code desktop")
        .run(|app, event| {
            if matches!(event, tauri::RunEvent::Exit) {
                let web_server = Arc::clone(&app.state::<AppState>().web_server);
                tauri::async_runtime::block_on(async move {
                    web_server.shutdown().await;
                });
            }
        });
}
