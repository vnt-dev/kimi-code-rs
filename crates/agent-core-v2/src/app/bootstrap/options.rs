//! Startup option resolution and bootstrap path helpers.
//!
//! Original: `packages/agent-core-v2/src/app/bootstrap/bootstrap.ts`.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use crate::_base::di::instantiation::ServiceIdentifier;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BootstrapOptions {
    pub home_dir: PathBuf,
    pub config_path: PathBuf,
    pub os_home_dir: PathBuf,
    pub platform: String,
    pub arch: String,
    pub cwd: PathBuf,
    pub env: HashMap<String, String>,
    pub client_version: String,
}

pub const BOOTSTRAP_OPTIONS_ID: ServiceIdentifier<BootstrapOptions> =
    ServiceIdentifier::new("bootstrapOptions");

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct BootstrapInput {
    pub home_dir: Option<PathBuf>,
    pub config_path: Option<PathBuf>,
    pub env: Option<HashMap<String, String>>,
    pub os_home_dir: Option<PathBuf>,
    pub platform: Option<String>,
    pub arch: Option<String>,
    pub cwd: Option<PathBuf>,
    pub client_version: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum BootstrapResolveError {
    #[error("unable to resolve the current user's home directory")]
    HomeDirectoryUnavailable,

    #[error("unable to resolve the current working directory")]
    CurrentDirectory(#[source] std::io::Error),
}

// Original: resolveBootstrapOptions(). Environment and host facts are copied
// once so downstream services observe the same frozen startup snapshot.
pub fn resolve_bootstrap_options(
    input: BootstrapInput,
) -> Result<BootstrapOptions, BootstrapResolveError> {
    let env = input.env.unwrap_or_else(current_environment);
    let os_home_dir = match input.os_home_dir {
        Some(home) => home,
        None => current_home_dir(&env).ok_or(BootstrapResolveError::HomeDirectoryUnavailable)?,
    };
    let home_dir = resolve_kimi_home(input.home_dir.as_deref(), &env, &os_home_dir);
    let config_path = resolve_config_path(
        Some(&home_dir),
        input.config_path.as_deref(),
        &env,
        &os_home_dir,
    );
    let cwd = match input.cwd {
        Some(cwd) => cwd,
        None => std::env::current_dir().map_err(BootstrapResolveError::CurrentDirectory)?,
    };
    Ok(BootstrapOptions {
        home_dir,
        config_path,
        os_home_dir,
        platform: input.platform.unwrap_or_else(node_platform),
        arch: input.arch.unwrap_or_else(node_arch),
        cwd,
        env,
        client_version: input.client_version.unwrap_or_else(|| "unknown".into()),
    })
}

// Original: resolveKimiHome().
pub fn resolve_kimi_home(
    home_dir: Option<&Path>,
    env: &HashMap<String, String>,
    os_home_dir: &Path,
) -> PathBuf {
    home_dir
        .map(Path::to_path_buf)
        .or_else(|| env.get("KIMI_CODE_HOME").map(PathBuf::from))
        .unwrap_or_else(|| os_home_dir.join(".kimi-code"))
}

// Original: resolveConfigPath().
pub fn resolve_config_path(
    home_dir: Option<&Path>,
    config_path: Option<&Path>,
    env: &HashMap<String, String>,
    os_home_dir: &Path,
) -> PathBuf {
    config_path
        .map(Path::to_path_buf)
        .unwrap_or_else(|| resolve_kimi_home(home_dir, env, os_home_dir).join("config.toml"))
}

// Original: ensureKimiHome(). This intentionally remains synchronous because
// it is a startup helper and the original method is synchronous.
pub fn ensure_kimi_home(home_dir: &Path) -> std::io::Result<()> {
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(home_dir)
}

fn current_environment() -> HashMap<String, String> {
    std::env::vars_os()
        .filter_map(|(key, value)| Some((key.into_string().ok()?, value.into_string().ok()?)))
        .collect()
}

#[cfg(unix)]
fn current_home_dir(env: &HashMap<String, String>) -> Option<PathBuf> {
    use uzers::os::unix::UserExt;

    env.get("HOME").map(PathBuf::from).or_else(|| {
        uzers::get_user_by_uid(uzers::get_current_uid()).map(|user| user.home_dir().to_path_buf())
    })
}

#[cfg(windows)]
fn current_home_dir(env: &HashMap<String, String>) -> Option<PathBuf> {
    env.get("USERPROFILE")
        .map(PathBuf::from)
        .or_else(|| Some(PathBuf::from(env.get("HOMEDRIVE")?).join(env.get("HOMEPATH")?)))
}

#[cfg(not(any(unix, windows)))]
fn current_home_dir(env: &HashMap<String, String>) -> Option<PathBuf> {
    env.get("HOME").map(PathBuf::from)
}

fn node_platform() -> String {
    match std::env::consts::OS {
        "windows" => "win32",
        "macos" => "darwin",
        other => other,
    }
    .into()
}

fn node_arch() -> String {
    match std::env::consts::ARCH {
        "x86_64" => "x64",
        "x86" => "ia32",
        "aarch64" => "arm64",
        other => other,
    }
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_home_precedes_environment_and_os_home() {
        let env = HashMap::from([("KIMI_CODE_HOME".into(), "/c".into())]);
        assert_eq!(
            resolve_kimi_home(Some(Path::new("/a")), &env, Path::new("/b")),
            Path::new("/a")
        );
        assert_eq!(
            resolve_kimi_home(None, &env, Path::new("/b")),
            Path::new("/c")
        );
        assert_eq!(
            resolve_kimi_home(None, &HashMap::new(), Path::new("/b")),
            Path::new("/b/.kimi-code")
        );
    }

    #[test]
    fn options_resolve_and_freeze_all_defaults() {
        let options = resolve_bootstrap_options(BootstrapInput {
            os_home_dir: Some("/home/test".into()),
            env: Some(HashMap::new()),
            cwd: Some("/work".into()),
            ..BootstrapInput::default()
        })
        .unwrap();
        assert_eq!(options.home_dir, Path::new("/home/test/.kimi-code"));
        assert_eq!(
            options.config_path,
            Path::new("/home/test/.kimi-code/config.toml")
        );
        assert_eq!(options.cwd, Path::new("/work"));
        assert_eq!(options.client_version, "unknown");
    }

    #[test]
    fn explicit_config_path_precedes_derived_path() {
        assert_eq!(
            resolve_config_path(
                Some(Path::new("/tmp/kimi")),
                Some(Path::new("/x/config.toml")),
                &HashMap::new(),
                Path::new("/home/test"),
            ),
            Path::new("/x/config.toml")
        );
        assert_eq!(
            resolve_config_path(
                Some(Path::new("/tmp/kimi")),
                None,
                &HashMap::new(),
                Path::new("/home/test"),
            ),
            Path::new("/tmp/kimi/config.toml")
        );
    }

    #[test]
    fn ensure_home_creates_nested_directory() {
        let root =
            std::env::temp_dir().join(format!("kimi-code-bootstrap-{}", uuid::Uuid::new_v4()));
        let nested = root.join("nested");
        ensure_kimi_home(&nested).unwrap();
        assert!(nested.is_dir());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                nested.metadata().unwrap().permissions().mode() & 0o777,
                0o700
            );
        }
        std::fs::remove_dir_all(root).unwrap();
    }
}
