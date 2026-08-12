use std::{io, path::Path};

use kimi_code_fs::atomic_write;
use serde::{Deserialize, Serialize};
use tokio::fs;

use crate::DEFAULT_WEB_SERVER_PORT;

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum WebServerListenScope {
    #[default]
    Local,
    Global,
}

impl WebServerListenScope {
    pub(crate) fn bind_address(self) -> &'static str {
        match self {
            Self::Local => "127.0.0.1",
            Self::Global => "0.0.0.0",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WebServerSettings {
    pub enabled: bool,
    pub port: u16,
    #[serde(default)]
    pub listen_scope: WebServerListenScope,
}

impl Default for WebServerSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            port: DEFAULT_WEB_SERVER_PORT,
            listen_scope: WebServerListenScope::Local,
        }
    }
}

pub(crate) async fn load_settings(path: &Path) -> Result<WebServerSettings, String> {
    let bytes = match fs::read(path).await {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(WebServerSettings::default());
        }
        Err(error) => return Err(format!("failed to read {}: {error}", path.display())),
    };
    let settings: WebServerSettings = serde_json::from_slice(&bytes)
        .map_err(|error| format!("failed to parse {}: {error}", path.display()))?;
    validate_settings(settings)?;
    Ok(settings)
}

pub(crate) async fn save_settings(path: &Path, settings: WebServerSettings) -> Result<(), String> {
    validate_settings(settings)?;
    let bytes = serde_json::to_vec_pretty(&settings).map_err(|error| error.to_string())?;
    ensure_parent_directory(path, false).await?;
    atomic_write(path, &bytes, None)
        .await
        .map_err(|error| format!("failed to write {}: {error}", path.display()))
}

pub(crate) fn validate_settings(settings: WebServerSettings) -> Result<(), String> {
    if settings.port == 0 {
        return Err("web server port must be between 1 and 65535".into());
    }
    Ok(())
}

pub(crate) async fn load_or_create_token(path: &Path) -> Result<String, String> {
    match fs::read_to_string(path).await {
        Ok(token) if !token.trim().is_empty() => return Ok(token.trim().to_owned()),
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(format!("failed to read {}: {error}", path.display())),
    }

    use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
    let mut bytes = [0_u8; 32];
    getrandom::fill(&mut bytes).map_err(|error| error.to_string())?;
    let token = URL_SAFE_NO_PAD.encode(bytes);
    ensure_parent_directory(path, true).await?;
    atomic_write(path, token.as_bytes(), Some(0o600))
        .await
        .map_err(|error| format!("failed to write {}: {error}", path.display()))?;
    Ok(token)
}

async fn ensure_parent_directory(path: &Path, _private: bool) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "path has no parent directory"))
        .map_err(|error| format!("failed to write {}: {error}", path.display()))?;
    fs::create_dir_all(parent)
        .await
        .map_err(|error| format!("failed to write {}: {error}", path.display()))?;

    #[cfg(unix)]
    if _private {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))
            .await
            .map_err(|error| format!("failed to write {}: {error}", path.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use uuid::Uuid;

    fn temp_dir() -> PathBuf {
        std::env::temp_dir().join(format!("kimi-web-settings-{}", Uuid::new_v4()))
    }

    #[tokio::test]
    async fn settings_default_and_round_trip() {
        let directory = temp_dir();
        let path = directory.join("web-server.json");
        assert_eq!(
            load_settings(&path).await.unwrap(),
            WebServerSettings::default()
        );
        let expected = WebServerSettings {
            enabled: true,
            port: 61234,
            listen_scope: WebServerListenScope::Global,
        };
        save_settings(&path, expected).await.unwrap();
        assert_eq!(load_settings(&path).await.unwrap(), expected);

        fs::write(&path, br#"{"enabled":false,"port":58627}"#)
            .await
            .unwrap();
        assert_eq!(
            load_settings(&path).await.unwrap().listen_scope,
            WebServerListenScope::Local
        );
        let _ = fs::remove_dir_all(directory).await;
    }

    #[tokio::test]
    async fn token_is_persistent_and_url_safe() {
        let directory = temp_dir();
        let path = directory.join("server.token");
        let first = load_or_create_token(&path).await.unwrap();
        let second = load_or_create_token(&path).await.unwrap();
        assert_eq!(first, second);
        assert_eq!(first.len(), 43);
        assert!(
            first
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"-_".contains(&byte))
        );
        let _ = fs::remove_dir_all(directory).await;
    }
}
