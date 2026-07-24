use std::path::PathBuf;
use std::sync::Arc;

use crate::services::auth::AuthTokenService;
use crate::services::server_logger::{ServerLogLevel, ServerLogger};
use crate::transport::ws::connection_registry::ConnectionRegistry;

pub const DEFAULT_HOST: &str = "127.0.0.1";
pub const DEFAULT_PORT: u16 = 58_627;

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
            .finish_non_exhaustive()
    }
}

#[derive(Debug)]
pub struct RunningServer {
    pub connection_registry: Arc<ConnectionRegistry>,
    pub auth_token_service: AuthTokenService,
    pub host: String,
    pub port: u16,
}

impl RunningServer {
    pub async fn close(self) {
        // MIGRATION-TODO:
        // Original: start.ts, RunningServer.close()
        // Missing dependency: agent-core-v2 Scope disposal, event broadcaster,
        // model refresh scheduler and filesystem watch bridge lifecycles.
        todo!("finish ordered shutdown after kimi-code-agent-core-v2 is complete")
    }
}

// Original: packages/kap-server/src/start.ts, startServer().
pub async fn start_server(_options: ServerStartOptions) -> RunningServer {
    // MIGRATION-TODO:
    // Missing dependency: agent-core-v2 bootstrap(), Scope seeds, config,
    // workspace/session services, provider discovery and route service
    // registrations. The server-local services used by this composition root
    // are migrated in this crate; do not fabricate a partial HTTP daemon.
    // Completion condition: the required agent-core-v2 contracts and bootstrap
    // lifecycle are complete.
    todo!("bootstrap kap-server after kimi-code-agent-core-v2 is complete")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_typescript_server() {
        assert_eq!(DEFAULT_HOST, "127.0.0.1");
        assert_eq!(DEFAULT_PORT, 58_627);
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
}
