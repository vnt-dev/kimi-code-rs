use std::sync::Arc;

use chrono::{SecondsFormat, Utc};
use tokio::sync::watch;

use crate::middleware::hostnames::HostCheckOptions;
use crate::middleware::rate_limit::AuthFailureLimiter;
use crate::security::bind_classify::BindClass;
use crate::services::auth::{AuthTokenService, CredentialValidator};
use crate::services::gui_store::GuiStoreService;
use crate::transport::ws::connection_registry::ConnectionRegistry;

pub struct AppState {
    pub auth_token_service: AuthTokenService,
    pub credential_validator: CredentialValidator,
    pub connection_registry: Arc<ConnectionRegistry>,
    pub gui_store: Arc<GuiStoreService>,
    pub host: String,
    pub host_check: HostCheckOptions,
    pub allowed_origins: Vec<String>,
    pub disable_auth: bool,
    pub auth_failure_limiter: Option<Arc<AuthFailureLimiter>>,
    pub exposure_class: BindClass,
    pub enable_shutdown: bool,
    pub server_version: String,
    pub server_id: String,
    pub started_at: String,
    pub shutdown: watch::Sender<bool>,
}

impl AppState {
    pub fn started_at_now() -> String {
        Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
    }
}
