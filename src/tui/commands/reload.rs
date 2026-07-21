use std::{error::Error, fmt, future::Future};

use crate::tui::config::{TuiConfig, TuiConfigIoError, load_default_tui_config};

pub trait ReloadTuiCommandHost {
    type Error: Error + Send + Sync + 'static;

    fn current_theme_is_light(&self) -> bool;
    fn apply_theme(
        &mut self,
        theme: &str,
        resolved_auto_theme: Option<&str>,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send;
    fn refresh_terminal_theme_tracking(&mut self);
    fn update_tui_config_state(&mut self, config: &TuiConfig);
    fn set_editor_disable_paste_burst(&mut self, disabled: bool);
    fn show_success(&mut self, message: &str);
}

#[derive(Debug)]
pub enum ReloadTuiCommandError<E> {
    Config(TuiConfigIoError),
    Host(E),
}

impl<E: fmt::Display> fmt::Display for ReloadTuiCommandError<E> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Config(error) => error.fmt(formatter),
            Self::Host(error) => error.fmt(formatter),
        }
    }
}

impl<E: Error + 'static> Error for ReloadTuiCommandError<E> {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Config(error) => Some(error),
            Self::Host(error) => Some(error),
        }
    }
}

/// Original:
///   apps/kimi-code/src/tui/commands/reload.ts
///   handleReloadTuiCommand()
pub async fn handle_reload_tui_command<H: ReloadTuiCommandHost>(
    host: &mut H,
) -> Result<(), ReloadTuiCommandError<H::Error>> {
    let config = load_default_tui_config()
        .await
        .map_err(ReloadTuiCommandError::Config)?;
    apply_reloaded_tui_config(host, &config).await?;
    host.show_success("TUI config reloaded.");
    Ok(())
}

/// Theme application is awaited before terminal tracking is refreshed so the
/// tracking query observes the newly selected palette.
///
/// Original:
///   apps/kimi-code/src/tui/commands/reload.ts
///   applyReloadedTuiConfig()
pub async fn apply_reloaded_tui_config<H: ReloadTuiCommandHost>(
    host: &mut H,
    config: &TuiConfig,
) -> Result<(), ReloadTuiCommandError<H::Error>> {
    let resolved = (config.theme == "auto").then(|| {
        if host.current_theme_is_light() {
            "light"
        } else {
            "dark"
        }
    });
    host.apply_theme(&config.theme, resolved)
        .await
        .map_err(ReloadTuiCommandError::Host)?;
    host.refresh_terminal_theme_tracking();
    host.update_tui_config_state(config);
    host.set_editor_disable_paste_burst(config.disable_paste_burst);
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::io;

    use crate::tui::config::{NotificationCondition, NotificationsConfig, UpgradePreferences};

    use super::*;

    struct Host {
        light: bool,
        fail_theme: bool,
        operations: Vec<String>,
    }

    impl ReloadTuiCommandHost for Host {
        type Error = io::Error;

        fn current_theme_is_light(&self) -> bool {
            self.light
        }

        async fn apply_theme(
            &mut self,
            theme: &str,
            resolved_auto_theme: Option<&str>,
        ) -> Result<(), Self::Error> {
            self.operations
                .push(format!("theme:{theme}:{resolved_auto_theme:?}"));
            if self.fail_theme {
                Err(io::Error::other("theme unavailable"))
            } else {
                // A yield makes the ordering assertion cover the await boundary.
                tokio::task::yield_now().await;
                self.operations.push("theme_applied".to_owned());
                Ok(())
            }
        }

        fn refresh_terminal_theme_tracking(&mut self) {
            self.operations.push("tracking".to_owned());
        }

        fn update_tui_config_state(&mut self, config: &TuiConfig) {
            self.operations.push(format!(
                "state:{}:{:?}:{}:{}",
                config.editor_command.as_deref().unwrap_or(""),
                config.notifications.condition,
                config.notifications.enabled,
                config.upgrade.auto_install
            ));
        }

        fn set_editor_disable_paste_burst(&mut self, disabled: bool) {
            self.operations.push(format!("paste:{disabled}"));
        }

        fn show_success(&mut self, message: &str) {
            self.operations.push(format!("success:{message}"));
        }
    }

    fn config(theme: &str) -> TuiConfig {
        TuiConfig {
            theme: theme.to_owned(),
            disable_paste_burst: true,
            editor_command: Some("vim".to_owned()),
            notifications: NotificationsConfig {
                enabled: false,
                condition: NotificationCondition::Always,
            },
            upgrade: UpgradePreferences {
                auto_install: false,
            },
        }
    }

    #[tokio::test]
    async fn applies_theme_before_tracking_and_updates_every_tui_field() {
        let mut host = Host {
            light: true,
            fail_theme: false,
            operations: Vec::new(),
        };
        apply_reloaded_tui_config(&mut host, &config("auto"))
            .await
            .expect("apply config");
        assert_eq!(
            host.operations,
            [
                "theme:auto:Some(\"light\")",
                "theme_applied",
                "tracking",
                "state:vim:Always:false:false",
                "paste:true",
            ]
        );
    }

    #[tokio::test]
    async fn explicit_theme_has_no_auto_resolution_and_failure_stops_updates() {
        let mut explicit = Host {
            light: false,
            fail_theme: false,
            operations: Vec::new(),
        };
        apply_reloaded_tui_config(&mut explicit, &config("dark"))
            .await
            .expect("apply config");
        assert_eq!(explicit.operations[0], "theme:dark:None");

        let mut failed = Host {
            light: false,
            fail_theme: true,
            operations: Vec::new(),
        };
        let error = apply_reloaded_tui_config(&mut failed, &config("auto"))
            .await
            .expect_err("theme failure");
        assert_eq!(error.to_string(), "theme unavailable");
        assert_eq!(failed.operations, ["theme:auto:Some(\"dark\")"]);
    }
}
