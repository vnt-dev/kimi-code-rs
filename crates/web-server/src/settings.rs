use std::{
    fs::{self, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::DEFAULT_WEB_SERVER_PORT;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WebServerSettings {
    pub enabled: bool,
    pub port: u16,
}

impl Default for WebServerSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            port: DEFAULT_WEB_SERVER_PORT,
        }
    }
}

pub(crate) fn load_settings(path: &Path) -> Result<WebServerSettings, String> {
    let bytes = match fs::read(path) {
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

pub(crate) fn save_settings(path: &Path, settings: WebServerSettings) -> Result<(), String> {
    validate_settings(settings)?;
    let bytes = serde_json::to_vec_pretty(&settings).map_err(|error| error.to_string())?;
    atomic_write(path, &bytes, false)
        .map_err(|error| format!("failed to write {}: {error}", path.display()))
}

pub(crate) fn validate_settings(settings: WebServerSettings) -> Result<(), String> {
    if settings.port == 0 {
        return Err("web server port must be between 1 and 65535".into());
    }
    Ok(())
}

pub(crate) fn load_or_create_token(path: &Path) -> Result<String, String> {
    match fs::read_to_string(path) {
        Ok(token) if !token.trim().is_empty() => return Ok(token.trim().to_owned()),
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(format!("failed to read {}: {error}", path.display())),
    }

    use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
    let mut bytes = [0_u8; 32];
    getrandom::fill(&mut bytes).map_err(|error| error.to_string())?;
    let token = URL_SAFE_NO_PAD.encode(bytes);
    atomic_write(path, token.as_bytes(), true)
        .map_err(|error| format!("failed to write {}: {error}", path.display()))?;
    Ok(token)
}

fn atomic_write(path: &Path, bytes: &[u8], _private: bool) -> io::Result<()> {
    let parent = path.parent().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "path has no parent directory")
    })?;
    fs::create_dir_all(parent)?;

    #[cfg(unix)]
    if _private {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
    }

    let temp = temporary_path(path);
    let mut options = OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    if _private {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&temp)?;
    let write_result = (|| {
        file.write_all(bytes)?;
        file.sync_all()?;
        replace_file(&temp, path)
    })();
    if write_result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    write_result
}

fn temporary_path(path: &Path) -> PathBuf {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("web-server");
    path.with_file_name(format!(".{name}.{}.tmp", Uuid::new_v4()))
}

#[cfg(not(windows))]
fn replace_file(source: &Path, destination: &Path) -> io::Result<()> {
    fs::rename(source, destination)
}

#[cfg(windows)]
fn replace_file(source: &Path, destination: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
    };

    let source: Vec<u16> = source.as_os_str().encode_wide().chain(Some(0)).collect();
    let destination: Vec<u16> = destination
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();
    let result = unsafe {
        MoveFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if result == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir() -> PathBuf {
        std::env::temp_dir().join(format!("kimi-web-settings-{}", Uuid::new_v4()))
    }

    #[test]
    fn settings_default_and_round_trip() {
        let directory = temp_dir();
        let path = directory.join("web-server.json");
        assert_eq!(load_settings(&path).unwrap(), WebServerSettings::default());
        let expected = WebServerSettings {
            enabled: true,
            port: 61234,
        };
        save_settings(&path, expected).unwrap();
        assert_eq!(load_settings(&path).unwrap(), expected);
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn token_is_persistent_and_url_safe() {
        let directory = temp_dir();
        let path = directory.join("server.token");
        let first = load_or_create_token(&path).unwrap();
        let second = load_or_create_token(&path).unwrap();
        assert_eq!(first, second);
        assert_eq!(first.len(), 43);
        assert!(
            first
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"-_".contains(&byte))
        );
        let _ = fs::remove_dir_all(directory);
    }
}
