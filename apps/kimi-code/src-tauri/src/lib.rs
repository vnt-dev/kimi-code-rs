use std::sync::Arc;

use kimi_code_agent_core_v2::app::desktop_client::{
    DesktopAuthStatus, DesktopChatDelta, DesktopChatRequest, DesktopChatResult,
    DesktopCompactionEvent, DesktopDeviceCode, DesktopInteraction, DesktopModel,
    KimiCodeDesktopClient,
};
use serde::Serialize;
use serde_json::Value;
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_opener::OpenerExt;

struct AppState {
    client: Arc<KimiCodeDesktopClient>,
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
struct ChatStreamEvent {
    conversation_id: String,
    kind: String,
    content: String,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct AgentInteractionsEvent {
    conversation_id: String,
    interactions: Vec<DesktopInteraction>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct AgentCompactionEvent {
    conversation_id: String,
    event: DesktopCompactionEvent,
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
async fn send_message(
    app: AppHandle,
    state: State<'_, AppState>,
    conversation_id: String,
    request: DesktopChatRequest,
) -> Result<DesktopChatResult, String> {
    let app_for_event = app.clone();
    let conversation_for_event = conversation_id.clone();
    let app_for_interactions = app.clone();
    let conversation_for_interactions = conversation_id.clone();
    let app_for_compaction = app.clone();
    let conversation_for_compaction = conversation_id.clone();
    state
        .client
        .chat(
            &conversation_id,
            request,
            Arc::new(move |DesktopChatDelta { kind, content }| {
                let _ = app_for_event.emit(
                    "chat-stream",
                    ChatStreamEvent {
                        conversation_id: conversation_for_event.clone(),
                        kind,
                        content,
                    },
                );
            }),
            Arc::new(move |interactions| {
                let _ = app_for_interactions.emit(
                    "agent-interactions",
                    AgentInteractionsEvent {
                        conversation_id: conversation_for_interactions.clone(),
                        interactions,
                    },
                );
            }),
            Arc::new(move |event| {
                let _ = app_for_compaction.emit(
                    "agent-compaction",
                    AgentCompactionEvent {
                        conversation_id: conversation_for_compaction.clone(),
                        event,
                    },
                );
            }),
        )
        .await
}

#[tauri::command]
async fn respond_interaction(
    state: State<'_, AppState>,
    conversation_id: String,
    interaction_id: String,
    response: Value,
) -> Result<(), String> {
    state
        .client
        .respond_interaction(&conversation_id, &interaction_id, response)
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
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            auth_status,
            login,
            logout,
            list_models,
            send_message,
            respond_interaction
        ])
        .run(tauri::generate_context!())
        .expect("error while running Kimi Code desktop");
}
