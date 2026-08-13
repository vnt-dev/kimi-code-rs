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
        bootstrap::resolve_kimi_home,
        capability::CapabilityStatus,
        desktop_client::{
            DesktopAgentSettings, DesktopAgentSettingsPatch, DesktopAuthStatus,
            DesktopContextUsage, DesktopCreateCronTaskInput, DesktopCronTask, DesktopCustomAgent,
            DesktopDeleteCronTaskInput, DesktopDeleteCustomAgentInput, DesktopDeviceCode,
            DesktopInteraction, DesktopManagedUsage, DesktopManagedUserInfo, DesktopMessagePage,
            DesktopModel, DesktopPrepareSessionRequest, DesktopPreparedSession, DesktopProvider,
            DesktopSaveCustomAgentInput, DesktopSaveProviderInput, DesktopSkill,
            DesktopSkillContent, DesktopUsageStatistics, DesktopWorkspace, KimiCodeDesktopClient,
        },
        file::FileMeta,
        plugin::{
            PluginInfo, PluginInstallOperation, PluginSummary, PluginUpdateStatus, ReloadSummary,
        },
        session_index::SessionSummary,
    },
};
use kimi_code_web_server::{
    AgentRpcRequest, AssetProvider, DesktopStateChange, PluginMarketplace, RpcError, WebAsset,
    WebServerController, WebServerSettings, WebServerStatus, dispatch_agent_rpc,
    load_plugin_marketplace,
};
use serde::Serialize;
use serde_json::Value;
#[cfg(desktop)]
use tauri::menu::MenuBuilder;
#[cfg(desktop)]
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{
    AppHandle, Emitter, Manager, State,
    ipc::{InvokeBody, Request},
};
use tauri_plugin_opener::OpenerExt;

struct AppState {
    client: Arc<KimiCodeDesktopClient>,
    web_server: Arc<WebServerController>,
    subscriptions: Mutex<HashMap<String, DisposableHandle>>,
    next_subscription_id: AtomicU64,
    _application_event_subscription: DisposableHandle,
}

#[cfg(desktop)]
const TRAY_SHOW_MENU_ID: &str = "tray-show";
#[cfg(desktop)]
const TRAY_QUIT_MENU_ID: &str = "tray-quit";

/// Returns localized tray menu labels based on the OS UI language:
/// Chinese for Chinese systems, English for everything else.
#[cfg(desktop)]
fn tray_menu_labels() -> (&'static str, &'static str) {
    let is_chinese = sys_locale::get_locale()
        .map(|locale| locale.to_lowercase().starts_with("zh"))
        .unwrap_or(false);
    if is_chinese {
        ("显示 Kimi Code", "退出")
    } else {
        ("Show Kimi Code", "Quit")
    }
}

#[cfg(desktop)]
fn show_main_window(app: &AppHandle) {
    if let Some(main_window) = app.get_webview_window("main") {
        let _ = main_window.show();
        let _ = main_window.unminimize();
        let _ = main_window.set_focus();
    }
}

#[cfg(desktop)]
#[tauri::command]
fn show_conversation_notification(app: AppHandle, session_id: String, title: String, body: String) {
    #[cfg(windows)]
    {
        std::thread::spawn(move || {
            let app_id = if tauri::is_dev() {
                tauri_winrt_notification::Toast::POWERSHELL_APP_ID.to_owned()
            } else {
                app.config().identifier.clone()
            };
            let app_for_action = app.clone();
            let toast = tauri_winrt_notification::Toast::new(&app_id)
                .title(&title)
                .text1(&body)
                .on_activated(move |_| {
                    show_main_window(&app_for_action);
                    let _ = app_for_action.emit("notification-open-session", &session_id);
                    Ok(())
                });
            let _ = toast.show();
        });
    }

    #[cfg(not(windows))]
    std::thread::spawn(move || {
        let mut notification = notify_rust::Notification::new();
        notification.summary(&title).body(&body).auto_icon();

        #[cfg(target_os = "macos")]
        {
            let application_id = if tauri::is_dev() {
                "com.apple.Terminal"
            } else {
                &app.config().identifier
            };
            let _ = notify_rust::set_application(application_id);
        }

        let Ok(handle) = notification.show() else {
            return;
        };
        let app_for_action = app.clone();
        let _ = handle.wait_for_response(move |response: &notify_rust::NotificationResponse| {
            if !matches!(
                response,
                notify_rust::NotificationResponse::Default
                    | notify_rust::NotificationResponse::Action(_)
            ) {
                return;
            }
            show_main_window(&app_for_action);
            let _ = app_for_action.emit("notification-open-session", &session_id);
        });
    });
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
async fn account_usage(state: State<'_, AppState>) -> Result<DesktopManagedUsage, String> {
    state.client.managed_usage().await
}

#[tauri::command]
async fn get_usage_statistics(
    state: State<'_, AppState>,
) -> Result<DesktopUsageStatistics, String> {
    state.client.usage_statistics().await
}

#[tauri::command]
async fn account_profile(state: State<'_, AppState>) -> Result<DesktopManagedUserInfo, String> {
    state.client.managed_user_info().await
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
async fn list_custom_agents(
    state: State<'_, AppState>,
    workspace_id: String,
) -> Result<Vec<DesktopCustomAgent>, String> {
    state.client.list_custom_agents(&workspace_id).await
}

#[tauri::command]
async fn save_custom_agent(
    state: State<'_, AppState>,
    input: DesktopSaveCustomAgentInput,
) -> Result<DesktopCustomAgent, String> {
    state.client.save_custom_agent(input).await
}

#[tauri::command]
async fn delete_custom_agent(
    state: State<'_, AppState>,
    input: DesktopDeleteCustomAgentInput,
) -> Result<(), String> {
    state.client.delete_custom_agent(input).await
}

#[tauri::command]
async fn list_cron_tasks(
    state: State<'_, AppState>,
    session_id: String,
) -> Result<Vec<DesktopCronTask>, String> {
    state.client.list_cron_tasks(&session_id).await
}

#[tauri::command]
async fn create_cron_task(
    state: State<'_, AppState>,
    input: DesktopCreateCronTaskInput,
) -> Result<DesktopCronTask, String> {
    state.client.create_cron_task(input).await
}

#[tauri::command]
async fn delete_cron_task(
    state: State<'_, AppState>,
    input: DesktopDeleteCronTaskInput,
) -> Result<(), String> {
    state.client.delete_cron_task(input).await
}

#[tauri::command]
async fn upload_file(state: State<'_, AppState>, request: Request<'_>) -> Result<FileMeta, String> {
    let filename = request
        .headers()
        .get("x-kimi-filename")
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| "upload filename header is missing".to_owned())?;
    let filename = percent_encoding::percent_decode_str(filename)
        .decode_utf8()
        .map_err(|_| "upload filename is not valid UTF-8".to_owned())?
        .into_owned();
    let media_type = request
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("application/octet-stream")
        .to_owned();
    let bytes = match request.body() {
        InvokeBody::Raw(bytes) => bytes.clone(),
        InvokeBody::Json(_) => return Err("upload body must be raw bytes".into()),
    };
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
async fn get_agent_settings(state: State<'_, AppState>) -> Result<DesktopAgentSettings, String> {
    state.client.get_agent_settings().await
}

#[tauri::command]
async fn update_agent_settings(
    state: State<'_, AppState>,
    patch: DesktopAgentSettingsPatch,
) -> Result<DesktopAgentSettings, String> {
    state.client.update_agent_settings(patch).await
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
async fn list_archived_sessions(state: State<'_, AppState>) -> Result<Vec<SessionSummary>, String> {
    state.client.list_archived_sessions().await
}

#[tauri::command]
async fn delete_archived_sessions(
    state: State<'_, AppState>,
    session_ids: Vec<String>,
) -> Result<Vec<String>, String> {
    let session_ids = state.client.delete_archived_sessions(&session_ids).await?;
    if !session_ids.is_empty() {
        state
            .web_server
            .desktop_state_changed(DesktopStateChange::SessionsDeleted {
                session_ids: session_ids.clone(),
            });
    }
    Ok(session_ids)
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
async fn restore_session(
    state: State<'_, AppState>,
    session_id: String,
) -> Result<SessionSummary, String> {
    let session = state.client.restore_session(&session_id).await?;
    state
        .web_server
        .desktop_state_changed(DesktopStateChange::SessionRestored {
            session_id: session.id.clone(),
        });
    Ok(session)
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
    agent_id: Option<String>,
) -> Result<DesktopMessagePage, String> {
    state
        .client
        .list_messages(&session_id, agent_id.as_deref())
        .await
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
async fn list_providers(state: State<'_, AppState>) -> Result<Vec<DesktopProvider>, String> {
    state.client.list_providers().await
}

#[tauri::command]
async fn save_provider(
    state: State<'_, AppState>,
    input: DesktopSaveProviderInput,
) -> Result<DesktopProvider, String> {
    state.client.save_provider(input).await
}

#[tauri::command]
async fn delete_provider(state: State<'_, AppState>, id: String) -> Result<(), String> {
    state.client.delete_provider(id).await
}

#[tauri::command]
async fn list_plugins(state: State<'_, AppState>) -> Result<Vec<PluginSummary>, String> {
    state.client.list_plugins().await
}

#[tauri::command]
async fn list_capabilities(state: State<'_, AppState>) -> Result<Vec<CapabilityStatus>, String> {
    state.client.list_capabilities().await
}

#[tauri::command]
async fn get_capability(
    state: State<'_, AppState>,
    id: String,
) -> Result<CapabilityStatus, String> {
    state.client.get_capability(id).await
}

#[tauri::command]
async fn install_capability(
    state: State<'_, AppState>,
    id: String,
) -> Result<CapabilityStatus, String> {
    state.client.install_capability(id).await
}

#[tauri::command]
async fn install_plugin(
    state: State<'_, AppState>,
    source: String,
    operation_id: String,
) -> Result<(), String> {
    // The install runs in the background on the engine; the panel polls
    // `get_plugin_install_progress` for phase/byte progress.
    state
        .client
        .install_plugin_in_background(source, operation_id)
        .await
}

#[tauri::command]
async fn get_plugin_install_progress(
    state: State<'_, AppState>,
    operation_id: String,
) -> Result<Option<PluginInstallOperation>, String> {
    state.client.plugin_install_progress(operation_id).await
}

#[tauri::command]
async fn list_plugin_install_operations(
    state: State<'_, AppState>,
) -> Result<Vec<PluginInstallOperation>, String> {
    state.client.list_plugin_install_operations().await
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
    install_rustls_crypto_provider();

    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _, _| {
            #[cfg(desktop)]
            show_main_window(app);
        }))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .setup(|app| {
            #[cfg(desktop)]
            if let Some(icon) = app.default_window_icon() {
                let (tray_show_label, tray_quit_label) = tray_menu_labels();
                let tray_menu = MenuBuilder::new(app)
                    .text(TRAY_SHOW_MENU_ID, tray_show_label)
                    .separator()
                    .text(TRAY_QUIT_MENU_ID, tray_quit_label)
                    .build()?;

                TrayIconBuilder::with_id("kimi-code")
                    .icon(icon.clone())
                    .menu(&tray_menu)
                    .tooltip("Kimi Code")
                    .show_menu_on_left_click(false)
                    .on_menu_event(|app, event| match event.id().as_ref() {
                        TRAY_SHOW_MENU_ID => show_main_window(app),
                        TRAY_QUIT_MENU_ID => app.exit(0),
                        _ => {}
                    })
                    .on_tray_icon_event(|tray, event| {
                        if let TrayIconEvent::Click {
                            button: MouseButton::Left,
                            button_state: MouseButtonState::Up,
                            ..
                        } = event
                        {
                            show_main_window(tray.app_handle());
                        }
                    })
                    .build(app)?;
            }

            #[cfg(desktop)]
            if let Some(main_window) = app.get_webview_window("main") {
                let window_to_hide = main_window.clone();
                main_window.on_window_event(move |event| {
                    if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                        api.prevent_close();
                        let _ = window_to_hide.hide();
                    }
                });
            }

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
            get_usage_statistics,
            account_profile,
            login,
            logout,
            list_models,
            refresh_models,
            list_skills,
            get_skill_content,
            list_custom_agents,
            save_custom_agent,
            delete_custom_agent,
            list_cron_tasks,
            create_cron_task,
            delete_cron_task,
            upload_file,
            set_default_model,
            get_agent_settings,
            update_agent_settings,
            list_workspaces,
            create_or_touch_workspace,
            remove_workspace,
            list_workspace_sessions,
            list_archived_sessions,
            delete_archived_sessions,
            fork_session,
            archive_session,
            restore_session,
            prepare_session,
            conversation_context_usage,
            list_conversation_messages,
            agent_rpc,
            get_goal_mode,
            set_goal_mode,
            get_web_server_status,
            set_web_server_settings,
            list_providers,
            save_provider,
            delete_provider,
            list_plugins,
            list_capabilities,
            get_capability,
            install_capability,
            install_plugin,
            get_plugin_install_progress,
            list_plugin_install_operations,
            set_plugin_enabled,
            set_plugin_mcp_server_enabled,
            remove_plugin,
            reload_plugins,
            get_plugin_info,
            check_plugin_updates,
            get_plugin_marketplace,
            subscribe_agent_events,
            unsubscribe_agent_events,
            respond_interaction,
            show_conversation_notification
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

fn install_rustls_crypto_provider() {
    // reqwest 0.13 is pulled in by the updater and MCP transport with its
    // provider-neutral rustls feature. Choose a provider before either one can
    // construct an HTTP client. Ignore the result so this remains safe when a
    // dependency has already installed a process-wide provider.
    let _ = rustls::crypto::ring::default_provider().install_default();
}

#[cfg(test)]
mod tests {
    use super::install_rustls_crypto_provider;

    #[test]
    fn installs_rustls_crypto_provider() {
        install_rustls_crypto_provider();

        assert!(rustls::crypto::CryptoProvider::get_default().is_some());
    }
}
