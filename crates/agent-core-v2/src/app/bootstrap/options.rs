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

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ResolveConfigPathInput {
    pub home_dir: Option<PathBuf>,
    pub config_path: Option<PathBuf>,
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
        None => current_home_dir().ok_or(BootstrapResolveError::HomeDirectoryUnavailable)?,
    };
    let home_dir =
        resolve_kimi_home_with_environment(input.home_dir.as_deref(), &env, &os_home_dir);
    let config_path = resolve_config_path_with_environment(
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

// Original: resolveKimiHome(). This public entry point implements the source
// defaults (`process.env` and `os.homedir()`) through the maintained `dirs`
// crate rather than duplicating platform-specific home-directory probing.
pub fn resolve_kimi_home(home_dir: Option<&Path>) -> Result<PathBuf, BootstrapResolveError> {
    let env = current_environment();
    let os_home_dir = current_home_dir().ok_or(BootstrapResolveError::HomeDirectoryUnavailable)?;
    Ok(resolve_kimi_home_with_environment(
        home_dir,
        &env,
        &os_home_dir,
    ))
}

/// Resolves the same precedence with explicitly captured host facts. Bootstrap
/// uses this to freeze startup state, while tests avoid reading process state.
pub fn resolve_kimi_home_with_environment(
    home_dir: Option<&Path>,
    env: &HashMap<String, String>,
    os_home_dir: &Path,
) -> PathBuf {
    home_dir
        .map(Path::to_path_buf)
        .or_else(|| env.get("KIMI_CODE_HOME").map(PathBuf::from))
        .unwrap_or_else(|| os_home_dir.join(".kimi-code"))
}

// Original: resolveConfigPath(). Like the TypeScript entry point, this reads
// the current environment only when it needs to derive the default Kimi home.
pub fn resolve_config_path(
    input: ResolveConfigPathInput,
) -> Result<PathBuf, BootstrapResolveError> {
    if let Some(config_path) = input.config_path {
        return Ok(config_path);
    }
    Ok(resolve_kimi_home(input.home_dir.as_deref())?.join("config.toml"))
}

/// Deterministic variant used by bootstrap after it freezes the host facts.
pub fn resolve_config_path_with_environment(
    home_dir: Option<&Path>,
    config_path: Option<&Path>,
    env: &HashMap<String, String>,
    os_home_dir: &Path,
) -> PathBuf {
    config_path.map(Path::to_path_buf).unwrap_or_else(|| {
        resolve_kimi_home_with_environment(home_dir, env, os_home_dir).join("config.toml")
    })
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

fn current_home_dir() -> Option<PathBuf> {
    dirs::home_dir()
}

fn node_platform() -> String {
    match std::env::consts::OS {
        "windows" => "win32",
        "macos" => "darwin",
        "illumos" => "sunos",
        other => other,
    }
    .into()
}

fn node_arch() -> String {
    match std::env::consts::ARCH {
        "x86_64" => "x64",
        "x86" => "ia32",
        "aarch64" => "arm64",
        "loongarch64" => "loong64",
        "powerpc" => "ppc",
        "powerpc64" => "ppc64",
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
            resolve_kimi_home_with_environment(Some(Path::new("/a")), &env, Path::new("/b")),
            Path::new("/a")
        );
        assert_eq!(
            resolve_kimi_home_with_environment(None, &env, Path::new("/b")),
            Path::new("/c")
        );
        assert_eq!(
            resolve_kimi_home_with_environment(None, &HashMap::new(), Path::new("/b")),
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
    fn explicit_options_are_preserved_in_the_frozen_snapshot() {
        let env = HashMap::from([("FOO".into(), "bar".into())]);
        let options = resolve_bootstrap_options(BootstrapInput {
            home_dir: Some("/home/kimi".into()),
            config_path: Some("/config/custom.toml".into()),
            env: Some(env.clone()),
            os_home_dir: Some("/home/os".into()),
            platform: Some("test-platform".into()),
            arch: Some("test-arch".into()),
            cwd: Some("/work".into()),
            client_version: Some("1.2.3".into()),
        })
        .unwrap();

        assert_eq!(options.home_dir, Path::new("/home/kimi"));
        assert_eq!(options.config_path, Path::new("/config/custom.toml"));
        assert_eq!(options.env, env);
        assert_eq!(options.os_home_dir, Path::new("/home/os"));
        assert_eq!(options.platform, "test-platform");
        assert_eq!(options.arch, "test-arch");
        assert_eq!(options.cwd, Path::new("/work"));
        assert_eq!(options.client_version, "1.2.3");
    }

    #[test]
    fn explicit_config_path_precedes_derived_path() {
        assert_eq!(
            resolve_config_path_with_environment(
                Some(Path::new("/tmp/kimi")),
                Some(Path::new("/x/config.toml")),
                &HashMap::new(),
                Path::new("/home/test"),
            ),
            Path::new("/x/config.toml")
        );
        assert_eq!(
            resolve_config_path_with_environment(
                Some(Path::new("/tmp/kimi")),
                None,
                &HashMap::new(),
                Path::new("/home/test"),
            ),
            Path::new("/tmp/kimi/config.toml")
        );
    }

    #[test]
    fn public_config_path_helper_matches_source_input_shape() {
        assert_eq!(
            resolve_config_path(ResolveConfigPathInput {
                home_dir: Some("/tmp/kimi".into()),
                config_path: None,
            })
            .unwrap(),
            Path::new("/tmp/kimi/config.toml")
        );
        assert_eq!(
            resolve_config_path(ResolveConfigPathInput {
                home_dir: None,
                config_path: Some("/tmp/custom.toml".into()),
            })
            .unwrap(),
            Path::new("/tmp/custom.toml")
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
