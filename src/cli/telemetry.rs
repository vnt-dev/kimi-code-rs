use std::{error::Error, fmt, fs, io::Write, path::Path};

use crate::utils::paths::{HomeDirectoryUnavailable, get_data_dir};

pub const WEB_UI_MODE: &str = "web";
pub const KIMI_CODE_PROVIDER_NAME: &str = "managed:kimi-code";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CliTelemetryBootstrap {
    pub home_dir: std::path::PathBuf,
    pub device_id: String,
    pub first_launch: bool,
}

#[derive(Debug)]
pub struct CliTelemetryBootstrapError(HomeDirectoryUnavailable);

impl fmt::Display for CliTelemetryBootstrapError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl Error for CliTelemetryBootstrapError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(&self.0)
    }
}

// Original:
//   apps/kimi-code/src/cli/telemetry.ts
//   createCliTelemetryBootstrap()
pub fn create_cli_telemetry_bootstrap() -> Result<CliTelemetryBootstrap, CliTelemetryBootstrapError>
{
    let home_dir = get_data_dir().map_err(CliTelemetryBootstrapError)?;
    Ok(create_cli_telemetry_bootstrap_at(&home_dir))
}

pub fn create_cli_telemetry_bootstrap_at(home_dir: &Path) -> CliTelemetryBootstrap {
    let (device_id, first_launch) = create_kimi_device_id_at(home_dir);
    CliTelemetryBootstrap {
        home_dir: home_dir.to_path_buf(),
        device_id,
        first_launch,
    }
}

fn create_kimi_device_id_at(home_dir: &Path) -> (String, bool) {
    if let Some(device_id) = read_kimi_device_id_at(home_dir) {
        return (device_id, false);
    }

    let device_id = uuid::Uuid::new_v4().to_string();
    let _ = write_private_device_id(home_dir, &device_id);
    (device_id, true)
}

fn read_kimi_device_id_at(home_dir: &Path) -> Option<String> {
    let text = fs::read_to_string(home_dir.join("device_id")).ok()?;
    let device_id = text.trim();
    (!device_id.is_empty()).then(|| device_id.to_owned())
}

fn write_private_device_id(home_dir: &Path, device_id: &str) -> std::io::Result<()> {
    create_private_directory(home_dir)?;
    let file_path = home_dir.join("device_id");
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(file_path)?;
    file.write_all(device_id.as_bytes())
}

fn create_private_directory(home_dir: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    let already_existed = home_dir.exists();
    fs::create_dir_all(home_dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        if !already_existed {
            fs::set_permissions(home_dir, fs::Permissions::from_mode(0o700))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

    fn temp_home() -> std::path::PathBuf {
        let id = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "kimi-telemetry-bootstrap-{}-{id}",
            std::process::id()
        ))
    }

    #[test]
    fn first_bootstrap_persists_uuid_and_second_bootstrap_reuses_it() {
        let home = temp_home();
        let first = create_cli_telemetry_bootstrap_at(&home);
        assert!(first.first_launch);
        assert_eq!(first.home_dir, home);
        assert!(uuid::Uuid::parse_str(&first.device_id).is_ok());
        assert_eq!(
            fs::read_to_string(home.join("device_id")).expect("device id"),
            first.device_id
        );

        let second = create_cli_telemetry_bootstrap_at(&home);
        assert!(!second.first_launch);
        assert_eq!(second.device_id, first.device_id);
        fs::remove_dir_all(home).expect("cleanup");
    }

    #[test]
    fn existing_device_id_is_trimmed_without_rewriting_the_file() {
        let home = temp_home();
        fs::create_dir_all(&home).expect("home");
        fs::write(home.join("device_id"), "  existing-device\r\n").expect("device id");

        let bootstrap = create_cli_telemetry_bootstrap_at(&home);
        assert_eq!(bootstrap.device_id, "existing-device");
        assert!(!bootstrap.first_launch);
        assert_eq!(
            fs::read_to_string(home.join("device_id")).expect("unchanged"),
            "  existing-device\r\n"
        );
        fs::remove_dir_all(home).expect("cleanup");
    }

    #[test]
    fn blank_device_id_is_replaced_and_counted_as_first_launch() {
        let home = temp_home();
        fs::create_dir_all(&home).expect("home");
        fs::write(home.join("device_id"), " \n").expect("blank device id");

        let bootstrap = create_cli_telemetry_bootstrap_at(&home);
        assert!(bootstrap.first_launch);
        assert!(uuid::Uuid::parse_str(&bootstrap.device_id).is_ok());
        assert_eq!(
            fs::read_to_string(home.join("device_id")).expect("replacement"),
            bootstrap.device_id
        );
        fs::remove_dir_all(home).expect("cleanup");
    }

    #[cfg(unix)]
    #[test]
    fn new_bootstrap_uses_private_directory_and_file_modes() {
        use std::os::unix::fs::PermissionsExt;

        let home = temp_home();
        create_cli_telemetry_bootstrap_at(&home);
        assert_eq!(
            fs::metadata(&home)
                .expect("home metadata")
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(home.join("device_id"))
                .expect("device metadata")
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        fs::remove_dir_all(home).expect("cleanup");
    }
}
