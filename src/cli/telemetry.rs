use std::{error::Error, fmt, fs, future::Future, io::Write, path::Path, pin::Pin, sync::Arc};

use async_trait::async_trait;
use serde_json::{Map, Value};

use super::{prompt_session::PromptConfig, version::CLI_USER_AGENT_PRODUCT};
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

pub type TelemetryAccessTokenFuture =
    Pin<Box<dyn Future<Output = Result<Option<String>, CliTelemetryError>> + Send>>;
pub type TelemetryAccessTokenProvider = Arc<dyn Fn() -> TelemetryAccessTokenFuture + Send + Sync>;

#[derive(Debug)]
pub struct CliTelemetryError(Box<dyn Error + Send + Sync>);

impl CliTelemetryError {
    pub fn new(error: impl Error + Send + Sync + 'static) -> Self {
        Self(Box::new(error))
    }

    pub fn message(message: impl Into<String>) -> Self {
        Self(Box::new(CliTelemetryMessage(message.into())))
    }
}

impl fmt::Display for CliTelemetryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl Error for CliTelemetryError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(self.0.as_ref())
    }
}

#[derive(Debug)]
struct CliTelemetryMessage(String);

impl fmt::Display for CliTelemetryMessage {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl Error for CliTelemetryMessage {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InitializeCliTelemetryOptions {
    pub bootstrap: CliTelemetryBootstrap,
    pub config: PromptConfig,
    pub version: String,
    pub ui_mode: String,
    pub model: Option<String>,
    pub session_id: Option<String>,
}

pub struct TelemetryInitialization {
    pub home_dir: std::path::PathBuf,
    pub device_id: String,
    pub enabled: bool,
    pub app_name: String,
    pub version: String,
    pub ui_mode: String,
    pub model: Option<String>,
    pub session_id: Option<String>,
    pub get_access_token: TelemetryAccessTokenProvider,
}

#[async_trait]
pub trait CliTelemetryHarness: Send + Sync + 'static {
    fn home_dir(&self) -> &Path;
    fn track(&self, event: &str, properties: Option<&Map<String, Value>>);
    async fn get_cached_access_token(
        &self,
        provider_name: &str,
    ) -> Result<Option<String>, CliTelemetryError>;
}

pub trait CliTelemetryRuntime: Send + Sync {
    fn initialize_telemetry(
        &self,
        initialization: TelemetryInitialization,
    ) -> Result<(), CliTelemetryError>;
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

// Original:
//   apps/kimi-code/src/cli/telemetry.ts
//   initializeCliTelemetry()
pub fn initialize_cli_telemetry<H>(
    runtime: &dyn CliTelemetryRuntime,
    harness: Arc<H>,
    options: &InitializeCliTelemetryOptions,
) -> Result<(), CliTelemetryError>
where
    H: CliTelemetryHarness,
{
    let token_harness = Arc::clone(&harness);
    let get_access_token: TelemetryAccessTokenProvider = Arc::new(move || {
        let harness = Arc::clone(&token_harness);
        Box::pin(async move {
            harness
                .get_cached_access_token(KIMI_CODE_PROVIDER_NAME)
                .await
        })
    });
    runtime.initialize_telemetry(TelemetryInitialization {
        home_dir: harness.home_dir().to_path_buf(),
        device_id: options.bootstrap.device_id.clone(),
        enabled: options.config.telemetry,
        app_name: CLI_USER_AGENT_PRODUCT.to_owned(),
        version: options.version.clone(),
        ui_mode: options.ui_mode.clone(),
        model: options
            .model
            .clone()
            .or_else(|| options.config.default_model.clone()),
        session_id: options.session_id.clone(),
        get_access_token,
    })?;
    if options.bootstrap.first_launch {
        harness.track("first_launch", None);
    }
    Ok(())
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
    use std::sync::{
        Mutex,
        atomic::{AtomicU64, Ordering},
    };

    use super::*;

    static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

    struct HarnessMock {
        home_dir: std::path::PathBuf,
        token: Result<Option<String>, &'static str>,
        events: Mutex<Vec<String>>,
        token_providers: Mutex<Vec<String>>,
    }

    #[async_trait]
    impl CliTelemetryHarness for HarnessMock {
        fn home_dir(&self) -> &Path {
            &self.home_dir
        }

        fn track(&self, event: &str, _: Option<&Map<String, Value>>) {
            self.events.lock().expect("events").push(event.to_owned());
        }

        async fn get_cached_access_token(
            &self,
            provider_name: &str,
        ) -> Result<Option<String>, CliTelemetryError> {
            self.token_providers
                .lock()
                .expect("token providers")
                .push(provider_name.to_owned());
            self.token.clone().map_err(CliTelemetryError::message)
        }
    }

    #[derive(Default)]
    struct TelemetryRuntimeMock {
        initialization: Mutex<Option<TelemetryInitialization>>,
    }

    impl CliTelemetryRuntime for TelemetryRuntimeMock {
        fn initialize_telemetry(
            &self,
            initialization: TelemetryInitialization,
        ) -> Result<(), CliTelemetryError> {
            *self.initialization.lock().expect("initialization") = Some(initialization);
            Ok(())
        }
    }

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

    #[tokio::test]
    async fn initializes_cli_context_uses_explicit_model_and_tracks_first_launch_afterward() {
        let harness = Arc::new(HarnessMock {
            home_dir: std::path::PathBuf::from("/runtime-home"),
            token: Ok(Some("cached-token".to_owned())),
            events: Mutex::new(Vec::new()),
            token_providers: Mutex::new(Vec::new()),
        });
        let runtime = TelemetryRuntimeMock::default();
        let options = InitializeCliTelemetryOptions {
            bootstrap: CliTelemetryBootstrap {
                home_dir: std::path::PathBuf::from("/bootstrap-home"),
                device_id: "device-123".to_owned(),
                first_launch: true,
            },
            config: PromptConfig {
                default_model: Some("config-model".to_owned()),
                telemetry: false,
            },
            version: "1.2.3".to_owned(),
            ui_mode: "print".to_owned(),
            model: Some("cli-model".to_owned()),
            session_id: Some("session-1".to_owned()),
        };

        initialize_cli_telemetry(&runtime, harness.clone(), &options).expect("initialize");
        let initialization = runtime
            .initialization
            .lock()
            .expect("initialization")
            .take()
            .expect("captured initialization");
        assert_eq!(
            initialization.home_dir,
            std::path::PathBuf::from("/runtime-home")
        );
        assert_eq!(initialization.device_id, "device-123");
        assert!(!initialization.enabled);
        assert_eq!(initialization.app_name, "kimi-code-cli");
        assert_eq!(initialization.version, "1.2.3");
        assert_eq!(initialization.ui_mode, "print");
        assert_eq!(initialization.model.as_deref(), Some("cli-model"));
        assert_eq!(initialization.session_id.as_deref(), Some("session-1"));
        assert_eq!(
            (initialization.get_access_token)()
                .await
                .expect("access token")
                .as_deref(),
            Some("cached-token")
        );
        assert_eq!(
            harness
                .token_providers
                .lock()
                .expect("token providers")
                .as_slice(),
            ["managed:kimi-code"]
        );
        assert_eq!(
            harness.events.lock().expect("events").as_slice(),
            ["first_launch"]
        );
    }

    #[test]
    fn falls_back_to_config_model_and_does_not_repeat_first_launch() {
        let harness = Arc::new(HarnessMock {
            home_dir: std::path::PathBuf::from("/runtime-home"),
            token: Ok(None),
            events: Mutex::new(Vec::new()),
            token_providers: Mutex::new(Vec::new()),
        });
        let runtime = TelemetryRuntimeMock::default();
        let options = InitializeCliTelemetryOptions {
            bootstrap: CliTelemetryBootstrap {
                home_dir: std::path::PathBuf::from("/bootstrap-home"),
                device_id: "device-123".to_owned(),
                first_launch: false,
            },
            config: PromptConfig {
                default_model: Some("config-model".to_owned()),
                telemetry: true,
            },
            version: "1.2.3".to_owned(),
            ui_mode: "shell".to_owned(),
            model: None,
            session_id: None,
        };

        initialize_cli_telemetry(&runtime, harness.clone(), &options).expect("initialize");
        let guard = runtime.initialization.lock().expect("initialization");
        let initialization = guard.as_ref().expect("captured initialization");
        assert_eq!(initialization.model.as_deref(), Some("config-model"));
        assert!(initialization.enabled);
        assert!(harness.events.lock().expect("events").is_empty());
    }
}
