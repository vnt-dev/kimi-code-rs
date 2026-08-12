use std::{
    net::{IpAddr, Ipv4Addr, UdpSocket},
    path::PathBuf,
    sync::Arc,
};

use kimi_code_agent_core_v2::_base::di::lifecycle::DisposableHandle;
use kimi_code_agent_core_v2::app::desktop_client::KimiCodeDesktopClient;
use serde::Serialize;
use tokio::sync::Mutex;

use crate::{
    ApplicationEventHandler, AssetProvider, DesktopStateChange, WebServerListenScope,
    WebServerSettings,
    app_events::ApplicationEventBus,
    server::{RunningServer, start_server},
    settings::{load_or_create_token, load_settings, save_settings, validate_settings},
};

pub const DEFAULT_WEB_SERVER_PORT: u16 = 58_627;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum WebServerState {
    Stopped,
    Starting,
    Running,
    Error,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WebServerStatus {
    pub state: WebServerState,
    pub enabled: bool,
    pub port: u16,
    pub listen_scope: WebServerListenScope,
    pub listen_address: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub origin: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub access_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

struct ControllerState {
    settings: WebServerSettings,
    state: WebServerState,
    running: Option<RunningServer>,
    token: Option<String>,
    error: Option<String>,
}

pub struct WebServerController {
    client: Arc<KimiCodeDesktopClient>,
    assets: Arc<dyn AssetProvider>,
    version: String,
    settings_path: PathBuf,
    token_path: PathBuf,
    events: Arc<ApplicationEventBus>,
    operation: Mutex<()>,
    inner: Mutex<ControllerState>,
}

impl WebServerController {
    pub fn new(
        client: Arc<KimiCodeDesktopClient>,
        assets: Arc<dyn AssetProvider>,
        version: impl Into<String>,
        settings_path: impl Into<PathBuf>,
        token_path: impl Into<PathBuf>,
    ) -> Self {
        Self {
            client,
            assets,
            version: version.into(),
            settings_path: settings_path.into(),
            token_path: token_path.into(),
            events: Arc::new(ApplicationEventBus::default()),
            operation: Mutex::new(()),
            inner: Mutex::new(ControllerState {
                settings: WebServerSettings::default(),
                state: WebServerState::Stopped,
                running: None,
                token: None,
                error: None,
            }),
        }
    }

    pub async fn restore(&self) -> WebServerStatus {
        let _operation = self.operation.lock().await;
        let settings = match load_settings(&self.settings_path).await {
            Ok(settings) => settings,
            Err(error) => {
                let mut inner = self.inner.lock().await;
                inner.state = WebServerState::Error;
                inner.error = Some(error);
                return status_from(&inner);
            }
        };
        if let Err(error) = self.apply_locked(settings, false).await {
            let mut inner = self.inner.lock().await;
            inner.state = WebServerState::Error;
            inner.error = Some(error);
        }
        self.status().await
    }

    pub async fn set_settings(
        &self,
        settings: WebServerSettings,
    ) -> Result<WebServerStatus, String> {
        self.apply(settings, true).await?;
        Ok(self.status().await)
    }

    pub async fn status(&self) -> WebServerStatus {
        let inner = self.inner.lock().await;
        status_from(&inner)
    }

    pub async fn shutdown(&self) {
        let _operation = self.operation.lock().await;
        let running = {
            let mut inner = self.inner.lock().await;
            inner.running.take()
        };
        if let Some(running) = running {
            running.close().await;
        }
        let mut inner = self.inner.lock().await;
        inner.state = WebServerState::Stopped;
        inner.error = None;
    }

    pub fn subscribe_application_events(
        &self,
        handler: ApplicationEventHandler,
    ) -> DisposableHandle {
        self.events.subscribe(handler)
    }

    pub fn desktop_state_changed(&self, change: DesktopStateChange) {
        self.events.desktop_state_changed(change);
    }

    pub fn goal_mode(&self, session_id: &str) -> bool {
        self.events.goal_mode(session_id)
    }

    pub fn set_goal_mode(&self, session_id: String, enabled: bool) {
        self.events.set_goal_mode(session_id, enabled);
    }

    async fn apply(&self, settings: WebServerSettings, persist: bool) -> Result<(), String> {
        validate_settings(settings)?;
        let _operation = self.operation.lock().await;
        self.apply_locked(settings, persist).await
    }

    async fn apply_locked(&self, settings: WebServerSettings, persist: bool) -> Result<(), String> {
        if !settings.enabled {
            if persist {
                save_settings(&self.settings_path, settings).await?;
            }
            let running = {
                let mut inner = self.inner.lock().await;
                inner.settings = settings;
                inner.running.take()
            };
            if let Some(running) = running {
                running.close().await;
            }
            let mut inner = self.inner.lock().await;
            inner.state = WebServerState::Stopped;
            inner.error = None;
            return Ok(());
        }

        let already_running = {
            let mut inner = self.inner.lock().await;
            if inner.running.as_ref().is_some_and(|running| {
                running.port == settings.port && running.listen_scope == settings.listen_scope
            }) {
                true
            } else {
                inner.state = WebServerState::Starting;
                inner.error = None;
                false
            }
        };
        if already_running {
            if persist {
                save_settings(&self.settings_path, settings).await?;
            }
            let mut inner = self.inner.lock().await;
            inner.settings = settings;
            inner.state = WebServerState::Running;
            inner.error = None;
            return Ok(());
        }

        let token = match load_or_create_token(&self.token_path).await {
            Ok(token) => token,
            Err(error) => {
                self.mark_error(&error).await;
                return Err(error);
            }
        };
        let next = match start_server(
            Arc::clone(&self.client),
            Arc::clone(&self.assets),
            Arc::clone(&self.events),
            token.clone(),
            self.version.clone(),
            settings.port,
            settings.listen_scope,
        )
        .await
        {
            Ok(running) => running,
            Err(error) => {
                self.mark_error(&error).await;
                return Err(error);
            }
        };

        if persist && let Err(error) = save_settings(&self.settings_path, settings).await {
            next.close().await;
            self.mark_error(&error).await;
            return Err(error);
        }
        let previous = {
            let mut inner = self.inner.lock().await;
            inner.settings = settings;
            inner.token = Some(token);
            inner.state = WebServerState::Running;
            inner.error = None;
            inner.running.replace(next)
        };
        if let Some(previous) = previous {
            previous.close().await;
        }
        Ok(())
    }

    async fn mark_error(&self, error: &str) {
        let mut inner = self.inner.lock().await;
        inner.state = if inner.running.is_some() {
            WebServerState::Running
        } else {
            WebServerState::Error
        };
        inner.error = Some(error.to_owned());
    }
}

fn status_from(inner: &ControllerState) -> WebServerStatus {
    let port = inner
        .running
        .as_ref()
        .map_or(inner.settings.port, |server| server.port);
    let listen_scope = inner
        .running
        .as_ref()
        .map_or(inner.settings.listen_scope, |server| server.listen_scope);
    let listen_address = listen_scope.bind_address().to_owned();
    let origin = inner.running.as_ref().map(|_| {
        let host = match listen_scope {
            WebServerListenScope::Local => "127.0.0.1".to_owned(),
            WebServerListenScope::Global => global_access_host(),
        };
        format!("http://{host}:{port}")
    });
    let access_url = origin.as_ref().and_then(|origin| {
        inner
            .token
            .as_ref()
            .map(|token| format!("{origin}/#token={token}"))
    });
    WebServerStatus {
        state: inner.state,
        enabled: inner.settings.enabled,
        port,
        listen_scope,
        listen_address,
        origin,
        access_url,
        error: inner.error.clone(),
    }
}

fn global_access_host() -> String {
    UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0))
        .and_then(|socket| {
            socket.connect((Ipv4Addr::new(192, 0, 2, 1), 80))?;
            socket.local_addr()
        })
        .ok()
        .and_then(|address| match address.ip() {
            IpAddr::V4(ip) if !ip.is_unspecified() && !ip.is_loopback() => Some(ip.to_string()),
            _ => None,
        })
        .unwrap_or_else(|| "127.0.0.1".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::WebAsset;
    use uuid::Uuid;

    fn temp_dir() -> PathBuf {
        std::env::temp_dir().join(format!("kimi-web-controller-{}", Uuid::new_v4()))
    }

    fn available_port() -> u16 {
        std::net::TcpListener::bind(("127.0.0.1", 0))
            .unwrap()
            .local_addr()
            .unwrap()
            .port()
    }

    fn controller(root: &std::path::Path) -> WebServerController {
        let home = root.join("home");
        std::fs::create_dir_all(&home).unwrap();
        let client = Arc::new(KimiCodeDesktopClient::new(&home, "test").unwrap());
        let assets: Arc<dyn AssetProvider> = Arc::new(|path: &str| {
            (path == "index.html").then(|| WebAsset {
                bytes: b"Kimi Web".to_vec(),
                mime_type: "text/html".into(),
                csp_header: None,
            })
        });
        WebServerController::new(
            client,
            assets,
            "test",
            root.join("config/web-server.json"),
            home.join("server.token"),
        )
    }

    #[tokio::test]
    async fn starts_switches_transactionally_and_stops() {
        let root = temp_dir();
        let controller = controller(&root);
        let initial = controller.status().await;
        assert_eq!(initial.state, WebServerState::Stopped);
        assert!(!initial.enabled);
        assert_eq!(initial.port, DEFAULT_WEB_SERVER_PORT);

        let first_port = available_port();
        let running = controller
            .set_settings(WebServerSettings {
                enabled: true,
                port: first_port,
                listen_scope: WebServerListenScope::Local,
            })
            .await
            .unwrap();
        assert_eq!(running.state, WebServerState::Running);
        assert_eq!(running.port, first_port);
        assert_eq!(running.listen_scope, WebServerListenScope::Local);
        assert_eq!(running.listen_address, "127.0.0.1");
        let expected_origin = format!("http://127.0.0.1:{first_port}");
        assert_eq!(running.origin.as_deref(), Some(expected_origin.as_str()));
        assert!(running.access_url.as_deref().unwrap().contains("/#token="));

        let occupied = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let occupied_port = occupied.local_addr().unwrap().port();
        let error = controller
            .set_settings(WebServerSettings {
                enabled: true,
                port: occupied_port,
                listen_scope: WebServerListenScope::Local,
            })
            .await
            .unwrap_err();
        assert!(error.contains("failed to listen"));
        let retained = controller.status().await;
        assert_eq!(retained.state, WebServerState::Running);
        assert_eq!(retained.port, first_port);
        assert!(retained.error.is_some());
        drop(occupied);

        let stopped = controller
            .set_settings(WebServerSettings {
                enabled: false,
                port: first_port,
                listen_scope: WebServerListenScope::Local,
            })
            .await
            .unwrap();
        assert_eq!(stopped.state, WebServerState::Stopped);
        assert!(!stopped.enabled);
        assert!(stopped.origin.is_none());

        let global = controller
            .set_settings(WebServerSettings {
                enabled: true,
                port: available_port(),
                listen_scope: WebServerListenScope::Global,
            })
            .await
            .unwrap();
        assert_eq!(global.state, WebServerState::Running);
        assert_eq!(global.listen_scope, WebServerListenScope::Global);
        assert_eq!(global.listen_address, "0.0.0.0");
        controller.shutdown().await;
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn restores_an_enabled_persisted_listener() {
        let root = temp_dir();
        let port = available_port();
        let settings_path = root.join("config/web-server.json");
        save_settings(
            &settings_path,
            WebServerSettings {
                enabled: true,
                port,
                listen_scope: WebServerListenScope::Local,
            },
        )
        .await
        .unwrap();
        let controller = controller(&root);
        let status = controller.restore().await;
        assert_eq!(status.state, WebServerState::Running);
        assert_eq!(status.port, port);
        controller.shutdown().await;
        let _ = std::fs::remove_dir_all(root);
    }
}
