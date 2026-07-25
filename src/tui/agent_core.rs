//! Minimal agent-core-v2 composition owned by the interactive TUI process.
//!
//! Original reference: `apps/kimi-code/src/cli/v2/run-v2-print.ts` creates an
//! agent-core-v2 application before resolving native services. The TypeScript
//! interactive TUI still uses the legacy KimiHarness, so this Rust boundary
//! starts with the one v2 service that can operate independently: OAuth.

use std::{error::Error, fmt, sync::Arc};

use kimi_code_agent_core_v2::app::{
    auth::{AuthOperationError, OAuthToolkitService},
    bootstrap::{
        BootstrapInput, BootstrapOptions, BootstrapResolveError, ensure_kimi_home,
        resolve_bootstrap_options,
    },
};

pub struct TuiAgentCore {
    bootstrap: BootstrapOptions,
    oauth_toolkit: Arc<OAuthToolkitService>,
}

impl TuiAgentCore {
    pub fn bootstrap(client_version: impl Into<String>) -> Result<Self, TuiAgentCoreError> {
        let bootstrap = resolve_bootstrap_options(BootstrapInput {
            client_version: Some(client_version.into()),
            ..BootstrapInput::default()
        })?;
        Self::from_bootstrap_options(bootstrap)
    }

    fn from_bootstrap_options(bootstrap: BootstrapOptions) -> Result<Self, TuiAgentCoreError> {
        ensure_kimi_home(&bootstrap.home_dir)?;
        let oauth_toolkit = OAuthToolkitService::new(&bootstrap.home_dir)?;
        Ok(Self {
            bootstrap,
            oauth_toolkit: Arc::new(oauth_toolkit),
        })
    }

    pub fn oauth_toolkit(&self) -> &Arc<OAuthToolkitService> {
        &self.oauth_toolkit
    }

    pub fn bootstrap_options(&self) -> &BootstrapOptions {
        &self.bootstrap
    }
}

#[derive(Debug)]
pub enum TuiAgentCoreError {
    Bootstrap(BootstrapResolveError),
    EnsureHome(std::io::Error),
    OAuth(AuthOperationError),
}

impl fmt::Display for TuiAgentCoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Bootstrap(error) => error.fmt(formatter),
            Self::EnsureHome(error) => error.fmt(formatter),
            Self::OAuth(error) => error.fmt(formatter),
        }
    }
}

impl Error for TuiAgentCoreError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Bootstrap(error) => Some(error),
            Self::EnsureHome(error) => Some(error),
            Self::OAuth(error) => Some(error),
        }
    }
}

impl From<BootstrapResolveError> for TuiAgentCoreError {
    fn from(error: BootstrapResolveError) -> Self {
        Self::Bootstrap(error)
    }
}

impl From<std::io::Error> for TuiAgentCoreError {
    fn from(error: std::io::Error) -> Self {
        Self::EnsureHome(error)
    }
}

impl From<AuthOperationError> for TuiAgentCoreError {
    fn from(error: AuthOperationError) -> Self {
        Self::OAuth(error)
    }
}

#[cfg(test)]
mod tests {
    use std::{
        collections::HashMap,
        time::{SystemTime, UNIX_EPOCH},
    };

    use super::*;

    #[test]
    fn composition_creates_the_bootstrap_home_before_constructing_oauth() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let home_dir = std::env::temp_dir().join(format!("kimi-tui-agent-core-{unique}"));
        let agent_core = TuiAgentCore::from_bootstrap_options(BootstrapOptions {
            home_dir: home_dir.clone(),
            config_path: home_dir.join("config.toml"),
            os_home_dir: std::env::temp_dir(),
            platform: "linux".to_owned(),
            arch: "x64".to_owned(),
            cwd: std::env::temp_dir(),
            env: HashMap::new(),
            client_version: "test".to_owned(),
        })
        .expect("agent core composition");

        assert_eq!(agent_core.bootstrap_options().home_dir, home_dir);
        assert!(agent_core.bootstrap_options().home_dir.is_dir());
        let _ = std::fs::remove_dir_all(&agent_core.bootstrap_options().home_dir);
    }
}
