//! Shared path primitives for agent-file discovery.
//!
//! Original: `packages/agent-core-v2/src/app/agentFileCatalog/paths.ts`.

use std::path::{Component, Path, PathBuf};

use crate::os::interface::{
    host_file_system::HostFileSystemService,
    host_fs_errors::{HostFsError, OS_FS_NOT_DIRECTORY, OS_FS_NOT_FOUND},
};

// Original: resolveAgentPath(). `PathBuf` is used internally instead of the
// source string return value so callers cannot accidentally lose platform path
// semantics before passing the value to the host filesystem boundary.
pub fn resolve_agent_path(path: &str, base_dir: &Path, os_home_dir: &Path) -> PathBuf {
    let path = match path {
        "~" => return os_home_dir.to_path_buf(),
        _ => path.strip_prefix("~/").map_or_else(
            || PathBuf::from(path),
            |relative| os_home_dir.join(relative),
        ),
    };
    if path.is_absolute() {
        normalize_lexical(&path)
    } else {
        normalize_lexical(&base_dir.join(path))
    }
}

// Original: isDirectoryPath().
pub async fn is_directory_path(
    fs: &dyn HostFileSystemService,
    path: &Path,
) -> Result<bool, HostFsError> {
    let resolved = match fs.real_path(path).await {
        Ok(resolved) => resolved,
        Err(error) if is_missing_path_error(&error) => return Ok(false),
        Err(error) => return Err(error),
    };
    match fs.stat(Path::new(&resolved)).await {
        Ok(stat) => Ok(stat.is_directory),
        Err(error) if is_missing_path_error(&error) => Ok(false),
        Err(error) => Err(error),
    }
}

// Original: isFilePath().
pub async fn is_file_path(
    fs: &dyn HostFileSystemService,
    path: &Path,
) -> Result<bool, HostFsError> {
    let resolved = match fs.real_path(path).await {
        Ok(resolved) => resolved,
        Err(error) if is_missing_path_error(&error) => return Ok(false),
        Err(error) => return Err(error),
    };
    match fs.stat(Path::new(&resolved)).await {
        Ok(stat) => Ok(stat.is_file),
        Err(error) if is_missing_path_error(&error) => Ok(false),
        Err(error) => Err(error),
    }
}

// Original: pathExists(). Unlike the two typed probes, this deliberately
// stats the given path without resolving symlinks first.
pub async fn path_exists(fs: &dyn HostFileSystemService, path: &Path) -> Result<bool, HostFsError> {
    match fs.stat(path).await {
        Ok(_) => Ok(true),
        Err(error) if is_missing_path_error(&error) => Ok(false),
        Err(error) => Err(error),
    }
}

fn is_missing_path_error(error: &HostFsError) -> bool {
    matches!(error.code(), OS_FS_NOT_FOUND | OS_FS_NOT_DIRECTORY)
}

fn normalize_lexical(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(Path::new(std::path::MAIN_SEPARATOR_STR)),
            Component::CurDir => {}
            Component::ParentDir => {
                let can_pop = normalized
                    .file_name()
                    .is_some_and(|name| name != std::ffi::OsStr::new(".."));
                if can_pop {
                    normalized.pop();
                } else if !normalized.has_root() {
                    normalized.push("..");
                }
            }
            Component::Normal(component) => normalized.push(component),
        }
    }
    if normalized.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        normalized
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use crate::os::backends::node_local::host_fs_service::HostFileSystem;

    use super::*;

    #[test]
    fn resolve_agent_path_expands_home_and_normalizes_relative_paths() {
        let base = Path::new("/workspace/project");
        let home = Path::new("/home/kimi");
        assert_eq!(resolve_agent_path("~", base, home), home);
        assert_eq!(
            resolve_agent_path("~/agents", base, home),
            home.join("agents")
        );
        assert_eq!(
            resolve_agent_path("agents/../agents/review", base, home),
            base.join("agents/review")
        );
        assert_eq!(
            resolve_agent_path("/etc/../tmp", base, home),
            Path::new("/tmp")
        );
    }

    #[tokio::test]
    async fn probes_treat_missing_paths_as_false_and_preserve_file_kinds() {
        let fs = HostFileSystem;
        let manifest = Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
        let directory = Path::new(env!("CARGO_MANIFEST_DIR"));
        let missing = directory.join("agent-path-probe-does-not-exist");

        assert!(is_file_path(&fs, &manifest).await.unwrap());
        assert!(!is_directory_path(&fs, &manifest).await.unwrap());
        assert!(is_directory_path(&fs, directory).await.unwrap());
        assert!(!is_file_path(&fs, directory).await.unwrap());
        assert!(path_exists(&fs, &manifest).await.unwrap());
        assert!(!path_exists(&fs, &missing).await.unwrap());
    }
}
