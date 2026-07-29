use std::{
    collections::HashMap,
    error::Error,
    fmt, fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

#[cfg(windows)]
use std::os::windows::process::CommandExt;

use indexmap::IndexMap;

pub const KIMI_CODE_PLATFORM: &str = "kimi_code_cli";
pub const KIMI_CODE_CUSTOM_HEADERS_ENV: &str = "KIMI_CODE_CUSTOM_HEADERS";

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KimiHostIdentity {
    pub user_agent_product: String,
    pub version: String,
    pub user_agent_suffix: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KimiIdentityOptions {
    pub home_dir: PathBuf,
    pub host: KimiHostIdentity,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceSystemInfo {
    pub hostname: String,
    pub os_type: String,
    pub os_release: String,
    pub architecture: String,
    pub macos_product_version: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreatedDeviceId {
    pub id: String,
    pub first_launch: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdentityError {
    message: String,
}

impl IdentityError {
    fn required(field_name: &str) -> Self {
        Self {
            message: format!("{field_name} must be a non-empty ASCII string."),
        }
    }

    fn missing() -> Self {
        Self {
            message: "Kimi host identity is required. Pass the host product name and version."
                .to_owned(),
        }
    }
}

impl fmt::Display for IdentityError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for IdentityError {}

// Original:
//   packages/oauth/src/identity.ts
//   readKimiDeviceId()
pub fn read_kimi_device_id(home_dir: &Path) -> Option<String> {
    let text = fs::read_to_string(home_dir.join("device_id")).ok()?;
    let device_id = text.trim();
    (!device_id.is_empty()).then(|| device_id.to_owned())
}

// Original:
//   packages/oauth/src/identity.ts
//   createKimiDeviceId()
pub fn create_kimi_device_id(home_dir: &Path) -> String {
    create_kimi_device_id_with_status(home_dir).id
}

pub fn create_kimi_device_id_with_status(home_dir: &Path) -> CreatedDeviceId {
    if let Some(id) = read_kimi_device_id(home_dir) {
        return CreatedDeviceId {
            id,
            first_launch: false,
        };
    }

    let id = uuid::Uuid::new_v4().to_string();
    let _ = write_private_device_id(home_dir, &id);
    CreatedDeviceId {
        id,
        first_launch: true,
    }
}

// Original: createKimiDeviceHeaders()
pub fn create_kimi_device_headers(
    home_dir: &Path,
    version: &str,
) -> Result<IndexMap<String, String>, IdentityError> {
    create_kimi_device_headers_with_info(home_dir, version, &system_info())
}

pub fn create_kimi_device_headers_with_info(
    home_dir: &Path,
    version: &str,
    system: &DeviceSystemInfo,
) -> Result<IndexMap<String, String>, IdentityError> {
    Ok(IndexMap::from([
        ("X-Msh-Platform".to_owned(), KIMI_CODE_PLATFORM.to_owned()),
        (
            "X-Msh-Version".to_owned(),
            required_ascii_header(version, "Kimi identity version")?,
        ),
        (
            "X-Msh-Device-Name".to_owned(),
            ascii_header(&system.hostname, "unknown"),
        ),
        (
            "X-Msh-Device-Model".to_owned(),
            ascii_header(&device_model(system), "unknown"),
        ),
        (
            "X-Msh-Os-Version".to_owned(),
            ascii_header(&system.os_release, "unknown"),
        ),
        (
            "X-Msh-Device-Id".to_owned(),
            create_kimi_device_id(home_dir),
        ),
    ]))
}

// Original: createKimiUserAgent()
pub fn create_kimi_user_agent(identity: &KimiHostIdentity) -> Result<String, IdentityError> {
    let product = required_ascii_header(&identity.user_agent_product, "Kimi identity product")?;
    let version = required_ascii_header(&identity.version, "Kimi identity version")?;
    let suffix = identity
        .user_agent_suffix
        .as_deref()
        .map(|suffix| ascii_header(suffix, ""));
    Ok(match suffix.as_deref() {
        Some(suffix) if !suffix.is_empty() => format!("{product}/{version} ({suffix})"),
        _ => format!("{product}/{version}"),
    })
}

// Original: createKimiDefaultHeaders()
pub fn create_kimi_default_headers(
    options: &KimiIdentityOptions,
) -> Result<IndexMap<String, String>, IdentityError> {
    let mut headers = IndexMap::from([(
        "User-Agent".to_owned(),
        create_kimi_user_agent(&options.host)?,
    )]);
    headers.extend(create_kimi_device_headers(
        &options.home_dir,
        &options.host.version,
    )?);
    Ok(headers)
}

// Original: parseKimiCodeCustomHeaders()
pub fn parse_kimi_code_custom_headers(
    environment: &HashMap<String, String>,
) -> IndexMap<String, String> {
    let Some(raw) = environment
        .get(KIMI_CODE_CUSTOM_HEADERS_ENV)
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
    else {
        return IndexMap::new();
    };
    let mut headers = IndexMap::new();
    for line in raw.split('\n') {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        let name = name.trim();
        if !name.is_empty() {
            headers.insert(name.to_owned(), value.trim().to_owned());
        }
    }
    headers
}

// Original: assertKimiHostIdentity()
pub fn assert_kimi_host_identity(
    identity: Option<&KimiHostIdentity>,
) -> Result<&KimiHostIdentity, IdentityError> {
    let identity = identity.ok_or_else(IdentityError::missing)?;
    required_ascii_header(&identity.user_agent_product, "Kimi identity product")?;
    required_ascii_header(&identity.version, "Kimi identity version")?;
    Ok(identity)
}

fn ascii_header(value: &str, fallback: &str) -> String {
    let cleaned = value
        .chars()
        .filter(|character| matches!(*character as u32, 0x20..=0x7e))
        .collect::<String>();
    let cleaned = cleaned.trim();
    if cleaned.is_empty() {
        fallback.to_owned()
    } else {
        cleaned.to_owned()
    }
}

fn required_ascii_header(value: &str, field_name: &str) -> Result<String, IdentityError> {
    let cleaned = ascii_header(value, "");
    if cleaned.is_empty() {
        Err(IdentityError::required(field_name))
    } else {
        Ok(cleaned)
    }
}

fn device_model(system: &DeviceSystemInfo) -> String {
    match system.os_type.as_str() {
        "Darwin" => format!(
            "macOS {} {}",
            system
                .macos_product_version
                .as_deref()
                .unwrap_or(&system.os_release),
            system.architecture
        ),
        "Windows_NT" => format!("Windows {} {}", system.os_release, system.architecture),
        _ => format!(
            "{} {} {}",
            system.os_type, system.os_release, system.architecture
        )
        .trim()
        .to_owned(),
    }
}

fn system_info() -> DeviceSystemInfo {
    DeviceSystemInfo {
        hostname: system_hostname(),
        os_type: system_os_type().to_owned(),
        os_release: system_release(),
        architecture: node_architecture().to_owned(),
        macos_product_version: cfg!(target_os = "macos")
            .then(|| command_stdout_with_timeout("/usr/bin/sw_vers", &["-productVersion"]))
            .flatten(),
    }
}

fn system_hostname() -> String {
    #[cfg(windows)]
    let environment_name = std::env::var("COMPUTERNAME").ok();
    #[cfg(not(windows))]
    let environment_name = std::env::var("HOSTNAME").ok();

    environment_name
        .filter(|value| !value.trim().is_empty())
        .or_else(|| command_stdout_with_timeout("hostname", &[]))
        .or_else(|| fs::read_to_string("/etc/hostname").ok())
        .unwrap_or_else(|| "unknown".to_owned())
}

const fn system_os_type() -> &'static str {
    if cfg!(target_os = "windows") {
        "Windows_NT"
    } else if cfg!(target_os = "macos") {
        "Darwin"
    } else if cfg!(target_os = "linux") {
        "Linux"
    } else {
        std::env::consts::OS
    }
}

fn system_release() -> String {
    #[cfg(windows)]
    let output = command_stdout_with_timeout("cmd.exe", &["/D", "/C", "ver"])
        .and_then(|output| windows_release_from_ver(&output));
    #[cfg(not(windows))]
    let output = command_stdout_with_timeout("uname", &["-r"]);

    output.unwrap_or_else(|| "unknown".to_owned())
}

#[cfg(windows)]
fn windows_release_from_ver(output: &str) -> Option<String> {
    let bracketed = output.rsplit_once('[')?.1.trim_end_matches(']').trim();
    let version = bracketed.split_whitespace().next_back()?;
    (!version.is_empty()).then(|| version.to_owned())
}

fn node_architecture() -> &'static str {
    match std::env::consts::ARCH {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        "x86" => "ia32",
        architecture => architecture,
    }
}

fn command_stdout_with_timeout(command: &str, arguments: &[&str]) -> Option<String> {
    let mut process = Command::new(command);
    process
        .args(arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(windows)]
    process.creation_flags(CREATE_NO_WINDOW);
    let mut child = process.spawn().ok()?;
    let deadline = Instant::now() + Duration::from_secs(1);
    let success = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status.success(),
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(10));
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
            Err(_) => return None,
        }
    };
    if !success {
        return None;
    }
    let mut output = String::new();
    child.stdout.take()?.read_to_string(&mut output).ok()?;
    let output = output.trim();
    (!output.is_empty()).then(|| output.to_owned())
}

fn write_private_device_id(home_dir: &Path, device_id: &str) -> std::io::Result<()> {
    #[cfg(unix)]
    let already_existed = home_dir.exists();
    fs::create_dir_all(home_dir)?;
    #[cfg(unix)]
    if !already_existed {
        use std::os::unix::fs::PermissionsExt;

        fs::set_permissions(home_dir, fs::Permissions::from_mode(0o700))?;
    }
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(home_dir.join("device_id"))?;
    file.write_all(device_id.as_bytes())
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

    fn temp_home() -> PathBuf {
        let id = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!("kimi-oauth-identity-{}-{id}", std::process::id()))
    }

    fn linux_info() -> DeviceSystemInfo {
        DeviceSystemInfo {
            hostname: "  myhost  ".to_owned(),
            os_type: "Linux".to_owned(),
            os_release: "#101-Ubuntu SMP\n".to_owned(),
            architecture: "x64".to_owned(),
            macos_product_version: None,
        }
    }

    #[test]
    fn creates_reads_and_reuses_device_ids_per_home() {
        let first_home = temp_home();
        let second_home = temp_home();
        assert_eq!(read_kimi_device_id(&first_home), None);
        let first = create_kimi_device_id_with_status(&first_home);
        let repeated = create_kimi_device_id_with_status(&first_home);
        let second = create_kimi_device_id(&second_home);

        assert!(first.first_launch);
        assert!(!repeated.first_launch);
        assert_eq!(repeated.id, first.id);
        assert_ne!(second, first.id);
        assert_eq!(read_kimi_device_id(&first_home), Some(first.id));
        fs::remove_dir_all(first_home).expect("cleanup first");
        fs::remove_dir_all(second_home).expect("cleanup second");
    }

    #[test]
    fn empty_device_id_is_missing() {
        let home = temp_home();
        fs::create_dir_all(&home).expect("home");
        fs::write(home.join("device_id"), "  \n").expect("empty id");
        assert_eq!(read_kimi_device_id(&home), None);
        fs::remove_dir_all(home).expect("cleanup");
    }

    #[test]
    fn builds_complete_sanitized_device_and_default_headers() {
        let home = temp_home();
        let headers = create_kimi_device_headers_with_info(&home, " 1.2.3-test\n", &linux_info())
            .expect("headers");
        assert_eq!(headers["X-Msh-Platform"], KIMI_CODE_PLATFORM);
        assert_eq!(headers["X-Msh-Version"], "1.2.3-test");
        assert_eq!(headers["X-Msh-Device-Name"], "myhost");
        assert_eq!(headers["X-Msh-Device-Model"], "Linux #101-Ubuntu SMP x64");
        assert_eq!(headers["X-Msh-Os-Version"], "#101-Ubuntu SMP");
        assert!(uuid::Uuid::parse_str(&headers["X-Msh-Device-Id"]).is_ok());

        let options = KimiIdentityOptions {
            home_dir: home.clone(),
            host: KimiHostIdentity {
                user_agent_product: "kimi-code-cli".to_owned(),
                version: "1.2.3".to_owned(),
                user_agent_suffix: None,
            },
        };
        let defaults = create_kimi_default_headers(&options).expect("default headers");
        assert_eq!(defaults["User-Agent"], "kimi-code-cli/1.2.3");
        assert_eq!(defaults["X-Msh-Version"], "1.2.3");
        fs::remove_dir_all(home).expect("cleanup");
    }

    #[test]
    fn user_agent_sanitizes_fields_and_omits_empty_suffix() {
        for (suffix, expected) in [
            (None, "kimi-code-cli/1.2.3"),
            (Some("wire 4.5.6"), "kimi-code-cli/1.2.3 (wire 4.5.6)"),
            (Some("浣犲ソ"), "kimi-code-cli/1.2.3"),
        ] {
            assert_eq!(
                create_kimi_user_agent(&KimiHostIdentity {
                    user_agent_product: " kimi-code-cli ".to_owned(),
                    version: "h茅llo 1.2.3\n".to_owned(),
                    user_agent_suffix: suffix.map(str::to_owned),
                })
                .expect("user agent"),
                expected.replace("1.2.3", "hllo 1.2.3")
            );
        }
    }

    #[test]
    fn validates_required_identity_fields() {
        assert_eq!(
            assert_kimi_host_identity(None)
                .expect_err("missing")
                .to_string(),
            "Kimi host identity is required. Pass the host product name and version."
        );
        let error = create_kimi_user_agent(&KimiHostIdentity {
            user_agent_product: "浣犲ソ".to_owned(),
            version: "1.2.3".to_owned(),
            user_agent_suffix: None,
        })
        .expect_err("invalid product");
        assert_eq!(
            error.to_string(),
            "Kimi identity product must be a non-empty ASCII string."
        );
    }

    #[test]
    fn parses_custom_headers_using_first_colon_and_skips_invalid_lines() {
        let environment = HashMap::from([(
            KIMI_CODE_CUSTOM_HEADERS_ENV.to_owned(),
            " X-One: first \ninvalid\n: blank\nX-Two: a:b \nX-One: replaced ".to_owned(),
        )]);
        assert_eq!(
            parse_kimi_code_custom_headers(&environment),
            IndexMap::from([
                ("X-One".to_owned(), "replaced".to_owned()),
                ("X-Two".to_owned(), "a:b".to_owned())
            ])
        );
    }

    #[test]
    fn formats_macos_and_windows_device_models() {
        let mut info = linux_info();
        info.os_type = "Darwin".to_owned();
        info.os_release = "25.5.0".to_owned();
        info.architecture = "arm64".to_owned();
        assert_eq!(device_model(&info), "macOS 25.5.0 arm64");
        info.macos_product_version = Some("15.5".to_owned());
        assert_eq!(device_model(&info), "macOS 15.5 arm64");
        info.os_type = "Windows_NT".to_owned();
        info.os_release = "10.0.26100".to_owned();
        info.architecture = "x64".to_owned();
        assert_eq!(device_model(&info), "Windows 10.0.26100 x64");
    }

    #[cfg(windows)]
    #[test]
    fn parses_localized_windows_ver_output_by_its_bracketed_version() {
        assert_eq!(
            windows_release_from_ver("Microsoft Windows [Version 10.0.26100.4652]").as_deref(),
            Some("10.0.26100.4652")
        );
        assert_eq!(
            windows_release_from_ver("Microsoft Windows [版本 10.0.26100.4652]").as_deref(),
            Some("10.0.26100.4652")
        );
    }
}
