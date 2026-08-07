//! Agent discovery-root resolution.
//!
//! Original: `packages/agent-core-v2/src/app/agentFileCatalog/agentRoots.ts`.

use std::{
    env,
    path::{Path, PathBuf},
};

use crate::os::interface::{
    host_file_system::HostFileSystemService,
    host_fs_errors::{HostFsError, OS_FS_UNAVAILABLE, to_host_fs_error},
};

use super::{AgentFileRoot, AgentFileSource, is_directory_path, path_exists, resolve_agent_path};

const USER_BRAND_DIRS: &[&str] = &["agents"];
const USER_GENERIC_DIRS: &[&str] = &[".agents/agents"];
const PROJECT_BRAND_DIRS: &[&str] = &[".kimi-code/agents"];
const PROJECT_GENERIC_DIRS: &[&str] = &[".agents/agents"];

pub type AgentRootWarn<'a> = dyn Fn(&str, Option<&HostFsError>) + Send + Sync + 'a;

// Original: userAgentRoots().
pub async fn user_agent_roots(
    fs: &dyn HostFileSystemService,
    home_dir: &Path,
    os_home_dir: &Path,
    warn: Option<&AgentRootWarn<'_>>,
) -> Result<Vec<AgentFileRoot>, HostFsError> {
    let mut roots = Vec::new();
    push_first_existing(
        fs,
        &mut roots,
        USER_BRAND_DIRS,
        home_dir,
        AgentFileSource::User,
        warn,
    )
    .await?;
    push_first_existing(
        fs,
        &mut roots,
        USER_GENERIC_DIRS,
        os_home_dir,
        AgentFileSource::User,
        warn,
    )
    .await?;
    Ok(roots)
}

// Original: projectAgentRoots().
pub async fn project_agent_roots(
    fs: &dyn HostFileSystemService,
    work_dir: &Path,
    warn: Option<&AgentRootWarn<'_>>,
) -> Result<Vec<AgentFileRoot>, HostFsError> {
    let project_root = find_project_root(fs, work_dir, warn).await?;
    let mut roots = Vec::new();
    push_first_existing(
        fs,
        &mut roots,
        PROJECT_BRAND_DIRS,
        &project_root,
        AgentFileSource::Project,
        warn,
    )
    .await?;
    push_first_existing(
        fs,
        &mut roots,
        PROJECT_GENERIC_DIRS,
        &project_root,
        AgentFileSource::Project,
        warn,
    )
    .await?;
    Ok(roots)
}

/// Resolve the repository root used by project-scoped agent discovery.
///
/// Management surfaces use this helper when they need to create the canonical
/// `.kimi-code/agents` directory before it exists. Keeping the resolution here
/// ensures reads and writes agree on what "current project" means.
pub async fn resolve_agent_project_root(
    fs: &dyn HostFileSystemService,
    work_dir: &Path,
    warn: Option<&AgentRootWarn<'_>>,
) -> Result<PathBuf, HostFsError> {
    find_project_root(fs, work_dir, warn).await
}

// Original: configuredAgentRoots().
pub async fn configured_agent_roots(
    fs: &dyn HostFileSystemService,
    dirs: &[String],
    work_dir: &Path,
    os_home_dir: &Path,
    source: AgentFileSource,
    warn: Option<&AgentRootWarn<'_>>,
) -> Result<Vec<AgentFileRoot>, HostFsError> {
    let project_root = find_project_root(fs, work_dir, warn).await?;
    let mut roots = Vec::new();
    for dir in dirs {
        push_existing_root(
            fs,
            &mut roots,
            &resolve_agent_path(dir, &project_root, os_home_dir),
            source,
            warn,
        )
        .await?;
    }
    Ok(roots)
}

async fn find_project_root(
    fs: &dyn HostFileSystemService,
    work_dir: &Path,
    warn: Option<&AgentRootWarn<'_>>,
) -> Result<PathBuf, HostFsError> {
    let start = absolute_lexical(work_dir)?;
    let mut current = start.clone();
    loop {
        let marker = current.join(".git");
        match path_exists(fs, &marker).await {
            Ok(true) => return Ok(current),
            Ok(false) => {}
            Err(error) if is_unavailable(&error) => return Err(error),
            Err(error) => warn_error(
                warn,
                &format!(
                    "Skipping unreadable project marker {}: {error}",
                    marker.display()
                ),
                &error,
            ),
        }
        let Some(parent) = current.parent() else {
            return Ok(start);
        };
        if parent == current {
            return Ok(start);
        }
        current = parent.to_path_buf();
    }
}

async fn push_first_existing(
    fs: &dyn HostFileSystemService,
    out: &mut Vec<AgentFileRoot>,
    dirs: &[&str],
    base: &Path,
    source: AgentFileSource,
    warn: Option<&AgentRootWarn<'_>>,
) -> Result<(), HostFsError> {
    for dir in dirs {
        if push_existing_root(fs, out, &base.join(dir), source, warn).await? {
            return Ok(());
        }
    }
    Ok(())
}

async fn push_existing_root(
    fs: &dyn HostFileSystemService,
    out: &mut Vec<AgentFileRoot>,
    dir: &Path,
    source: AgentFileSource,
    warn: Option<&AgentRootWarn<'_>>,
) -> Result<bool, HostFsError> {
    match is_directory_path(fs, dir).await {
        Ok(false) => Ok(false),
        Ok(true) => match fs.real_path(dir).await {
            Ok(resolved) => {
                let path = resolved.replace('\\', "/");
                if !out.iter().any(|root| root.path == path) {
                    out.push(AgentFileRoot { path, source });
                }
                Ok(true)
            }
            Err(error) if is_unavailable(&error) => Err(error),
            Err(error) => {
                warn_error(
                    warn,
                    &format!("Skipping unreadable agent root {}: {error}", dir.display()),
                    &error,
                );
                Ok(false)
            }
        },
        Err(error) if is_unavailable(&error) => Err(error),
        Err(error) => {
            warn_error(
                warn,
                &format!("Skipping unreadable agent root {}: {error}", dir.display()),
                &error,
            );
            Ok(false)
        }
    }
}

fn absolute_lexical(path: &Path) -> Result<PathBuf, HostFsError> {
    if path.is_absolute() {
        return Ok(normalize_lexical(path));
    }
    let current = env::current_dir()
        .map_err(|error| to_host_fs_error(Box::new(error), &path.to_string_lossy(), "resolve"))?;
    Ok(normalize_lexical(&current.join(path)))
}

fn normalize_lexical(path: &Path) -> PathBuf {
    resolve_agent_path(&path.to_string_lossy(), Path::new("/"), Path::new("~"))
}

fn is_unavailable(error: &HostFsError) -> bool {
    error.code() == OS_FS_UNAVAILABLE
}

fn warn_error(warn: Option<&AgentRootWarn<'_>>, message: &str, error: &HostFsError) {
    if let Some(warn) = warn {
        warn(message, Some(error));
    }
}

#[cfg(test)]
mod tests {
    use std::{
        path::Path,
        time::{SystemTime, UNIX_EPOCH},
    };

    use crate::os::backends::node_local::host_fs_service::HostFileSystem;

    use super::*;

    #[tokio::test]
    async fn roots_keep_source_priority_and_dedupe_real_paths() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root =
            std::env::temp_dir().join(format!("kimi-agent-roots-{}-{nonce}", std::process::id()));
        let home = root.join("home");
        let os_home = root.join("os-home");
        let project = root.join("project");
        let work_dir = project.join("nested/work");
        tokio::fs::create_dir_all(home.join("agents"))
            .await
            .unwrap();
        tokio::fs::create_dir_all(os_home.join(".agents/agents"))
            .await
            .unwrap();
        tokio::fs::create_dir_all(project.join(".git"))
            .await
            .unwrap();
        tokio::fs::create_dir_all(project.join(".kimi-code/agents"))
            .await
            .unwrap();
        tokio::fs::create_dir_all(&work_dir).await.unwrap();

        let fs = HostFileSystem;
        let user = user_agent_roots(&fs, &home, &os_home, None).await.unwrap();
        assert_eq!(user.len(), 2);
        assert!(user.iter().all(|root| root.source == AgentFileSource::User));

        let project_roots = project_agent_roots(&fs, &work_dir, None).await.unwrap();
        assert_eq!(project_roots.len(), 1);
        assert_eq!(project_roots[0].source, AgentFileSource::Project);

        let configured = configured_agent_roots(
            &fs,
            &[
                ".kimi-code/agents".into(),
                ".kimi-code/agents/..//agents".into(),
            ],
            &work_dir,
            Path::new("/unused-home"),
            AgentFileSource::Extra,
            None,
        )
        .await
        .unwrap();
        assert_eq!(configured.len(), 1);
        assert_eq!(configured[0].source, AgentFileSource::Extra);

        tokio::fs::remove_dir_all(root).await.unwrap();
    }
}
