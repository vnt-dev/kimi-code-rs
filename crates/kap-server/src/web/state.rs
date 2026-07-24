use std::path::PathBuf;
use std::sync::Arc;

use kimi_code_agent_core_v2::_base::utils::iso_date_time::{IsoDateTime, now_iso_date_time};
use kimi_code_agent_core_v2::app::auth_legacy::AuthLegacyServiceHandle;
use kimi_code_agent_core_v2::app::config::ConfigServiceHandle;
use kimi_code_agent_core_v2::app::event::EventServiceHandle;
use tokio::sync::watch;

use crate::middleware::hostnames::HostCheckOptions;
use crate::middleware::rate_limit::AuthFailureLimiter;
use crate::security::bind_classify::BindClass;
use crate::services::auth::{AuthTokenService, CredentialValidator};
use crate::services::gui_store::GuiStoreService;
use crate::transport::ws::connection_registry::ConnectionRegistry;

use super::core_bridge::AgentCoreBridge;

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
    pub enable_terminals: bool,
    pub debug_endpoints: bool,
    pub server_version: String,
    pub server_id: String,
    pub started_at: IsoDateTime,
    pub shutdown: watch::Sender<bool>,
    pub auth_legacy_service: Option<AuthLegacyServiceHandle>,
    pub config_service: Option<ConfigServiceHandle>,
    pub event_service: Option<EventServiceHandle>,
    pub core_bridge: Arc<dyn AgentCoreBridge>,
    pub web_assets_dir: Option<PathBuf>,
}

impl AppState {
    pub fn started_at_now() -> IsoDateTime {
        now_iso_date_time()
    }
}
