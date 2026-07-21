use std::{
    error::Error,
    fmt, io,
    path::{Path, PathBuf},
};

use serde::Deserialize;
use tokio::fs;

use crate::utils::paths::{HomeDirectoryUnavailable, get_data_dir};

pub const INVALID_TUI_CONFIG_MESSAGE: &str =
    "Invalid TUI config in ~/.kimi-code/tui.toml; using defaults.";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NotificationCondition {
    Unfocused,
    Always,
}

impl NotificationCondition {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unfocused => "unfocused",
            Self::Always => "always",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotificationsConfig {
    pub enabled: bool,
    pub condition: NotificationCondition,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpgradePreferences {
    pub auto_install: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TuiConfig {
    pub theme: String,
    pub disable_paste_burst: bool,
    pub editor_command: Option<String>,
    pub notifications: NotificationsConfig,
    pub upgrade: UpgradePreferences,
}

impl Default for TuiConfig {
    fn default() -> Self {
        Self {
            theme: "auto".to_owned(),
            disable_paste_burst: false,
            editor_command: None,
            notifications: NotificationsConfig {
                enabled: true,
                condition: NotificationCondition::Unfocused,
            },
            upgrade: UpgradePreferences { auto_install: true },
        }
    }
}

#[derive(Debug, Default, Deserialize)]
struct TuiConfigFile {
    theme: Option<String>,
    disable_paste_burst: Option<bool>,
    editor: Option<EditorConfigFile>,
    notifications: Option<NotificationsConfigFile>,
    upgrade: Option<UpgradePreferencesFile>,
}

#[derive(Debug, Default, Deserialize)]
struct EditorConfigFile {
    command: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct NotificationsConfigFile {
    enabled: Option<bool>,
    notification_condition: Option<NotificationCondition>,
}

#[derive(Debug, Default, Deserialize)]
struct UpgradePreferencesFile {
    auto_install: Option<bool>,
}

#[derive(Debug)]
pub struct TuiConfigValidationError(toml::de::Error);

impl fmt::Display for TuiConfigValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl Error for TuiConfigValidationError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(&self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TuiConfigParseError {
    pub fallback: TuiConfig,
}

impl fmt::Display for TuiConfigParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(INVALID_TUI_CONFIG_MESSAGE)
    }
}

impl Error for TuiConfigParseError {}

#[derive(Debug)]
pub enum TuiConfigIoError {
    Home(HomeDirectoryUnavailable),
    Io(io::Error),
    Parse(TuiConfigParseError),
}

impl fmt::Display for TuiConfigIoError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Home(error) => error.fmt(formatter),
            Self::Io(error) => error.fmt(formatter),
            Self::Parse(error) => error.fmt(formatter),
        }
    }
}

impl Error for TuiConfigIoError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Home(error) => Some(error),
            Self::Io(error) => Some(error),
            Self::Parse(error) => Some(error),
        }
    }
}

impl From<HomeDirectoryUnavailable> for TuiConfigIoError {
    fn from(error: HomeDirectoryUnavailable) -> Self {
        Self::Home(error)
    }
}

impl From<io::Error> for TuiConfigIoError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

// Original:
//   apps/kimi-code/src/tui/config.ts
//   getTuiConfigPath()
pub fn get_tui_config_path() -> Result<PathBuf, HomeDirectoryUnavailable> {
    Ok(get_data_dir()?.join("tui.toml"))
}

pub async fn load_default_tui_config() -> Result<TuiConfig, TuiConfigIoError> {
    load_tui_config(&get_tui_config_path()?).await
}

// Original:
//   apps/kimi-code/src/tui/config.ts
//   loadTuiConfig()
pub async fn load_tui_config(file_path: &Path) -> Result<TuiConfig, TuiConfigIoError> {
    match fs::metadata(file_path).await {
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            let config = TuiConfig::default();
            save_tui_config(&config, file_path).await?;
            return Ok(config);
        }
        Err(error) => return Err(TuiConfigIoError::Io(error)),
    }

    let text = fs::read_to_string(file_path)
        .await
        .map_err(|_| invalid_config_error())?;
    parse_tui_config(&text).map_err(|_| invalid_config_error())
}

fn invalid_config_error() -> TuiConfigIoError {
    TuiConfigIoError::Parse(TuiConfigParseError {
        fallback: TuiConfig::default(),
    })
}

// Original:
//   apps/kimi-code/src/tui/config.ts
//   parseTuiConfig(), normalizeTuiConfig()
pub fn parse_tui_config(toml_text: &str) -> Result<TuiConfig, TuiConfigValidationError> {
    if toml_text.trim().is_empty() {
        return Ok(TuiConfig::default());
    }
    let raw = toml::from_str::<TuiConfigFile>(toml_text).map_err(TuiConfigValidationError)?;
    Ok(normalize_tui_config(raw))
}

fn normalize_tui_config(raw: TuiConfigFile) -> TuiConfig {
    let defaults = TuiConfig::default();
    let editor_command = raw
        .editor
        .and_then(|editor| editor.command)
        .map(|command| command.trim().to_owned())
        .filter(|command| !command.is_empty());
    let notifications = raw.notifications.unwrap_or_default();
    let upgrade = raw.upgrade.unwrap_or_default();
    TuiConfig {
        theme: raw.theme.unwrap_or(defaults.theme),
        disable_paste_burst: raw
            .disable_paste_burst
            .unwrap_or(defaults.disable_paste_burst),
        editor_command,
        notifications: NotificationsConfig {
            enabled: notifications
                .enabled
                .unwrap_or(defaults.notifications.enabled),
            condition: notifications
                .notification_condition
                .unwrap_or(defaults.notifications.condition),
        },
        upgrade: UpgradePreferences {
            auto_install: upgrade
                .auto_install
                .unwrap_or(defaults.upgrade.auto_install),
        },
    }
}

// Original:
//   apps/kimi-code/src/tui/config.ts
//   saveTuiConfig()
pub async fn save_tui_config(config: &TuiConfig, file_path: &Path) -> Result<(), TuiConfigIoError> {
    if let Some(parent) = file_path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent).await?;
    }
    fs::write(file_path, render_tui_config(config)).await?;
    Ok(())
}

pub async fn save_default_tui_config(config: &TuiConfig) -> Result<(), TuiConfigIoError> {
    save_tui_config(config, &get_tui_config_path()?).await
}

// Original:
//   apps/kimi-code/src/tui/config.ts
//   renderTuiConfig()
pub fn render_tui_config(config: &TuiConfig) -> String {
    format!(
        "# ~/.kimi-code/tui.toml\n\
# Client preferences for kimi-code.\n\
# Agent/runtime settings stay in ~/.kimi-code/config.toml.\n\
\n\
theme = \"{}\" # \"auto\" | \"dark\" | \"light\" | custom theme name\n\
disable_paste_burst = {} # true disables non-bracketed paste-burst fallback\n\
\n\
[editor]\n\
command = \"{}\" # Empty uses $VISUAL / $EDITOR\n\
\n\
[notifications]\n\
enabled = {} # true | false\n\
notification_condition = \"{}\" # \"unfocused\" | \"always\"\n\
\n\
[upgrade]\n\
auto_install = {} # true | false\n",
        escape_toml_basic_string(&config.theme),
        config.disable_paste_burst,
        escape_toml_basic_string(config.editor_command.as_deref().unwrap_or("")),
        config.notifications.enabled,
        config.notifications.condition.as_str(),
        config.upgrade.auto_install,
    )
}

fn escape_toml_basic_string(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\u{0008}', "\\b")
        .replace('\t', "\\t")
        .replace('\n', "\\n")
        .replace('\u{000c}', "\\f")
        .replace('\r', "\\r")
}

#[cfg(test)]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

    fn temp_path() -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        std::env::temp_dir()
            .join(format!("kimi-tui-config-{unique}"))
            .join("tui.toml")
    }

    #[tokio::test]
    async fn creates_the_default_config_when_missing() {
        let path = temp_path();
        let config = load_tui_config(&path).await.expect("default config");
        assert_eq!(config, TuiConfig::default());
        let text = fs::read_to_string(&path).await.expect("saved config");
        assert!(text.contains("Client preferences for kimi-code."));
        assert!(text.contains("theme = \"auto\""));
        assert!(text.contains("notification_condition = \"unfocused\""));
        fs::remove_dir_all(path.parent().expect("parent"))
            .await
            .expect("cleanup");
    }

    #[test]
    fn parses_and_normalizes_valid_toml() {
        let config = parse_tui_config(
            r#"
theme = "light"
disable_paste_burst = true

[editor]
command = "code --wait"

[notifications]
enabled = false
notification_condition = "always"

[upgrade]
auto_install = false
"#,
        )
        .expect("valid config");
        assert_eq!(
            config,
            TuiConfig {
                theme: "light".to_owned(),
                disable_paste_burst: true,
                editor_command: Some("code --wait".to_owned()),
                notifications: NotificationsConfig {
                    enabled: false,
                    condition: NotificationCondition::Always,
                },
                upgrade: UpgradePreferences {
                    auto_install: false
                },
            }
        );
    }

    #[test]
    fn empty_text_and_blank_editor_use_defaults() {
        assert_eq!(
            parse_tui_config(" \n").expect("empty"),
            TuiConfig::default()
        );
        assert_eq!(
            parse_tui_config("[editor]\ncommand = \"   \"\n")
                .expect("blank editor")
                .editor_command,
            None
        );
    }

    #[test]
    fn rejects_wrong_types_and_notification_conditions() {
        assert!(parse_tui_config("disable_paste_burst = \"yes\"").is_err());
        assert!(
            parse_tui_config("[notifications]\nnotification_condition = \"sometimes\"").is_err()
        );
    }

    #[tokio::test]
    async fn malformed_file_returns_parse_error_with_fallback_without_overwriting() {
        let path = temp_path();
        fs::create_dir_all(path.parent().expect("parent"))
            .await
            .expect("directory");
        fs::write(&path, "[[[").await.expect("broken config");

        let error = load_tui_config(&path).await.expect_err("parse failure");
        let TuiConfigIoError::Parse(error) = error else {
            panic!("expected parse error");
        };
        assert_eq!(error.to_string(), INVALID_TUI_CONFIG_MESSAGE);
        assert_eq!(error.fallback, TuiConfig::default());
        assert_eq!(fs::read_to_string(&path).await.expect("untouched"), "[[[");
        fs::remove_dir_all(path.parent().expect("parent"))
            .await
            .expect("cleanup");
    }

    #[tokio::test]
    async fn saves_reloads_and_escapes_special_characters() {
        let path = temp_path();
        let config = TuiConfig {
            theme: "weird\"name\\with-quote".to_owned(),
            disable_paste_burst: false,
            editor_command: Some("vim\n--clean".to_owned()),
            notifications: NotificationsConfig {
                enabled: false,
                condition: NotificationCondition::Always,
            },
            upgrade: UpgradePreferences {
                auto_install: false,
            },
        };
        save_tui_config(&config, &path).await.expect("save");
        assert_eq!(load_tui_config(&path).await.expect("reload"), config);
        fs::remove_dir_all(path.parent().expect("parent"))
            .await
            .expect("cleanup");
    }
}
