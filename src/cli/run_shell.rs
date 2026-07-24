use std::{error::Error, fmt, path::Path};

use crate::tui::config::{TuiConfig, TuiConfigIoError, get_tui_config_path, load_tui_config};

use super::options::CliOptions;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RunShellOptions {
    pub migrate_only: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellStartupConfig {
    pub tui_config: TuiConfig,
    pub config_warning: Option<String>,
}

#[derive(Debug)]
pub enum RunShellError {
    ConfigPath(crate::utils::paths::HomeDirectoryUnavailable),
    Config(TuiConfigIoError),
}

impl fmt::Display for RunShellError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ConfigPath(error) => error.fmt(formatter),
            Self::Config(error) => error.fmt(formatter),
        }
    }
}

impl Error for RunShellError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::ConfigPath(error) => Some(error),
            Self::Config(error) => Some(error),
        }
    }
}

// Original:
//   apps/kimi-code/src/cli/run-shell.ts
//   runShell() config-loading block
//
// Rust adaptation:
//   The file path is injectable so the fallback behavior can be verified
//   without mutating the user's home directory. Parse failures remain
//   recoverable and become the same startup notice used by the TUI.
pub async fn load_shell_startup_config(
    file_path: &Path,
) -> Result<ShellStartupConfig, RunShellError> {
    match load_tui_config(file_path).await {
        Ok(tui_config) => Ok(ShellStartupConfig {
            tui_config,
            config_warning: None,
        }),
        Err(TuiConfigIoError::Parse(error)) => {
            let config_warning = error.to_string();
            Ok(ShellStartupConfig {
                tui_config: error.fallback,
                config_warning: Some(config_warning),
            })
        }
        Err(error) => Err(RunShellError::Config(error)),
    }
}

// Original:
//   apps/kimi-code/src/cli/run-shell.ts
//   runShell()
//
// Rust adaptation:
//   Configuration I/O is asynchronous. The remaining harness and TUI
//   composition must target kimi-code-agent-core-v2 directly.
pub async fn run_shell(
    options: &CliOptions,
    version: &str,
    run_options: RunShellOptions,
) -> Result<(), RunShellError> {
    let config_path = get_tui_config_path().map_err(RunShellError::ConfigPath)?;
    let startup = load_shell_startup_config(&config_path).await?;

    start_v2_shell(options, version, run_options, startup).await
}

async fn start_v2_shell(
    _options: &CliOptions,
    _version: &str,
    _run_options: RunShellOptions,
    _startup: ShellStartupConfig,
) -> Result<(), RunShellError> {
    // MIGRATION-TODO:
    // Original: apps/kimi-code/src/cli/run-shell.ts, runShell() after
    // loadTuiConfig().
    // Missing dependency: kimi-code-agent-core-v2 does not yet expose the
    // application-level harness composition used to create a session-backed
    // KimiTUI.
    // Temporary behavior: reaching the interactive shell reports an explicit
    // incomplete migration through todo!().
    // Completion condition: compose the v2 services and pass their session
    // facade to the migrated KimiTUI without importing the legacy agent-core.
    todo!("compose runShell with kimi-code-agent-core-v2 and KimiTUI")
}

#[cfg(test)]
mod tests {
    use std::{
        path::PathBuf,
        time::{SystemTime, UNIX_EPOCH},
    };

    use tokio::fs;

    use super::*;
    use crate::tui::config::{INVALID_TUI_CONFIG_MESSAGE, TuiConfig};

    fn temp_config_path() -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        std::env::temp_dir()
            .join(format!("kimi-run-shell-{unique}"))
            .join("tui.toml")
    }

    #[tokio::test]
    async fn loads_valid_tui_config_without_warning() {
        let path = temp_config_path();
        fs::create_dir_all(path.parent().expect("parent"))
            .await
            .expect("directory");
        fs::write(&path, "theme = \"light\"\ndisable_paste_burst = true\n")
            .await
            .expect("config");

        let startup = load_shell_startup_config(&path)
            .await
            .expect("startup config");

        assert_eq!(startup.tui_config.theme, "light");
        assert!(startup.tui_config.disable_paste_burst);
        assert_eq!(startup.config_warning, None);
        fs::remove_dir_all(path.parent().expect("parent"))
            .await
            .expect("cleanup");
    }

    #[tokio::test]
    async fn malformed_tui_config_uses_defaults_and_preserves_warning() {
        let path = temp_config_path();
        fs::create_dir_all(path.parent().expect("parent"))
            .await
            .expect("directory");
        fs::write(&path, "[[[").await.expect("malformed config");

        let startup = load_shell_startup_config(&path)
            .await
            .expect("fallback config");

        assert_eq!(startup.tui_config, TuiConfig::default());
        assert_eq!(
            startup.config_warning.as_deref(),
            Some(INVALID_TUI_CONFIG_MESSAGE)
        );
        fs::remove_dir_all(path.parent().expect("parent"))
            .await
            .expect("cleanup");
    }
}
