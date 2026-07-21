use std::error::Error;

use async_trait::async_trait;

use super::types::{InstallSource, NPM_PACKAGE_NAME};

const PNPM_PATH_SEGMENT: &str = "pnpm/global/";
const YARN_PATH_SEGMENTS: [&str; 2] = [".config/yarn/global/", "/.yarn/global/"];
const BUN_PATH_SEGMENT: &str = ".bun/install/global/";
const HOMEBREW_PATH_SEGMENT: &str = "/cellar/";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallPlatform {
    Windows,
    Unix,
}

pub fn current_install_platform() -> InstallPlatform {
    if cfg!(windows) {
        InstallPlatform::Windows
    } else {
        InstallPlatform::Unix
    }
}

// Original:
//   apps/kimi-code/src/cli/update/source.ts
//   classifyByPathHeuristic()
pub fn classify_by_path_heuristic(package_root: &str) -> Option<InstallSource> {
    let normalized = package_root.replace('\\', "/").to_ascii_lowercase();
    if normalized.contains(PNPM_PATH_SEGMENT) {
        return Some(InstallSource::PnpmGlobal);
    }
    if YARN_PATH_SEGMENTS
        .iter()
        .any(|segment| normalized.contains(segment))
    {
        return Some(InstallSource::YarnGlobal);
    }
    if normalized.contains(BUN_PATH_SEGMENT) {
        return Some(InstallSource::BunGlobal);
    }
    if normalized.contains(HOMEBREW_PATH_SEGMENT) {
        return Some(InstallSource::Homebrew);
    }
    None
}

// Original:
//   apps/kimi-code/src/cli/update/source.ts
//   classifyInstallSource()
pub fn classify_install_source(
    package_root: &str,
    global_prefix: &str,
    platform: InstallPlatform,
) -> InstallSource {
    let Some(package_root) = normalize_path_for_comparison(package_root, platform) else {
        return InstallSource::Unsupported;
    };
    for candidate in candidate_global_package_dirs(global_prefix, platform) {
        if normalize_path_for_comparison(&candidate, platform).as_deref()
            == Some(package_root.as_str())
        {
            return InstallSource::NpmGlobal;
        }
    }
    InstallSource::Unsupported
}

#[async_trait]
pub trait DetectInstallSourceDeps: Send + Sync {
    type Error: Error + Send + Sync + 'static;

    fn package_root(&self) -> String;

    async fn global_prefix(&self) -> Result<String, Self::Error>;

    fn detect_native(&self) -> bool;

    fn platform(&self) -> InstallPlatform;
}

// Original:
//   apps/kimi-code/src/cli/update/source.ts
//   detectInstallSource()
pub async fn detect_install_source<D>(deps: &D) -> InstallSource
where
    D: DetectInstallSourceDeps,
{
    if deps.detect_native() {
        return InstallSource::Native;
    }
    let package_root = deps.package_root();
    if let Some(source) = classify_by_path_heuristic(&package_root) {
        return source;
    }
    match deps.global_prefix().await {
        Ok(prefix) => classify_install_source(&package_root, &prefix, deps.platform()),
        Err(_) => InstallSource::Unsupported,
    }
}

fn candidate_global_package_dirs(global_prefix: &str, platform: InstallPlatform) -> Vec<String> {
    let prefix = global_prefix.trim_end_matches(['/', '\\']);
    match platform {
        InstallPlatform::Windows => {
            vec![format!("{prefix}/node_modules/{NPM_PACKAGE_NAME}")]
        }
        InstallPlatform::Unix => vec![
            format!("{prefix}/lib/node_modules/{NPM_PACKAGE_NAME}"),
            format!("{prefix}/node_modules/{NPM_PACKAGE_NAME}"),
        ],
    }
}

fn normalize_path_for_comparison(file_path: &str, platform: InstallPlatform) -> Option<String> {
    let trimmed = file_path.trim();
    if trimmed.is_empty() {
        return None;
    }
    let canonical = std::fs::canonicalize(trimmed)
        .ok()
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_else(|| {
            if is_absolute_for_platform(trimmed, platform) {
                trimmed.to_owned()
            } else {
                std::env::current_dir()
                    .map(|directory| directory.join(trimmed).to_string_lossy().into_owned())
                    .unwrap_or_else(|_| trimmed.to_owned())
            }
        });
    let normalized = lexical_normalize(&canonical);
    Some(match platform {
        InstallPlatform::Windows => normalized.to_ascii_lowercase(),
        InstallPlatform::Unix => normalized,
    })
}

fn lexical_normalize(path: &str) -> String {
    let path = path.replace('\\', "/");
    let path = path.strip_prefix("//?/").unwrap_or(&path);
    let drive_prefix = path
        .get(..2)
        .filter(|prefix| prefix.ends_with(':'))
        .unwrap_or("");
    let absolute = path.starts_with('/') || !drive_prefix.is_empty();
    let start = if drive_prefix.is_empty() { 0 } else { 2 };
    let mut parts = Vec::new();
    for part in path[start..].split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            part => parts.push(part),
        }
    }
    let joined = parts.join("/");
    if !drive_prefix.is_empty() {
        format!("{drive_prefix}/{joined}")
    } else if absolute {
        format!("/{joined}")
    } else {
        joined
    }
}

fn is_absolute_for_platform(path: &str, platform: InstallPlatform) -> bool {
    match platform {
        InstallPlatform::Windows => {
            path.starts_with(['/', '\\'])
                || path.as_bytes().get(1).is_some_and(|byte| *byte == b':')
        }
        InstallPlatform::Unix => path.starts_with('/'),
    }
}

#[cfg(test)]
mod tests {
    use std::fmt;

    use super::*;

    #[test]
    fn detects_all_path_heuristics_without_overmatching_homebrew_prefix() {
        assert_eq!(
            classify_by_path_heuristic(
                "/Users/me/Library/pnpm/global/5/node_modules/@moonshot-ai/kimi-code"
            ),
            Some(InstallSource::PnpmGlobal)
        );
        assert_eq!(
            classify_by_path_heuristic(
                r"C:\Users\me\AppData\Local\pnpm\global\5\node_modules\@moonshot-ai/kimi-code"
            ),
            Some(InstallSource::PnpmGlobal)
        );
        assert_eq!(
            classify_by_path_heuristic(
                "/Users/me/.config/yarn/global/node_modules/@moonshot-ai/kimi-code"
            ),
            Some(InstallSource::YarnGlobal)
        );
        assert_eq!(
            classify_by_path_heuristic(
                "/Users/me/.yarn/global/node_modules/@moonshot-ai/kimi-code"
            ),
            Some(InstallSource::YarnGlobal)
        );
        assert_eq!(
            classify_by_path_heuristic(
                "/Users/me/.bun/install/global/node_modules/@moonshot-ai/kimi-code"
            ),
            Some(InstallSource::BunGlobal)
        );
        assert_eq!(
            classify_by_path_heuristic(
                "/opt/homebrew/Cellar/kimi-code/0.5.0/libexec/lib/node_modules/@moonshot-ai/kimi-code"
            ),
            Some(InstallSource::Homebrew)
        );
        assert_eq!(
            classify_by_path_heuristic("/opt/homebrew/lib/node_modules/@moonshot-ai/kimi-code"),
            None
        );
    }

    #[test]
    fn classifies_unix_and_case_insensitive_windows_npm_global_paths() {
        assert_eq!(
            classify_install_source(
                "/usr/local/lib/node_modules/@moonshot-ai/kimi-code",
                "/usr/local",
                InstallPlatform::Unix,
            ),
            InstallSource::NpmGlobal
        );
        assert_eq!(
            classify_install_source(
                r"C:\USERS\ME\APPDATA\ROAMING\NPM\node_modules\@moonshot-ai\kimi-code",
                r"c:\users\me\appdata\roaming\npm",
                InstallPlatform::Windows,
            ),
            InstallSource::NpmGlobal
        );
        assert_eq!(
            classify_install_source(
                "/Users/me/dev/@moonshot-ai/kimi-code",
                "/usr/local",
                InstallPlatform::Unix,
            ),
            InstallSource::Unsupported
        );
    }

    #[derive(Debug, Clone, Copy)]
    struct PrefixError;

    impl fmt::Display for PrefixError {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("prefix failed")
        }
    }

    impl Error for PrefixError {}

    struct Deps {
        root: &'static str,
        prefix: Result<&'static str, PrefixError>,
        native: bool,
    }

    #[async_trait]
    impl DetectInstallSourceDeps for Deps {
        type Error = PrefixError;

        fn package_root(&self) -> String {
            self.root.to_owned()
        }

        async fn global_prefix(&self) -> Result<String, Self::Error> {
            self.prefix.map(str::to_owned)
        }

        fn detect_native(&self) -> bool {
            self.native
        }

        fn platform(&self) -> InstallPlatform {
            InstallPlatform::Unix
        }
    }

    #[tokio::test]
    async fn detection_prioritizes_native_then_heuristic_then_npm_prefix() {
        let native = Deps {
            root: "/usr/local/lib/node_modules/@moonshot-ai/kimi-code",
            prefix: Ok("/usr/local"),
            native: true,
        };
        assert_eq!(detect_install_source(&native).await, InstallSource::Native);

        let pnpm = Deps {
            root: "/Users/me/Library/pnpm/global/5/node_modules/@moonshot-ai/kimi-code",
            prefix: Err(PrefixError),
            native: false,
        };
        assert_eq!(
            detect_install_source(&pnpm).await,
            InstallSource::PnpmGlobal
        );

        let npm = Deps {
            root: "/usr/local/lib/node_modules/@moonshot-ai/kimi-code",
            prefix: Ok("/usr/local"),
            native: false,
        };
        assert_eq!(detect_install_source(&npm).await, InstallSource::NpmGlobal);
    }

    #[tokio::test]
    async fn prefix_failure_and_unknown_layout_are_unsupported() {
        let failed = Deps {
            root: "/Users/me/dev/@moonshot-ai/kimi-code",
            prefix: Err(PrefixError),
            native: false,
        };
        assert_eq!(
            detect_install_source(&failed).await,
            InstallSource::Unsupported
        );
    }
}
