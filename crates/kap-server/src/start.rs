use std::collections::HashMap;
use std::io;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::Router;
use kimi_code_agent_core_v2::app::auth_legacy::AuthLegacyServiceHandle;
use kimi_code_agent_core_v2::app::bootstrap::{
    BootstrapInput, BootstrapResolveError, resolve_bootstrap_options,
};
use thiserror::Error;
use tokio::net::TcpListener;
use tokio::sync::watch;
use tokio::task::{JoinError, JoinHandle};

use crate::instance_registry::{InstanceRegistry, InstanceRegistryOptions, RegistrationInfo};
use crate::middleware::hostnames::HostCheckOptions;
use crate::middleware::rate_limit::{AuthFailureLimiter, AuthFailureLimiterOptions};
use crate::routes::web_assets::validate_web_assets;
use crate::security::bind_classify::{BindClass, ClassifyOptions, classify};
use crate::services::auth::{
    AuthTokenService, CredentialValidator, PasswordError, PrivateFileError, TokenStore,
    create_auth_token_service, resolve_password_hash,
};
use crate::services::gui_store::GuiStoreService;
use crate::services::server_logger::{ServerLogLevel, ServerLogger, create_server_logger};
use crate::transport::ws::connection_registry::ConnectionRegistry;
use crate::version::get_server_version;
use crate::web::{AgentCoreBridge, AppState, TodoAgentCoreBridge, create_router};

pub const DEFAULT_HOST: &str = "127.0.0.1";
pub const DEFAULT_PORT: u16 = 58_627;
pub const PORT_RETRY_LIMIT: usize = 100;

#[derive(Default)]
pub struct ServerStartOptions {
    pub host: Option<String>,
    pub port: Option<u16>,
    pub home_dir: Option<PathBuf>,
    pub config_path: Option<PathBuf>,
    pub instances_dir: Option<PathBuf>,
    pub log_level: Option<ServerLogLevel>,
    pub logger: Option<Arc<dyn ServerLogger>>,
    pub debug_endpoints: bool,
    pub bind_class: Option<crate::security::bind_classify::WildcardBindClass>,
    pub allowed_hosts: Vec<String>,
    pub cors_origins: Vec<String>,
    pub disable_host_check: bool,
    pub insecure_no_tls: bool,
    pub allow_remote_shutdown: bool,
    pub allow_remote_terminals: bool,
    pub auth_token_service: Option<AuthTokenService>,
    pub disable_auth: bool,
    pub rpc_token: Option<String>,
    pub skill_dirs: Vec<PathBuf>,
    pub web_assets_dir: Option<PathBuf>,
    pub version: Option<String>,
    pub auth_legacy_service: Option<AuthLegacyServiceHandle>,
    pub core_bridge: Option<Arc<dyn AgentCoreBridge>>,
}

impl std::fmt::Debug for ServerStartOptions {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ServerStartOptions")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("home_dir", &self.home_dir)
            .field("config_path", &self.config_path)
            .field("instances_dir", &self.instances_dir)
            .field("log_level", &self.log_level)
            .field("debug_endpoints", &self.debug_endpoints)
            .field("bind_class", &self.bind_class)
            .field("allowed_hosts", &self.allowed_hosts)
            .field("cors_origins", &self.cors_origins)
            .field("disable_host_check", &self.disable_host_check)
            .field("insecure_no_tls", &self.insecure_no_tls)
            .field("allow_remote_shutdown", &self.allow_remote_shutdown)
            .field("allow_remote_terminals", &self.allow_remote_terminals)
            .field(
                "auth_token_service",
                &self.auth_token_service.as_ref().map(|_| "[configured]"),
            )
            .field("disable_auth", &self.disable_auth)
            .field("rpc_token", &self.rpc_token.as_ref().map(|_| "[redacted]"))
            .field("skill_dirs", &self.skill_dirs)
            .field("web_assets_dir", &self.web_assets_dir)
            .field("version", &self.version)
            .field(
                "auth_legacy_service",
                &self.auth_legacy_service.as_ref().map(|_| "[configured]"),
            )
            .field(
                "core_bridge",
                &self.core_bridge.as_ref().map(|_| "[configured]"),
            )
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Error)]
pub enum StartServerError {
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error(transparent)]
    PrivateFile(#[from] PrivateFileError),
    #[error(transparent)]
    Password(#[from] PasswordError),
    #[error(transparent)]
    Bootstrap(#[from] BootstrapResolveError),
    #[error("server task failed: {0}")]
    Join(#[from] JoinError),
    #[error(
        "refusing to bind {host} ({exposure_class:?}) without TLS; terminate TLS at a reverse proxy or pass --insecure-no-tls"
    )]
    NonLoopbackWithoutTls {
        host: String,
        exposure_class: BindClass,
    },
    #[error(
        "Kimi web assets were not found at {path}. Run the package build before starting the server"
    )]
    WebAssetsNotFound {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

pub struct RunningServer {
    pub app: Router,
    pub connection_registry: Arc<ConnectionRegistry>,
    pub auth_token_service: AuthTokenService,
    pub host: String,
    pub port: u16,
    shutdown: watch::Sender<bool>,
    server_task: JoinHandle<io::Result<()>>,
    auth_failure_limiter: Option<Arc<AuthFailureLimiter>>,
}

impl std::fmt::Debug for RunningServer {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RunningServer")
            .field("connection_registry", &self.connection_registry)
            .field("host", &self.host)
            .field("port", &self.port)
            .finish_non_exhaustive()
    }
}

impl RunningServer {
    // Original: start.ts, RunningServer.close().
    pub async fn close(self) -> Result<(), StartServerError> {
        self.connection_registry
            .close_all(Some("server shutting down"));
        let _ = self.shutdown.send(true);
        self.server_task.await??;
        if let Some(limiter) = &self.auth_failure_limiter {
            limiter.dispose();
        }

        // MIGRATION-TODO:
        // Original: start.ts, close() also disposes the agent-core-v2 Scope,
        // model refresh scheduler, event broadcaster and filesystem watches.
        // These lifecycles will be attached here when agent-core-v2 is complete.
        Ok(())
    }
}

// Original: packages/kap-server/src/start.ts, startServer().
pub async fn start_server(options: ServerStartOptions) -> Result<RunningServer, StartServerError> {
    let host = options.host.unwrap_or_else(|| DEFAULT_HOST.to_owned());
    let requested_port = options.port.unwrap_or(DEFAULT_PORT);
    let version = options
        .version
        .unwrap_or_else(|| get_server_version().to_owned());
    let bootstrap_options = resolve_bootstrap_options(BootstrapInput {
        home_dir: options.home_dir,
        config_path: options.config_path,
        client_version: Some(version.clone()),
        ..BootstrapInput::default()
    })?;
    let home_dir = bootstrap_options.home_dir;
    let instances_dir = options
        .instances_dir
        .unwrap_or_else(|| home_dir.join("server").join("instances"));
    let started_at_ms = now_millis();
    let registry = InstanceRegistry::create(InstanceRegistryOptions {
        instances_dir: Some(instances_dir),
        ..InstanceRegistryOptions::default()
    });
    let registration = Arc::new(
        registry
            .register(RegistrationInfo {
                pid: std::process::id(),
                host: host.clone(),
                port: requested_port,
                started_at: started_at_ms,
                host_version: Some(version.clone()),
            })
            .await?,
    );

    let exposure_class = classify(
        &host,
        ClassifyOptions {
            bind_class: options.bind_class,
        },
    );
    if exposure_class != BindClass::Loopback && !options.insecure_no_tls {
        registration.release().await?;
        return Err(StartServerError::NonLoopbackWithoutTls {
            host,
            exposure_class,
        });
    }

    let logger = options
        .logger
        .unwrap_or_else(|| create_server_logger(options.log_level.unwrap_or(ServerLogLevel::Info)));
    let auth_token_service = match options.auth_token_service {
        Some(service) => service,
        None => match create_default_auth_service(&home_dir).await {
            Ok(service) => service,
            Err(error) => {
                registration.release().await?;
                return Err(error);
            }
        },
    };
    let credential_validator =
        CredentialValidator::new(auth_token_service.clone(), options.rpc_token);
    let web_assets_dir = options.web_assets_dir;
    if let Some(assets_dir) = &web_assets_dir
        && let Err(source) = validate_web_assets(assets_dir).await
    {
        registration.release().await?;
        return Err(StartServerError::WebAssetsNotFound {
            path: assets_dir.clone(),
            source,
        });
    }
    let connection_registry = Arc::new(ConnectionRegistry::default());
    let auth_failure_limiter = (exposure_class != BindClass::Loopback)
        .then(|| Arc::new(AuthFailureLimiter::new(AuthFailureLimiterOptions::default())));
    let (shutdown, shutdown_rx) = watch::channel(false);
    let enable_shutdown = exposure_class == BindClass::Loopback || options.allow_remote_shutdown;
    let enable_terminals = exposure_class == BindClass::Loopback || options.allow_remote_terminals;
    let debug_endpoints = exposure_class == BindClass::Loopback && options.debug_endpoints;
    let core_bridge = options
        .core_bridge
        .unwrap_or_else(|| Arc::new(TodoAgentCoreBridge));
    let state = Arc::new(AppState {
        auth_token_service: auth_token_service.clone(),
        credential_validator,
        connection_registry: Arc::clone(&connection_registry),
        gui_store: Arc::new(GuiStoreService::new(&home_dir)),
        host: host.clone(),
        host_check: HostCheckOptions {
            bound_host: Some(host.clone()),
            extra: options.allowed_hosts,
            disable: options.disable_host_check,
        },
        allowed_origins: options.cors_origins,
        disable_auth: options.disable_auth,
        auth_failure_limiter: auth_failure_limiter.clone(),
        exposure_class,
        enable_shutdown,
        enable_terminals,
        debug_endpoints,
        server_version: version,
        server_id: registration.server_id.clone(),
        started_at: AppState::started_at_now(),
        shutdown: shutdown.clone(),
        auth_legacy_service: options.auth_legacy_service,
        core_bridge,
        web_assets_dir,
    });
    let app = create_router(state);
    let listener = match listen_with_port_retry(&host, requested_port, PORT_RETRY_LIMIT).await {
        Ok(listener) => listener,
        Err(error) => {
            registration.release().await?;
            return Err(error.into());
        }
    };
    let port = listener.local_addr()?.port();
    registration.update(Some(port)).await?;

    let server_app = app.clone();
    let task_registration = Arc::clone(&registration);
    let server_task = tokio::spawn(async move {
        let result = axum::serve(
            listener,
            server_app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .with_graceful_shutdown(wait_for_shutdown(shutdown_rx))
        .await;
        let release_result = task_registration.release().await;
        result.and(release_result)
    });

    logger.log(
        ServerLogLevel::Info,
        serde_json::json!({"host": host, "port": port}),
        "kap-server listening",
    );
    Ok(RunningServer {
        app,
        connection_registry,
        auth_token_service,
        host,
        port,
        shutdown,
        server_task,
        auth_failure_limiter,
    })
}

async fn create_default_auth_service(
    home_dir: &Path,
) -> Result<AuthTokenService, StartServerError> {
    let token_store = Arc::new(TokenStore::create(home_dir).await?);
    let environment = std::env::vars().collect::<HashMap<_, _>>();
    let password_hash = resolve_password_hash(&environment).await?;
    Ok(create_auth_token_service(token_store, password_hash))
}

async fn wait_for_shutdown(mut shutdown: watch::Receiver<bool>) {
    if *shutdown.borrow() {
        return;
    }
    while shutdown.changed().await.is_ok() {
        if *shutdown.borrow() {
            return;
        }
    }
}

// Original: start.ts, listenWithPortRetry().
pub async fn listen_with_port_retry(
    host: &str,
    requested_port: u16,
    max_retries: usize,
) -> io::Result<TcpListener> {
    if requested_port == 0 {
        return TcpListener::bind((host, 0)).await;
    }

    let mut port = requested_port;
    for attempt in 0..=max_retries {
        match TcpListener::bind((host, port)).await {
            Ok(listener) => return Ok(listener),
            Err(error)
                if error.kind() == io::ErrorKind::AddrInUse
                    && attempt < max_retries
                    && port < u16::MAX =>
            {
                port += 1;
            }
            Err(error) => return Err(error),
        }
    }
    unreachable!("bounded retry loop always returns")
}

fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use async_trait::async_trait;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use futures_util::{SinkExt, StreamExt};
    use kimi_code_agent_core_v2::app::auth_legacy::{
        AuthLegacyResult, AuthLegacyServiceContract, AuthSummary as CoreAuthSummary,
        ManagedProviderStatus as CoreManagedProviderStatus,
        ManagedProviderSummary as CoreManagedProviderSummary,
    };
    use serde_json::Value;
    use tokio_tungstenite::connect_async;
    use tokio_tungstenite::tungstenite::Message as ClientWsMessage;
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    use tokio_tungstenite::tungstenite::http::HeaderValue as WsHeaderValue;
    use tower::ServiceExt;

    use super::*;
    use crate::web::{CoreHttpRequest, CoreHttpResponse, CoreOperation};

    #[derive(Default)]
    struct RecordingCoreBridge {
        calls: Mutex<Vec<(CoreOperation, CoreHttpRequest)>>,
    }

    struct StubAuthLegacyService {
        result: Result<CoreAuthSummary, String>,
        calls: AtomicUsize,
    }

    impl StubAuthLegacyService {
        fn succeeds(summary: CoreAuthSummary) -> Self {
            Self {
                result: Ok(summary),
                calls: AtomicUsize::new(0),
            }
        }

        fn fails(message: impl Into<String>) -> Self {
            Self {
                result: Err(message.into()),
                calls: AtomicUsize::new(0),
            }
        }
    }

    #[async_trait]
    impl AuthLegacyServiceContract for StubAuthLegacyService {
        async fn get(&self) -> AuthLegacyResult<CoreAuthSummary> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.result
                .as_ref()
                .cloned()
                .map_err(|message| std::io::Error::other(message.clone()).into())
        }
    }

    #[async_trait]
    impl AgentCoreBridge for RecordingCoreBridge {
        async fn invoke(
            &self,
            operation: CoreOperation,
            request: CoreHttpRequest,
        ) -> CoreHttpResponse {
            self.calls
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .push((operation, request));
            CoreHttpResponse::json(serde_json::json!({"proxied": true}))
        }
    }

    #[test]
    fn defaults_match_typescript_server() {
        assert_eq!(DEFAULT_HOST, "127.0.0.1");
        assert_eq!(DEFAULT_PORT, 58_627);
        assert_eq!(PORT_RETRY_LIMIT, 100);
        let options = ServerStartOptions::default();
        assert!(options.host.is_none());
        assert!(options.port.is_none());
        assert!(!options.debug_endpoints);
        assert!(!options.disable_auth);
    }

    #[test]
    fn debug_output_redacts_credentials() {
        let options = ServerStartOptions {
            rpc_token: Some("secret".into()),
            ..ServerStartOptions::default()
        };
        let debug = format!("{options:?}");
        assert!(!debug.contains("secret"));
        assert!(debug.contains("[redacted]"));
    }

    #[tokio::test]
    async fn retries_the_next_port_and_ephemeral_bind() {
        let occupied = TcpListener::bind((DEFAULT_HOST, 0)).await.unwrap();
        let occupied_port = occupied.local_addr().unwrap().port();
        if occupied_port < u16::MAX {
            let listener = listen_with_port_retry(DEFAULT_HOST, occupied_port, 1)
                .await
                .unwrap();
            assert_eq!(listener.local_addr().unwrap().port(), occupied_port + 1);
        }
        let listener = listen_with_port_retry(DEFAULT_HOST, 0, 0).await.unwrap();
        assert_ne!(listener.local_addr().unwrap().port(), 0);
    }

    #[tokio::test]
    async fn starts_serves_health_and_releases_registration_on_close() {
        let home = tempfile::tempdir().unwrap();
        let server = start_server(ServerStartOptions {
            port: Some(0),
            home_dir: Some(home.path().to_owned()),
            ..ServerStartOptions::default()
        })
        .await
        .unwrap();
        let registration_path = home.path().join("server").join("instances").join(format!(
            "{}.json",
            std::fs::read_dir(home.path().join("server").join("instances"))
                .unwrap()
                .next()
                .unwrap()
                .unwrap()
                .file_name()
                .to_string_lossy()
                .trim_end_matches(".json")
        ));
        assert!(registration_path.exists());

        let response = server
            .app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/healthz")
                    .header("host", "localhost")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        server.close().await.unwrap();
        assert!(!registration_path.exists());
    }

    #[tokio::test]
    async fn protects_authenticated_routes_and_docs() {
        let home = tempfile::tempdir().unwrap();
        let server = start_server(ServerStartOptions {
            port: Some(0),
            home_dir: Some(home.path().to_owned()),
            ..ServerStartOptions::default()
        })
        .await
        .unwrap();
        for path in ["/api/v1/meta", "/openapi.json", "/asyncapi.json"] {
            let response = server
                .app
                .clone()
                .oneshot(
                    Request::builder()
                        .uri(path)
                        .header("host", "localhost")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        }
        server.close().await.unwrap();
    }

    #[tokio::test]
    async fn dispatches_core_routes_at_the_interface_boundary() {
        let home = tempfile::tempdir().unwrap();
        let bridge = Arc::new(RecordingCoreBridge::default());
        let auth = Arc::new(StubAuthLegacyService::succeeds(CoreAuthSummary {
            ready: true,
            providers_count: 1,
            default_model: Some("kimi-for-coding".into()),
            managed_provider: None,
        }));
        let server = start_server(ServerStartOptions {
            port: Some(0),
            home_dir: Some(home.path().to_owned()),
            debug_endpoints: true,
            auth_legacy_service: Some(AuthLegacyServiceHandle(
                Arc::clone(&auth) as Arc<dyn AuthLegacyServiceContract>
            )),
            core_bridge: Some(Arc::clone(&bridge) as Arc<dyn AgentCoreBridge>),
            ..ServerStartOptions::default()
        })
        .await
        .unwrap();
        let token = server.auth_token_service.get_token();
        let specs = crate::routes::core_route_specs(true, true);
        for spec in &specs {
            let path = materialize_route_path(spec.runtime_path);
            let response = server
                .app
                .clone()
                .oneshot(
                    Request::builder()
                        .method(spec.method)
                        .uri(path)
                        .header("host", "localhost")
                        .header("authorization", format!("Bearer {token}"))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(
                response.status(),
                StatusCode::OK,
                "{} {}",
                spec.method,
                spec.runtime_path
            );
        }
        {
            let bridge_specs = specs
                .iter()
                .filter(|spec| spec.operation != CoreOperation::GetAuth)
                .collect::<Vec<_>>();
            let calls = bridge
                .calls
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            assert_eq!(calls.len(), bridge_specs.len());
            for (call, spec) in calls.iter().zip(bridge_specs) {
                assert_eq!(call.0, spec.operation);
                assert_eq!(call.1.method, spec.method);
                assert_eq!(call.1.path, materialize_route_path(spec.runtime_path));
            }
        }
        assert_eq!(auth.calls.load(Ordering::SeqCst), 1);
        server.close().await.unwrap();
    }

    #[tokio::test]
    async fn auth_route_returns_legacy_summary_envelope() {
        let home = tempfile::tempdir().unwrap();
        let auth = Arc::new(StubAuthLegacyService::succeeds(CoreAuthSummary {
            ready: true,
            providers_count: 2,
            default_model: Some("kimi-for-coding".into()),
            managed_provider: Some(CoreManagedProviderSummary {
                name: "kimi-code".into(),
                status: CoreManagedProviderStatus::Authenticated,
            }),
        }));
        let server = start_server(ServerStartOptions {
            port: Some(0),
            home_dir: Some(home.path().to_owned()),
            auth_legacy_service: Some(AuthLegacyServiceHandle(
                Arc::clone(&auth) as Arc<dyn AuthLegacyServiceContract>
            )),
            ..ServerStartOptions::default()
        })
        .await
        .unwrap();
        let response = server
            .app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/auth")
                    .header("host", "localhost")
                    .header("x-request-id", "01ARZ3NDEKTSV4RRFFQ69G5FAV")
                    .header(
                        "authorization",
                        format!("Bearer {}", server.auth_token_service.get_token()),
                    )
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["code"], 0);
        assert_eq!(body["msg"], "success");
        assert_eq!(
            body["data"],
            serde_json::json!({
                "ready": true,
                "providers_count": 2,
                "default_model": "kimi-for-coding",
                "managed_provider": {
                    "name": "kimi-code",
                    "status": "authenticated"
                }
            })
        );
        assert_eq!(body["request_id"], "01ARZ3NDEKTSV4RRFFQ69G5FAV");
        assert_eq!(auth.calls.load(Ordering::SeqCst), 1);
        server.close().await.unwrap();
    }

    #[tokio::test]
    async fn auth_route_maps_service_errors_to_internal_error_envelope() {
        let home = tempfile::tempdir().unwrap();
        let auth = Arc::new(StubAuthLegacyService::fails("config load failed"));
        let server = start_server(ServerStartOptions {
            port: Some(0),
            home_dir: Some(home.path().to_owned()),
            auth_legacy_service: Some(AuthLegacyServiceHandle(
                Arc::clone(&auth) as Arc<dyn AuthLegacyServiceContract>
            )),
            ..ServerStartOptions::default()
        })
        .await
        .unwrap();
        let response = server
            .app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/auth")
                    .header("host", "localhost")
                    .header("x-request-id", "01ARZ3NDEKTSV4RRFFQ69G5FAV")
                    .header(
                        "authorization",
                        format!("Bearer {}", server.auth_token_service.get_token()),
                    )
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["code"], 50_001);
        assert_eq!(body["msg"], "config load failed");
        assert!(body["data"].is_null());
        assert_eq!(body["request_id"], "01ARZ3NDEKTSV4RRFFQ69G5FAV");
        assert_eq!(auth.calls.load(Ordering::SeqCst), 1);
        server.close().await.unwrap();
    }

    fn materialize_route_path(path: &str) -> String {
        [
            ("{session_ref}", "session-1"),
            ("{session_id}", "session-1"),
            ("{provider_id}", "provider-1"),
            ("{message_id}", "message-1"),
            ("{approval_id}", "approval-1"),
            ("{workspace_id}", "workspace-1"),
            ("{file_id}", "file-1"),
            ("{terminal_id}", "terminal-1"),
            ("{task_id}", "task-1"),
            ("{agent_id}", "agent-1"),
            ("{service}", "service"),
            ("{method}", "method"),
            ("{item}", "item"),
            ("{tail}", "action"),
            ("{*path}", "directory/file.txt"),
        ]
        .into_iter()
        .fold(path.to_owned(), |path, (capture, value)| {
            path.replace(capture, value)
        })
    }

    #[tokio::test]
    async fn meta_uses_protocol_dto_and_agent_core_iso_time() {
        let home = tempfile::tempdir().unwrap();
        let server = start_server(ServerStartOptions {
            port: Some(0),
            home_dir: Some(home.path().to_owned()),
            version: Some("9.8.7".into()),
            ..ServerStartOptions::default()
        })
        .await
        .unwrap();
        let token = server.auth_token_service.get_token();
        let response = server
            .app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/meta")
                    .header("host", "localhost")
                    .header("authorization", format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let envelope: kimi_code_protocol::Envelope<kimi_code_protocol::MetaResponse> =
            serde_json::from_slice(&body).unwrap();
        let meta = envelope.data.unwrap();
        assert_eq!(meta.server_version, "9.8.7");
        assert_eq!(
            meta.backend,
            Some(kimi_code_protocol::BackendGeneration::V2)
        );
        assert!(meta.started_at.as_ref().ends_with('Z'));
        server.close().await.unwrap();
    }

    #[tokio::test]
    async fn serves_gui_store_interfaces_without_agent_core() {
        let home = tempfile::tempdir().unwrap();
        let server = start_server(ServerStartOptions {
            port: Some(0),
            home_dir: Some(home.path().to_owned()),
            ..ServerStartOptions::default()
        })
        .await
        .unwrap();
        let authorization = format!("Bearer {}", server.auth_token_service.get_token());
        let set_response = server
            .app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/gui/store/setItem")
                    .header("host", "localhost")
                    .header("authorization", &authorization)
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"key":"theme","value":"dark"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(set_response.status(), StatusCode::OK);
        let get_response = server
            .app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/gui/store/getItem?key=theme")
                    .header("host", "localhost")
                    .header("authorization", authorization)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(get_response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(get_response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["data"]["value"], "dark");
        server.close().await.unwrap();
    }

    #[tokio::test]
    async fn websocket_authenticates_with_bearer_protocol_and_tracks_subscriptions() {
        let home = tempfile::tempdir().unwrap();
        let bridge = Arc::new(RecordingCoreBridge::default());
        let server = start_server(ServerStartOptions {
            port: Some(0),
            home_dir: Some(home.path().to_owned()),
            core_bridge: Some(Arc::clone(&bridge) as Arc<dyn AgentCoreBridge>),
            ..ServerStartOptions::default()
        })
        .await
        .unwrap();
        let protocol = format!("kimi-code.bearer.{}", server.auth_token_service.get_token());
        let mut request = format!("ws://{}:{}/api/v1/ws", server.host, server.port)
            .into_client_request()
            .unwrap();
        request.headers_mut().insert(
            "sec-websocket-protocol",
            WsHeaderValue::from_str(&protocol).unwrap(),
        );
        let (mut socket, response) = connect_async(request).await.unwrap();
        assert_eq!(
            response
                .headers()
                .get("sec-websocket-protocol")
                .unwrap()
                .to_str()
                .unwrap(),
            protocol
        );

        let hello = socket.next().await.unwrap().unwrap().into_text().unwrap();
        let hello: Value = serde_json::from_str(&hello).unwrap();
        assert_eq!(hello["type"], "server_hello");
        assert_eq!(hello["payload"]["protocol_version"], 2);
        assert_eq!(hello["payload"]["capabilities"]["event_batching"], false);

        socket
            .send(ClientWsMessage::Text(
                r#"{"type":"client_hello","id":"hello-1","payload":{"client_id":"test","subscriptions":[]}}"#
                    .into(),
            ))
            .await
            .unwrap();
        let hello_ack = socket.next().await.unwrap().unwrap().into_text().unwrap();
        let hello_ack: Value = serde_json::from_str(&hello_ack).unwrap();
        assert_eq!(hello_ack["type"], "ack");
        assert_eq!(hello_ack["id"], "hello-1");

        socket
            .send(ClientWsMessage::Text(
                r#"{"type":"subscribe","id":"sub-1","payload":{"session_ids":["session-1"]}}"#
                    .into(),
            ))
            .await
            .unwrap();
        let subscribe_ack = socket.next().await.unwrap().unwrap().into_text().unwrap();
        let subscribe_ack: Value = serde_json::from_str(&subscribe_ack).unwrap();
        assert_eq!(subscribe_ack["payload"]["accepted"][0], "session-1");
        assert_eq!(server.connection_registry.size(), 1);
        let connection = server.connection_registry.values().pop().unwrap();
        assert!(connection.has_client_hello());
        assert_eq!(
            connection.subscription_session_ids(),
            vec!["session-1".to_owned()]
        );
        {
            let calls = bridge
                .calls
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            assert_eq!(calls.len(), 1);
            assert_eq!(calls[0].0, CoreOperation::WebSocketEventReplay);
            assert_eq!(calls[0].1.method, "WS");
        }

        socket.close(None).await.unwrap();
        for _ in 0..20 {
            if server.connection_registry.is_empty() {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert!(server.connection_registry.is_empty());
        server.close().await.unwrap();
    }

    #[tokio::test]
    async fn websocket_rejects_missing_credentials_and_disallowed_origins() {
        let home = tempfile::tempdir().unwrap();
        let server = start_server(ServerStartOptions {
            port: Some(0),
            home_dir: Some(home.path().to_owned()),
            ..ServerStartOptions::default()
        })
        .await
        .unwrap();
        let url = format!("ws://{}:{}/api/v1/ws", server.host, server.port);
        let missing = connect_async(&url).await.unwrap_err();
        match missing {
            tokio_tungstenite::tungstenite::Error::Http(response) => {
                assert_eq!(response.status(), 401);
            }
            error => panic!("expected HTTP rejection, got {error}"),
        }

        let protocol = format!("kimi-code.bearer.{}", server.auth_token_service.get_token());
        let mut request = url.into_client_request().unwrap();
        request.headers_mut().insert(
            "sec-websocket-protocol",
            WsHeaderValue::from_str(&protocol).unwrap(),
        );
        request
            .headers_mut()
            .insert("origin", WsHeaderValue::from_static("https://evil.example"));
        let disallowed = connect_async(request).await.unwrap_err();
        match disallowed {
            tokio_tungstenite::tungstenite::Error::Http(response) => {
                assert_eq!(response.status(), 403);
            }
            error => panic!("expected HTTP rejection, got {error}"),
        }
        server.close().await.unwrap();
    }

    #[tokio::test]
    async fn serves_auth_exempt_web_assets_with_spa_fallback() {
        let home = tempfile::tempdir().unwrap();
        let assets = tempfile::tempdir().unwrap();
        std::fs::write(assets.path().join("index.html"), "<main>Kimi</main>").unwrap();
        std::fs::write(assets.path().join("app.js"), "console.log('kimi')").unwrap();
        std::fs::create_dir(assets.path().join("settings")).unwrap();
        std::fs::write(
            assets.path().join("settings").join("index.html"),
            "<main>Settings</main>",
        )
        .unwrap();
        let server = start_server(ServerStartOptions {
            port: Some(0),
            home_dir: Some(home.path().to_owned()),
            web_assets_dir: Some(assets.path().to_owned()),
            ..ServerStartOptions::default()
        })
        .await
        .unwrap();

        for (path, expected_type) in [
            ("/", "text/html; charset=utf-8"),
            ("/sessions/local", "text/html; charset=utf-8"),
            ("/app.js", "text/javascript; charset=utf-8"),
        ] {
            let response = server
                .app
                .clone()
                .oneshot(
                    Request::builder()
                        .uri(path)
                        .header("host", "localhost")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(
                response.headers().get("content-type").unwrap(),
                expected_type
            );
        }
        let directory_index = server
            .app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/settings/")
                    .header("host", "localhost")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let directory_body = axum::body::to_bytes(directory_index.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(directory_body, "<main>Settings</main>");
        let reserved = server
            .app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/not-a-route")
                    .header("host", "localhost")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(reserved.status(), StatusCode::UNAUTHORIZED);
        server.close().await.unwrap();
    }

    #[tokio::test]
    async fn rejects_missing_web_asset_build_and_releases_registration() {
        let home = tempfile::tempdir().unwrap();
        let assets = tempfile::tempdir().unwrap();
        let error = start_server(ServerStartOptions {
            port: Some(0),
            home_dir: Some(home.path().to_owned()),
            web_assets_dir: Some(assets.path().to_owned()),
            ..ServerStartOptions::default()
        })
        .await
        .unwrap_err();
        assert!(matches!(error, StartServerError::WebAssetsNotFound { .. }));
        let instances = home.path().join("server").join("instances");
        assert_eq!(std::fs::read_dir(instances).unwrap().count(), 0);
    }
}
