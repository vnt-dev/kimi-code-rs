mod rpc_dispatch;

use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

use kimi_code_agent_core_v2::{
    _base::di::lifecycle::DisposableHandle,
    app::{
        desktop_client::{
            DesktopAuthStatus, DesktopContextUsage, DesktopDeviceCode, DesktopInteraction,
            DesktopMessagePage, DesktopModel, DesktopPrepareSessionRequest, DesktopPreparedSession,
            DesktopWorkspace, KimiCodeDesktopClient,
        },
        session_index::SessionSummary,
    },
};
use rpc_dispatch::{AgentRpcRequest, RpcError};
use serde::Serialize;
use serde_json::Value;
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_opener::OpenerExt;

struct AppState {
    client: Arc<KimiCodeDesktopClient>,
    subscriptions: Mutex<HashMap<String, DisposableHandle>>,
    next_subscription_id: AtomicU64,
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

#[tauri::command]
async fn auth_status(state: State<'_, AppState>) -> Result<DesktopAuthStatus, String> {
    state.client.auth_status().await
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
async fn list_workspaces(state: State<'_, AppState>) -> Result<Vec<DesktopWorkspace>, String> {
    state.client.list_workspaces().await
}

#[tauri::command]
async fn create_or_touch_workspace(
    state: State<'_, AppState>,
    root: String,
    name: Option<String>,
) -> Result<DesktopWorkspace, String> {
    state
        .client
        .create_or_touch_workspace(&root, name.as_deref())
        .await
}

#[tauri::command]
async fn remove_workspace(state: State<'_, AppState>, workspace_id: String) -> Result<(), String> {
    state.client.remove_workspace(&workspace_id).await
}

#[tauri::command]
async fn list_workspace_sessions(
    state: State<'_, AppState>,
    workspace_id: String,
) -> Result<Vec<SessionSummary>, String> {
    state.client.list_workspace_sessions(&workspace_id).await
}

#[tauri::command]
async fn archive_session(state: State<'_, AppState>, session_id: String) -> Result<(), String> {
    state.client.archive_session(&session_id).await
}

#[tauri::command]
async fn prepare_session(
    state: State<'_, AppState>,
    request: DesktopPrepareSessionRequest,
) -> Result<DesktopPreparedSession, String> {
    state.client.prepare_session(request).await
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
    before_id: Option<String>,
    page_size: Option<usize>,
) -> Result<DesktopMessagePage, String> {
    state
        .client
        .list_messages(&session_id, before_id, page_size)
        .await
}

#[tauri::command]
async fn agent_rpc(
    state: State<'_, AppState>,
    request: AgentRpcRequest,
) -> Result<Value, RpcError> {
    rpc_dispatch::dispatch(&state.client, request).await
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
    let agent_for_event = agent_id.clone();
    let app_for_interactions = app.clone();
    let session_for_interactions = session_id.clone();
    let subscription = state
        .client
        .subscribe_agent_events(
            &session_id,
            &agent_id,
            Arc::new(move |event| {
                let _ = app_for_event.emit(
                    "agent-event",
                    AgentEvent {
                        session_id: session_for_event.clone(),
                        agent_id: agent_for_event.clone(),
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
        .setup(|app| {
            let client = KimiCodeDesktopClient::bootstrap(env!("CARGO_PKG_VERSION"))
                .map_err(|error| format!("failed to initialize Kimi Code agent core: {error}"))?;
            app.manage(AppState {
                client: Arc::new(client),
                subscriptions: Mutex::new(HashMap::new()),
                next_subscription_id: AtomicU64::new(1),
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            auth_status,
            login,
            logout,
            list_models,
            list_workspaces,
            create_or_touch_workspace,
            remove_workspace,
            list_workspace_sessions,
            archive_session,
            prepare_session,
            conversation_context_usage,
            list_conversation_messages,
            agent_rpc,
            subscribe_agent_events,
            unsubscribe_agent_events,
            respond_interaction
        ])
        .run(tauri::generate_context!())
        .expect("error while running Kimi Code desktop");
}
