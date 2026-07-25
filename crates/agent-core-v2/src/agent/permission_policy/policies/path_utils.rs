//! Permission-policy file-access and local Git worktree helpers.
//!
//! Original: `agent/permissionPolicy/policies/path-utils.ts`.

use std::path::{Path, PathBuf};

use crate::{
    agent::tool_executor::ResolvedToolExecutionHookContext,
    tool::{
        ToolFileAccess, ToolFileAccessOperation, ToolResourceAccess,
        path_access::{PathClass, is_within_directory},
    },
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PermissionGitWorkTreeMarker {
    pub dot_git_path: String,
    pub control_dir_path: String,
}

pub fn file_accesses(context: &ResolvedToolExecutionHookContext) -> Vec<ToolFileAccess> {
    context
        .execution
        .accesses
        .as_ref()
        .map_or_else(Vec::new, |accesses| {
            accesses
                .iter()
                .filter_map(|access| match access {
                    ToolResourceAccess::File(access) => Some(access.clone()),
                    ToolResourceAccess::All => None,
                })
                .collect()
        })
}

pub fn write_file_accesses(context: &ResolvedToolExecutionHookContext) -> Vec<ToolFileAccess> {
    file_accesses(context)
        .into_iter()
        .filter(|access| {
            matches!(
                access.operation,
                ToolFileAccessOperation::Write | ToolFileAccessOperation::ReadWrite
            )
        })
        .collect()
}

pub fn writes_only_plan_file(
    context: &ResolvedToolExecutionHookContext,
    plan_file_path: &str,
) -> bool {
    let accesses = write_file_accesses(context);
    !accesses.is_empty() && accesses.iter().all(|access| access.path == plan_file_path)
}

pub fn has_git_path_component(target_path: &str, cwd: &str, path_class: PathClass) -> bool {
    relative_path_parts(target_path, cwd, path_class)
        .iter()
        .any(|part| part.eq_ignore_ascii_case(".git"))
}

pub fn is_git_control_path(
    target_path: &str,
    marker: &PermissionGitWorkTreeMarker,
    path_class: PathClass,
) -> bool {
    is_within_directory(target_path, &marker.dot_git_path, path_class)
        || is_within_directory(target_path, &marker.control_dir_path, path_class)
}

pub const fn default_path_class() -> PathClass {
    if cfg!(windows) {
        PathClass::Win32
    } else {
        PathClass::Posix
    }
}

// Original: findLocalGitWorkTreeMarker(). This is async because the marker
// probe performs filesystem metadata and (for worktrees) a small text read.
pub async fn find_local_git_work_tree_marker(cwd: &str) -> Option<PermissionGitWorkTreeMarker> {
    let cwd = Path::new(cwd);
    if cwd.as_os_str().is_empty() || !cwd.is_absolute() {
        return None;
    }
    let mut current = cwd.to_path_buf();
    for _ in 0..256 {
        let dot_git = current.join(".git");
        if let Some(marker) = probe_local_git_marker(&dot_git, &current).await {
            return Some(marker);
        }
        let parent = current.parent()?;
        if parent == current {
            return None;
        }
        current = parent.to_path_buf();
    }
    None
}

fn relative_path_parts(target_path: &str, cwd: &str, path_class: PathClass) -> Vec<String> {
    let target = target_path.replace('\\', "/");
    let cwd = cwd.replace('\\', "/").trim_end_matches('/').to_owned();
    let target = if path_class == PathClass::Win32 {
        target.to_ascii_lowercase()
    } else {
        target
    };
    let cwd = if path_class == PathClass::Win32 {
        cwd.to_ascii_lowercase()
    } else {
        cwd
    };
    let relative = target.strip_prefix(&(cwd + "/")).unwrap_or(&target);
    relative
        .split('/')
        .filter(|part| !part.is_empty())
        .map(str::to_owned)
        .collect()
}

async fn probe_local_git_marker(
    dot_git: &Path,
    marker_parent: &Path,
) -> Option<PermissionGitWorkTreeMarker> {
    let metadata = tokio::fs::metadata(dot_git).await.ok()?;
    if metadata.is_dir() {
        let path = dot_git.to_string_lossy().into_owned();
        return Some(PermissionGitWorkTreeMarker {
            dot_git_path: path.clone(),
            control_dir_path: path,
        });
    }
    if !metadata.is_file() {
        return None;
    }
    let content = tokio::fs::read_to_string(dot_git).await.ok()?;
    let control_dir_path = parse_local_git_dir(&content, marker_parent)?;
    Some(PermissionGitWorkTreeMarker {
        dot_git_path: dot_git.to_string_lossy().into_owned(),
        control_dir_path,
    })
}

fn parse_local_git_dir(content: &str, marker_parent: &Path) -> Option<String> {
    let content = content.strip_prefix('\u{feff}').unwrap_or(content);
    let line = content.trim_start().lines().next()?.trim();
    let raw = line.strip_prefix("gitdir:")?.trim();
    if raw.is_empty() {
        return None;
    }
    let path = Path::new(raw);
    let path: PathBuf = if path.is_absolute() {
        path.to_path_buf()
    } else {
        marker_parent.join(path)
    };
    Some(path.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_worktree_markers_and_git_path_components() {
        let marker_parent = Path::new("work").join("child");
        assert_eq!(
            parse_local_git_dir(
                "\u{feff} gitdir: ../repo/.git/worktrees/a\n",
                &marker_parent
            )
            .map(PathBuf::from),
            Some(marker_parent.join("../repo/.git/worktrees/a"))
        );
        assert!(parse_local_git_dir("not-a-marker", Path::new("/work")).is_none());
        assert!(has_git_path_component(
            "/work/repo/.git/config",
            "/work/repo",
            PathClass::Posix
        ));
        assert!(!has_git_path_component(
            "/work/repo/src/git.rs",
            "/work/repo",
            PathClass::Posix
        ));
    }
}
