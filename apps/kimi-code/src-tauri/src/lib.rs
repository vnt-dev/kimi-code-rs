use std::sync::Arc;

use kimi_code_agent_core_v2::app::desktop_client::{
    DesktopAuthStatus, DesktopChatDelta, DesktopChatRequest, DesktopChatResult, DesktopDeviceCode,
    DesktopModel, KimiCodeDesktopClient,
};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

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
            let _ = app_for_event.emit("auth-device-code", DeviceCodeEvent::from(code));
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
    state
        .client
        .chat(
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
        )
        .await
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let app_home = app
                .path()
                .app_data_dir()
                .map_err(|error| format!("failed to resolve app data directory: {error}"))?;
            let client = KimiCodeDesktopClient::new(app_home, env!("CARGO_PKG_VERSION"))
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
            send_message
        ])
        .run(tauri::generate_context!())
        .expect("error while running Kimi Code desktop");
}
