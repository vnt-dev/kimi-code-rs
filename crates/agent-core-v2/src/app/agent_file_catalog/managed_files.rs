//! Safe CRUD helpers for UI-managed Markdown agent files.

use std::{
    collections::VecDeque,
    path::{Component, Path, PathBuf},
};

use tokio::fs;

use crate::_base::utils::fs::atomic_write;

use super::{AgentFileDefinition, AgentFileSource, ParseAgentFileOptions, parse_agent_file_text};

const MAX_MANAGED_AGENT_SCAN_DEPTH: usize = 8;

#[derive(Clone, Debug)]
pub struct ManagedAgentFile {
    pub relative_path: String,
    pub path: String,
    pub content: String,
    pub definition: Option<AgentFileDefinition>,
    pub error: Option<String>,
}

/// List every Markdown file under a canonical managed root. Unlike runtime
/// discovery, this intentionally keeps invalid and duplicate definitions so a
/// user can open and repair them from the management UI.
pub async fn list_managed_agent_files(
    root: &Path,
    source: AgentFileSource,
) -> Result<Vec<ManagedAgentFile>, String> {
    if !path_exists(root).await? {
        return Ok(Vec::new());
    }

    let metadata = fs::metadata(root).await.map_err(|error| {
        format!(
            "Failed to inspect agent directory {}: {error}",
            root.display()
        )
    })?;
    if !metadata.is_dir() {
        return Err(format!("Agent path {} is not a directory.", root.display()));
    }

    let mut pending = VecDeque::from([(root.to_path_buf(), 0usize)]);
    let mut files = Vec::new();
    while let Some((directory, depth)) = pending.pop_front() {
        if depth > MAX_MANAGED_AGENT_SCAN_DEPTH {
            continue;
        }
        let mut entries = fs::read_dir(&directory).await.map_err(|error| {
            format!(
                "Failed to read agent directory {}: {error}",
                directory.display()
            )
        })?;
        let mut paths = Vec::new();
        while let Some(entry) = entries.next_entry().await.map_err(|error| {
            format!(
                "Failed to read agent directory {}: {error}",
                directory.display()
            )
        })? {
            paths.push(entry.path());
        }
        paths.sort();

        for path in paths {
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default();
            if name.starts_with('.') || name == "node_modules" {
                continue;
            }
            let metadata = fs::symlink_metadata(&path).await.map_err(|error| {
                format!("Failed to inspect agent path {}: {error}", path.display())
            })?;
            if metadata.file_type().is_symlink() {
                continue;
            }
            if metadata.is_dir() {
                pending.push_back((path, depth + 1));
                continue;
            }
            if !metadata.is_file()
                || path.extension().and_then(|value| value.to_str()) != Some("md")
            {
                continue;
            }

            files.push(read_managed_agent_file(root, &path, source).await?);
        }
    }
    files.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
    Ok(files)
}

pub async fn save_managed_agent_file(
    root: &Path,
    source: AgentFileSource,
    relative_path: Option<&str>,
    content: &str,
) -> Result<ManagedAgentFile, String> {
    if content.trim().is_empty() {
        return Err("Agent Markdown must not be empty.".into());
    }

    let creating = relative_path.is_none();
    let provisional_path = relative_path.unwrap_or("custom-agent.md");
    let provisional_display = root
        .join(provisional_path)
        .to_string_lossy()
        .replace('\\', "/");
    let definition = parse_agent_file_text(ParseAgentFileOptions {
        path: &provisional_display,
        source,
        text: content,
    })
    .map_err(|error| error.to_string())?;
    let relative_path = relative_path
        .map(str::to_owned)
        .unwrap_or_else(|| format!("{}.md", definition.name));
    let relative = validate_relative_markdown_path(&relative_path)?;

    fs::create_dir_all(root).await.map_err(|error| {
        format!(
            "Failed to create agent directory {}: {error}",
            root.display()
        )
    })?;
    let canonical_root = fs::canonicalize(root).await.map_err(|error| {
        format!(
            "Failed to resolve agent directory {}: {error}",
            root.display()
        )
    })?;
    let target = root.join(&relative);
    let parent = target.parent().unwrap_or(root);
    fs::create_dir_all(parent).await.map_err(|error| {
        format!(
            "Failed to create agent directory {}: {error}",
            parent.display()
        )
    })?;
    let canonical_parent = fs::canonicalize(parent).await.map_err(|error| {
        format!(
            "Failed to resolve agent directory {}: {error}",
            parent.display()
        )
    })?;
    if !canonical_parent.starts_with(&canonical_root) {
        return Err("Agent file must stay inside the managed agent directory.".into());
    }
    if let Ok(metadata) = fs::symlink_metadata(&target).await
        && metadata.file_type().is_symlink()
    {
        return Err("Refusing to overwrite a symbolic-link agent file.".into());
    }
    if creating && fs::symlink_metadata(&target).await.is_ok() {
        return Err(format!(
            "Agent `{}` already exists in this scope.",
            definition.name
        ));
    }

    atomic_write(&target, content.as_bytes(), Some(0o600))
        .await
        .map_err(|error| format!("Failed to save agent file {}: {error}", target.display()))?;
    read_managed_agent_file(root, &target, source).await
}

pub async fn delete_managed_agent_file(root: &Path, relative_path: &str) -> Result<(), String> {
    let relative = validate_relative_markdown_path(relative_path)?;
    if !path_exists(root).await? {
        return Err("The managed agent directory does not exist.".into());
    }
    let canonical_root = fs::canonicalize(root).await.map_err(|error| {
        format!(
            "Failed to resolve agent directory {}: {error}",
            root.display()
        )
    })?;
    let target = root.join(relative);
    let metadata = fs::symlink_metadata(&target)
        .await
        .map_err(|error| format!("Failed to inspect agent file {}: {error}", target.display()))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err("Only regular Markdown agent files can be deleted.".into());
    }
    let canonical_target = fs::canonicalize(&target)
        .await
        .map_err(|error| format!("Failed to resolve agent file {}: {error}", target.display()))?;
    if !canonical_target.starts_with(&canonical_root) {
        return Err("Agent file must stay inside the managed agent directory.".into());
    }
    fs::remove_file(&target)
        .await
        .map_err(|error| format!("Failed to delete agent file {}: {error}", target.display()))
}

async fn read_managed_agent_file(
    root: &Path,
    path: &Path,
    source: AgentFileSource,
) -> Result<ManagedAgentFile, String> {
    let relative_path = path
        .strip_prefix(root)
        .map_err(|_| "Agent file escaped its managed directory.".to_owned())?
        .to_string_lossy()
        .replace('\\', "/");
    let display_path = path.to_string_lossy().replace('\\', "/");
    let content = fs::read_to_string(path)
        .await
        .map_err(|error| format!("Failed to read agent file {}: {error}", path.display()))?;
    match parse_agent_file_text(ParseAgentFileOptions {
        path: &display_path,
        source,
        text: &content,
    }) {
        Ok(definition) => Ok(ManagedAgentFile {
            relative_path,
            path: display_path,
            content,
            definition: Some(definition),
            error: None,
        }),
        Err(error) => Ok(ManagedAgentFile {
            relative_path,
            path: display_path,
            content,
            definition: None,
            error: Some(error.to_string()),
        }),
    }
}

fn validate_relative_markdown_path(value: &str) -> Result<PathBuf, String> {
    let value = value.trim();
    let path = Path::new(value);
    if value.is_empty()
        || path.extension().and_then(|extension| extension.to_str()) != Some("md")
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err("Agent path must be a relative .md path inside the managed directory.".into());
    }
    Ok(path.to_path_buf())
}

async fn path_exists(path: &Path) -> Result<bool, String> {
    match fs::metadata(path).await {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(format!("Failed to inspect {}: {error}", path.display())),
    }
}

#[cfg(test)]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

    fn temp_root() -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "kimi-managed-agents-{}-{nonce}",
            std::process::id()
        ))
    }

    #[tokio::test]
    async fn saves_lists_invalid_files_and_deletes_managed_agents() {
        let root = temp_root();
        let valid = "---\nname: reviewer\ndescription: Reviews code\n---\n${base_prompt}\n\nReview carefully.\n";
        let saved = save_managed_agent_file(&root, AgentFileSource::User, None, valid)
            .await
            .unwrap();
        assert_eq!(saved.relative_path, "reviewer.md");
        assert_eq!(saved.definition.unwrap().name, "reviewer");

        fs::write(root.join("broken.md"), "not frontmatter")
            .await
            .unwrap();
        let listed = list_managed_agent_files(&root, AgentFileSource::User)
            .await
            .unwrap();
        assert_eq!(listed.len(), 2);
        assert!(listed.iter().any(|file| file.error.is_some()));

        delete_managed_agent_file(&root, "reviewer.md")
            .await
            .unwrap();
        assert!(!root.join("reviewer.md").exists());
        fs::remove_dir_all(root).await.unwrap();
    }

    #[tokio::test]
    async fn rejects_invalid_content_and_paths_outside_the_root() {
        let root = temp_root();
        let invalid = save_managed_agent_file(
            &root,
            AgentFileSource::Project,
            None,
            "---\ndescription: Missing body\n---\n",
        )
        .await
        .unwrap_err();
        assert!(invalid.contains("Missing prompt body"));
        assert!(
            delete_managed_agent_file(&root, "../escape.md")
                .await
                .is_err()
        );
    }
}
