//! Skill-root path resolution primitives.
//!
//! Original: `packages/agent-core-v2/src/app/skillCatalog/skillRoots.ts`.

use std::{
    io,
    path::{Path, PathBuf},
};

use super::types::{SkillRoot, SkillSource};

const USER_BRAND_DIRS: &[&str] = &["skills"];
const USER_GENERIC_DIRS: &[&str] = &[".agents/skills"];
const PROJECT_BRAND_DIRS: &[&str] = &[".kimi-code/skills"];
const PROJECT_GENERIC_DIRS: &[&str] = &[".agents/skills"];

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct SkillRootsOptions {
    pub merge_all_available_skills: Option<bool>,
}

// Original: userRoots(). Filesystem waits remain asynchronous.
pub async fn user_roots(
    home_dir: &Path,
    os_home_dir: &Path,
    options: SkillRootsOptions,
) -> io::Result<Vec<SkillRoot>> {
    let mut roots = Vec::new();
    let merge_all_available_skills = options.merge_all_available_skills.unwrap_or(true);
    push_brand_group(
        &mut roots,
        USER_BRAND_DIRS,
        home_dir,
        SkillSource::User,
        merge_all_available_skills,
    )
    .await?;
    push_first_existing(
        &mut roots,
        USER_GENERIC_DIRS,
        os_home_dir,
        SkillSource::User,
    )
    .await?;
    Ok(roots)
}

// Original: projectRoots(). The starting directory is retained when no .git
// marker exists in it or any ancestor.
pub async fn project_roots(
    work_dir: &Path,
    options: SkillRootsOptions,
) -> io::Result<Vec<SkillRoot>> {
    let project_root = find_project_root(work_dir).await?;
    let mut roots = Vec::new();
    let merge_all_available_skills = options.merge_all_available_skills.unwrap_or(true);
    push_brand_group(
        &mut roots,
        PROJECT_BRAND_DIRS,
        &project_root,
        SkillSource::Project,
        merge_all_available_skills,
    )
    .await?;
    push_first_existing(
        &mut roots,
        PROJECT_GENERIC_DIRS,
        &project_root,
        SkillSource::Project,
    )
    .await?;
    Ok(roots)
}

// Original: configuredRoots(). Entries retain input order and missing paths are
// ignored independently.
pub async fn configured_roots(
    dirs: &[String],
    work_dir: &Path,
    os_home_dir: &Path,
    source: SkillSource,
) -> io::Result<Vec<SkillRoot>> {
    let project_root = find_project_root(work_dir).await?;
    let mut roots = Vec::new();
    for dir in dirs {
        let resolved = resolve_configured_dir(dir, &project_root, os_home_dir);
        push_existing_root(&mut roots, &resolved, source).await?;
    }
    Ok(roots)
}

async fn find_project_root(work_dir: &Path) -> io::Result<PathBuf> {
    let start = std::path::absolute(work_dir)?;
    let mut current = start.clone();
    loop {
        if exists(&current.join(".git")).await {
            return Ok(current);
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
    out: &mut Vec<SkillRoot>,
    dirs: &[&str],
    base: &Path,
    source: SkillSource,
) -> io::Result<()> {
    for dir in dirs {
        if push_existing_root(out, &base.join(dir), source).await? {
            return Ok(());
        }
    }
    Ok(())
}

async fn push_brand_group(
    out: &mut Vec<SkillRoot>,
    dirs: &[&str],
    base: &Path,
    source: SkillSource,
    merge_all_available_skills: bool,
) -> io::Result<()> {
    if !merge_all_available_skills {
        return push_first_existing(out, dirs, base, source).await;
    }
    for dir in dirs {
        push_existing_root(out, &base.join(dir), source).await?;
    }
    Ok(())
}

async fn push_existing_root(
    out: &mut Vec<SkillRoot>,
    dir: &Path,
    source: SkillSource,
) -> io::Result<bool> {
    if !is_dir(dir).await {
        return Ok(false);
    }
    let resolved = realpath(dir).await?;
    if !out.iter().any(|root| root.path == resolved) {
        out.push(SkillRoot {
            path: resolved,
            source,
            plugin: None,
        });
    }
    Ok(true)
}

fn resolve_configured_dir(dir: &str, project_root: &Path, os_home_dir: &Path) -> PathBuf {
    if dir == "~" {
        return os_home_dir.to_path_buf();
    }
    if let Some(relative) = dir.strip_prefix("~/") {
        return os_home_dir.join(relative);
    }
    let path = Path::new(dir);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        project_root.join(path)
    }
}

async fn is_dir(path: &Path) -> bool {
    tokio::fs::metadata(path)
        .await
        .is_ok_and(|metadata| metadata.is_dir())
}

async fn realpath(path: &Path) -> io::Result<String> {
    Ok(tokio::fs::canonicalize(path)
        .await?
        .to_string_lossy()
        .replace('\\', "/"))
}

async fn exists(path: &Path) -> bool {
    tokio::fs::metadata(path).await.is_ok()
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    fn temp_dir() -> PathBuf {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        std::env::temp_dir().join(format!(
            "kimi-skill-roots-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ))
    }

    #[tokio::test]
    async fn user_roots_preserve_brand_then_generic_order_and_skip_missing() {
        let root = temp_dir();
        let home = root.join("kimi-home");
        let os_home = root.join("os-home");
        tokio::fs::create_dir_all(home.join("skills"))
            .await
            .unwrap();
        tokio::fs::create_dir_all(os_home.join(".agents/skills"))
            .await
            .unwrap();

        let roots = user_roots(&home, &os_home, SkillRootsOptions::default())
            .await
            .unwrap();
        assert_eq!(roots.len(), 2);
        assert_eq!(roots[0].source, SkillSource::User);
        assert!(roots[0].path.ends_with("/kimi-home/skills"));
        assert!(roots[1].path.ends_with("/os-home/.agents/skills"));

        tokio::fs::remove_dir_all(root).await.unwrap();
    }

    #[tokio::test]
    async fn project_and_configured_roots_use_git_ancestor_and_expand_home() {
        let root = temp_dir();
        let project = root.join("project");
        let work_dir = project.join("nested/work");
        let os_home = root.join("home");
        tokio::fs::create_dir_all(project.join(".git"))
            .await
            .unwrap();
        tokio::fs::create_dir_all(project.join(".kimi-code/skills"))
            .await
            .unwrap();
        tokio::fs::create_dir_all(project.join("relative-skills"))
            .await
            .unwrap();
        tokio::fs::create_dir_all(os_home.join("home-skills"))
            .await
            .unwrap();
        tokio::fs::create_dir_all(&work_dir).await.unwrap();

        let project_result = project_roots(&work_dir, SkillRootsOptions::default())
            .await
            .unwrap();
        assert_eq!(project_result.len(), 1);
        assert!(
            project_result[0]
                .path
                .ends_with("/project/.kimi-code/skills")
        );

        let configured = configured_roots(
            &[
                "relative-skills".into(),
                "~/home-skills".into(),
                "missing".into(),
            ],
            &work_dir,
            &os_home,
            SkillSource::Extra,
        )
        .await
        .unwrap();
        assert_eq!(configured.len(), 2);
        assert!(configured[0].path.ends_with("/project/relative-skills"));
        assert!(configured[1].path.ends_with("/home/home-skills"));
        assert!(
            configured
                .iter()
                .all(|root| root.source == SkillSource::Extra)
        );

        tokio::fs::remove_dir_all(root).await.unwrap();
    }
}
