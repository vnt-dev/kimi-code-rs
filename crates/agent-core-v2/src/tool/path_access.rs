//! Workspace path access policy for file tools.
//!
//! Original: `packages/agent-core-v2/src/tool/path-access.ts`.

use std::{borrow::Cow, fmt};

use crate::_base::exec_env::environment_probe::HostEnvironmentInfo;
pub use crate::_base::exec_env::environment_probe::PathClass;

const SENSITIVE_BASENAMES: &[&str] = &[".env", "id_rsa", "id_ed25519", "id_ecdsa", "credentials"];
const ENV_EXEMPTIONS: &[&str] = &[".env.example", ".env.sample", ".env.template"];
const SENSITIVE_PREFIXES: &[&str] = &["id_rsa", "id_ed25519", "id_ecdsa", "credentials"];
const PUBLIC_KEYS: &[&str] = &["id_rsa.pub", "id_ed25519.pub", "id_ecdsa.pub"];
pub const SENSITIVE_DOT_VARIANT_SUFFIXES: &[&str] = &[
    ".bak",
    ".backup",
    ".copy",
    ".disabled",
    ".key",
    ".old",
    ".orig",
    ".pem",
    ".save",
    ".tmp",
];

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkspaceConfig {
    pub workspace_dir: String,
    pub additional_dirs: Vec<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PathSecurityCode {
    OutsideWorkspace,
    Sensitive,
    Invalid,
}

impl PathSecurityCode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::OutsideWorkspace => "PATH_OUTSIDE_WORKSPACE",
            Self::Sensitive => "PATH_SENSITIVE",
            Self::Invalid => "PATH_INVALID",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PathSecurityError {
    pub code: PathSecurityCode,
    pub raw_path: String,
    pub canonical_path: String,
    message: String,
}

impl fmt::Display for PathSecurityError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for PathSecurityError {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PathAccessOperation {
    Read,
    Write,
    Search,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WorkspaceGuardMode {
    AbsoluteOutsideAllowed,
    Disabled,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WorkspaceAccessPolicy {
    pub guard_mode: WorkspaceGuardMode,
    pub check_sensitive: bool,
}

pub const DEFAULT_WORKSPACE_ACCESS_POLICY: WorkspaceAccessPolicy = WorkspaceAccessPolicy {
    guard_mode: WorkspaceGuardMode::AbsoluteOutsideAllowed,
    check_sensitive: true,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PathAccess {
    pub path: String,
    pub outside_workspace: bool,
}

pub fn is_sensitive_file(path: &str) -> bool {
    let normalized = path.replace('\\', "/").to_ascii_lowercase();
    let name = normalized.rsplit('/').next().unwrap_or_default();
    if ENV_EXEMPTIONS.contains(&name) || PUBLIC_KEYS.contains(&name) {
        return false;
    }
    if SENSITIVE_BASENAMES.contains(&name) || name.starts_with(".env.") {
        return true;
    }
    for prefix in SENSITIVE_PREFIXES {
        let Some(suffix) = name.strip_prefix(prefix) else {
            continue;
        };
        if suffix.is_empty()
            || suffix.starts_with(['-', '_'])
            || (suffix.starts_with('.') && SENSITIVE_DOT_VARIANT_SUFFIXES.contains(&suffix))
        {
            return true;
        }
    }
    [".aws/credentials", ".gcp/credentials"]
        .iter()
        .any(|suffix| {
            normalized.ends_with(&format!("/{suffix}"))
                || normalized.contains(&format!("/{suffix}/"))
        })
}

pub fn normalize_user_path(path: &str, path_class: PathClass) -> String {
    if path_class != PathClass::Win32 || path == "/" || path.starts_with("//") {
        return path.to_owned();
    }
    if let Some(rest) = path.strip_prefix("/cygdrive/")
        && let Some((drive, tail)) = split_drive_path(rest)
    {
        return format_drive_path(drive, tail);
    }
    if let Some(rest) = path.strip_prefix('/')
        && let Some((drive, tail)) = split_drive_path(rest)
    {
        return format_drive_path(drive, tail);
    }
    path.to_owned()
}

fn split_drive_path(value: &str) -> Option<(char, &str)> {
    let mut characters = value.chars();
    let drive = characters.next()?;
    if !drive.is_ascii_alphabetic() {
        return None;
    }
    let tail = characters.as_str();
    (tail.is_empty() || tail.starts_with('/')).then_some((drive.to_ascii_uppercase(), tail))
}

fn format_drive_path(drive: char, tail: &str) -> String {
    format!("{drive}:{}", if tail.is_empty() { "/" } else { tail })
}

// Original: canonicalizePath(). This is lexical and intentionally does not resolve symlinks.
pub fn canonicalize_path(
    path: &str,
    cwd: &str,
    path_class: PathClass,
) -> Result<String, PathSecurityError> {
    if path.is_empty() {
        return Err(path_error(
            PathSecurityCode::Invalid,
            path,
            path,
            "Path cannot be empty".into(),
        ));
    }
    let normalized = normalize_user_path(path, path_class);
    if path_class == PathClass::Win32 && is_win32_drive_relative(&normalized) {
        return Err(path_error(
            PathSecurityCode::Invalid,
            path,
            &normalized,
            format!(
                "\"{path}\" is a drive-relative Windows path. Use an absolute path like C:\\path or a path relative to the working directory."
            ),
        ));
    }
    if !is_absolute(&normalized) && !is_absolute(cwd) {
        return Err(path_error(
            PathSecurityCode::Invalid,
            path,
            &normalized,
            format!("Cannot resolve \"{path}\" against non-absolute cwd \"{cwd}\"."),
        ));
    }
    let absolute = if is_absolute(&normalized) {
        normalized
    } else {
        format!("{}/{}", cwd.trim_end_matches(['/', '\\']), normalized)
    };
    Ok(normalize_lexical(&absolute))
}

fn is_win32_drive_relative(path: &str) -> bool {
    let bytes = path.as_bytes();
    bytes.len() >= 2
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && (bytes.len() == 2 || !matches!(bytes[2], b'/' | b'\\'))
}

fn is_absolute(path: &str) -> bool {
    let path = path.replace('\\', "/");
    path.starts_with('/')
        || (path.len() >= 3
            && path.as_bytes()[0].is_ascii_alphabetic()
            && path.as_bytes()[1] == b':'
            && path.as_bytes()[2] == b'/')
}

fn normalize_lexical(path: &str) -> String {
    let path = path.replace('\\', "/");
    let (prefix, remainder) = if path.starts_with("//") {
        ("//".to_owned(), path.trim_start_matches('/'))
    } else if path.len() >= 3 && path.as_bytes()[1] == b':' && path.as_bytes()[2] == b'/' {
        (format!("{}:/", &path[..1]), &path[3..])
    } else if let Some(remainder) = path.strip_prefix('/') {
        ("/".to_owned(), remainder)
    } else {
        (String::new(), path.as_str())
    };
    let mut components = Vec::new();
    for component in remainder.split('/') {
        match component {
            "" | "." => {}
            ".." => {
                components.pop();
            }
            component => components.push(component),
        }
    }
    if components.is_empty() {
        prefix
    } else if prefix.is_empty() {
        components.join("/")
    } else {
        format!("{}{}", prefix, components.join("/"))
    }
}

pub fn is_within_directory(candidate: &str, base: &str, path_class: PathClass) -> bool {
    let candidate = normalize_lexical(candidate);
    let base = normalize_lexical(base);
    let (candidate, base) = if path_class == PathClass::Win32 {
        (candidate.to_ascii_lowercase(), base.to_ascii_lowercase())
    } else {
        (candidate, base)
    };
    candidate == base || candidate.starts_with(&format!("{}/", base.trim_end_matches('/')))
}

pub fn is_within_workspace(
    candidate: &str,
    workspace: &WorkspaceConfig,
    path_class: PathClass,
) -> bool {
    is_within_directory(candidate, &workspace.workspace_dir, path_class)
        || workspace
            .additional_dirs
            .iter()
            .any(|directory| is_within_directory(candidate, directory, path_class))
}

pub fn extend_workspace_with_skill_roots<'a>(
    workspace: &'a WorkspaceConfig,
    skill_roots: &[String],
    path_class: PathClass,
) -> Cow<'a, WorkspaceConfig> {
    let mut additional = workspace.additional_dirs.clone();
    for root in skill_roots {
        if is_within_directory(root, &workspace.workspace_dir, path_class)
            || additional
                .iter()
                .any(|directory| is_within_directory(root, directory, path_class))
        {
            continue;
        }
        additional.push(root.clone());
    }
    if additional.len() == workspace.additional_dirs.len() {
        Cow::Borrowed(workspace)
    } else {
        Cow::Owned(WorkspaceConfig {
            workspace_dir: workspace.workspace_dir.clone(),
            additional_dirs: additional,
        })
    }
}

pub fn resolve_path_access(
    path: &str,
    cwd: &str,
    workspace: &WorkspaceConfig,
    operation: PathAccessOperation,
    policy: WorkspaceAccessPolicy,
    path_class: PathClass,
    home_dir: Option<&str>,
) -> Result<PathAccess, PathSecurityError> {
    let normalized = normalize_user_path(path, path_class);
    let expanded = expand_user_path(&normalized, home_dir, path_class);
    let raw_is_absolute = is_absolute(&expanded);
    let canonical = canonicalize_path(&expanded, cwd, path_class)?;
    let outside_workspace = !is_within_workspace(&canonical, workspace, path_class);
    if policy.check_sensitive && is_sensitive_file(&canonical) {
        return Err(path_error(
            PathSecurityCode::Sensitive,
            path,
            &canonical,
            format!(
                "\"{path}\" matches a sensitive-file pattern (env / credential / SSH key). Access is blocked to protect secrets."
            ),
        ));
    }
    if outside_workspace
        && policy.guard_mode == WorkspaceGuardMode::AbsoluteOutsideAllowed
        && !raw_is_absolute
    {
        let verb = match operation {
            PathAccessOperation::Write => "write or edit a file",
            PathAccessOperation::Search => "search",
            PathAccessOperation::Read => "read a file",
        };
        return Err(path_error(
            PathSecurityCode::OutsideWorkspace,
            path,
            &canonical,
            format!(
                "\"{path}\" is not an absolute path. You must provide an absolute path to {verb} outside the working directory."
            ),
        ));
    }
    Ok(PathAccess {
        path: canonical,
        outside_workspace,
    })
}

// Original: resolvePathAccessPath().
pub fn resolve_path_access_path(
    path: &str,
    environment: &HostEnvironmentInfo,
    workspace: &WorkspaceConfig,
    operation: PathAccessOperation,
    policy: WorkspaceAccessPolicy,
    expand_home: bool,
) -> Result<String, PathSecurityError> {
    resolve_path_access(
        path,
        &workspace.workspace_dir,
        workspace,
        operation,
        policy,
        environment.path_class,
        expand_home.then_some(environment.home_dir.as_str()),
    )
    .map(|access| access.path)
}

// Original: assertPathAllowed().
pub fn assert_path_allowed(
    path: &str,
    cwd: &str,
    workspace: &WorkspaceConfig,
    operation: PathAccessOperation,
    check_sensitive: Option<bool>,
    path_class: PathClass,
) -> Result<String, PathSecurityError> {
    resolve_path_access(
        path,
        cwd,
        workspace,
        operation,
        WorkspaceAccessPolicy {
            guard_mode: WorkspaceGuardMode::AbsoluteOutsideAllowed,
            check_sensitive: check_sensitive
                .unwrap_or(DEFAULT_WORKSPACE_ACCESS_POLICY.check_sensitive),
        },
        path_class,
        None,
    )
    .map(|access| access.path)
}

pub(crate) fn expand_user_path(
    path: &str,
    home_dir: Option<&str>,
    path_class: PathClass,
) -> String {
    let Some(home) = home_dir else {
        return path.to_owned();
    };
    if path == "~" {
        return home.to_owned();
    }
    let windows_rest = if path_class == PathClass::Win32 {
        path.strip_prefix("~\\")
    } else {
        None
    };
    if let Some(rest) = path.strip_prefix("~/").or(windows_rest) {
        return format!("{}/{}", home.trim_end_matches(['/', '\\']), rest);
    }
    path.to_owned()
}

fn path_error(
    code: PathSecurityCode,
    raw_path: &str,
    canonical_path: &str,
    message: String,
) -> PathSecurityError {
    PathSecurityError {
        code,
        raw_path: raw_path.into(),
        canonical_path: canonical_path.into(),
        message,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::_base::exec_env::environment_probe::ShellName;

    #[test]
    fn sensitive_patterns_preserve_exemptions_and_case_folding() {
        for path in [
            ".env",
            "/app/.Env.Local",
            "/home/user/.AWS/Credentials",
            "/home/user/.ssh/ID_ED25519.OLD",
            "credentials",
        ] {
            assert!(is_sensitive_file(path), "{path}");
        }
        for path in [
            ".env.example",
            "id_rsa.pub",
            "credentials.json",
            ".envrc",
            "server.key.example",
        ] {
            assert!(!is_sensitive_file(path), "{path}");
        }
    }

    #[test]
    fn canonicalization_handles_msys_windows_and_shared_prefixes() {
        assert_eq!(
            canonicalize_path("/cygdrive/c/repo/../file", "C:/repo", PathClass::Win32).unwrap(),
            "C:/file"
        );
        assert_eq!(
            canonicalize_path("./dir/../file", "/repo", PathClass::Posix).unwrap(),
            "/repo/file"
        );
        assert!(is_within_directory(
            "/workspace/a",
            "/workspace",
            PathClass::Posix
        ));
        assert!(!is_within_directory(
            "/workspace-evil",
            "/workspace",
            PathClass::Posix
        ));
        assert!(canonicalize_path("C:relative", "C:/repo", PathClass::Win32).is_err());
    }

    #[test]
    fn skill_roots_dedupe_nested_and_windows_case_variants() {
        let workspace = WorkspaceConfig {
            workspace_dir: "/repo".into(),
            additional_dirs: vec!["/extra".into()],
        };
        let extended = extend_workspace_with_skill_roots(
            &workspace,
            &["/skills".into(), "/skills".into(), "/skills/sub".into()],
            PathClass::Posix,
        );
        assert_eq!(extended.additional_dirs, vec!["/extra", "/skills"]);
        let windows = WorkspaceConfig {
            workspace_dir: "C:/repo".into(),
            additional_dirs: Vec::new(),
        };
        assert!(matches!(
            extend_workspace_with_skill_roots(
                &windows,
                &["c:/Repo/skills".into()],
                PathClass::Win32
            ),
            Cow::Borrowed(_)
        ));
    }

    #[test]
    fn resolution_blocks_sensitive_and_relative_outside_but_allows_absolute_outside() {
        let workspace = WorkspaceConfig {
            workspace_dir: "/repo/work".into(),
            additional_dirs: Vec::new(),
        };
        let sensitive = resolve_path_access(
            ".env",
            "/repo/work",
            &workspace,
            PathAccessOperation::Read,
            DEFAULT_WORKSPACE_ACCESS_POLICY,
            PathClass::Posix,
            None,
        )
        .unwrap_err();
        assert_eq!(sensitive.code, PathSecurityCode::Sensitive);
        let relative = resolve_path_access(
            "../../outside",
            "/repo/work",
            &workspace,
            PathAccessOperation::Read,
            DEFAULT_WORKSPACE_ACCESS_POLICY,
            PathClass::Posix,
            None,
        )
        .unwrap_err();
        assert_eq!(relative.code, PathSecurityCode::OutsideWorkspace);
        let absolute = resolve_path_access(
            "/outside",
            "/repo/work",
            &workspace,
            PathAccessOperation::Read,
            DEFAULT_WORKSPACE_ACCESS_POLICY,
            PathClass::Posix,
            None,
        )
        .unwrap();
        assert!(absolute.outside_workspace);

        let unguarded = resolve_path_access(
            "../../outside",
            "/repo/work",
            &workspace,
            PathAccessOperation::Search,
            WorkspaceAccessPolicy {
                guard_mode: WorkspaceGuardMode::Disabled,
                check_sensitive: false,
            },
            PathClass::Posix,
            None,
        )
        .unwrap();
        assert_eq!(unguarded.path, "/outside");
        assert!(unguarded.outside_workspace);
    }

    #[test]
    fn convenience_entry_points_preserve_home_and_sensitive_options() {
        let workspace = WorkspaceConfig {
            workspace_dir: "/home/user".into(),
            additional_dirs: Vec::new(),
        };
        let environment = HostEnvironmentInfo {
            os_kind: "Linux".into(),
            os_arch: "x64".into(),
            os_version: "test".into(),
            shell_name: ShellName::Bash,
            shell_path: "/bin/bash".into(),
            path_class: PathClass::Posix,
            home_dir: "/home/user".into(),
        };
        assert_eq!(
            resolve_path_access_path(
                "~/notes",
                &environment,
                &workspace,
                PathAccessOperation::Read,
                DEFAULT_WORKSPACE_ACCESS_POLICY,
                true,
            )
            .unwrap(),
            "/home/user/notes"
        );
        assert_eq!(
            assert_path_allowed(
                ".env",
                "/home/user",
                &workspace,
                PathAccessOperation::Read,
                Some(false),
                PathClass::Posix,
            )
            .unwrap(),
            "/home/user/.env"
        );
    }
}
