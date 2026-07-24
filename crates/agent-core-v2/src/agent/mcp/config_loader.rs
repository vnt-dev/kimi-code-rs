//! MCP JSON configuration discovery and loading.
//!
//! Original: `agent/mcp/config-loader.ts`.

use std::{
    collections::HashMap,
    path::{Component, Path, PathBuf},
};

use serde::Deserialize;
use tokio::fs;

use super::{McpServerConfig, parse_mcp_server_config};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct McpJsonPaths {
    pub user: PathBuf,
    pub project_root: PathBuf,
    pub project: PathBuf,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolveMcpJsonPathsInput {
    pub cwd: PathBuf,
    pub home_dir: Option<PathBuf>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LoadMcpServersInput {
    pub cwd: PathBuf,
    pub home_dir: Option<PathBuf>,
}

#[derive(Debug, thiserror::Error)]
pub enum McpConfigLoadError {
    #[error("Failed to resolve MCP configuration paths from {cwd}: {source}")]
    ResolvePaths {
        cwd: String,
        #[source]
        source: std::io::Error,
    },
    #[error("Failed to read {path}: {source}")]
    Read {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("Invalid JSON in {path}: {message}")]
    Json { path: String, message: String },
    #[error("Invalid MCP server config in {path}: {message}")]
    ServerConfig { path: String, message: String },
    #[error("unable to resolve the current user's home directory")]
    HomeDirectoryUnavailable,
}

#[derive(Deserialize)]
struct McpJsonFile {
    #[serde(rename = "mcpServers", default)]
    mcp_servers: HashMap<String, serde_json::Value>,
}

// Original: resolveMcpJsonPaths(). Filesystem metadata is asynchronous.
pub async fn resolve_mcp_json_paths(
    input: &ResolveMcpJsonPathsInput,
) -> Result<McpJsonPaths, McpConfigLoadError> {
    let project_root = find_project_root(&input.cwd).await?;
    let home_dir = resolve_kimi_home(input.home_dir.as_deref())?;
    Ok(McpJsonPaths {
        user: home_dir.join("mcp.json"),
        project_root: project_root.join(".mcp.json"),
        project: input.cwd.join(".kimi-code/mcp.json"),
    })
}

// Original: loadMcpServers(). Later layers overwrite earlier server names.
pub async fn load_mcp_servers(
    input: &LoadMcpServersInput,
) -> Result<HashMap<String, McpServerConfig>, McpConfigLoadError> {
    let paths = resolve_mcp_json_paths(&ResolveMcpJsonPathsInput {
        cwd: input.cwd.clone(),
        home_dir: input.home_dir.clone(),
    })
    .await?;
    let (user, project_root, project) = tokio::try_join!(
        read_mcp_json(&paths.user, None),
        read_mcp_json(&paths.project_root, paths.project_root.parent()),
        read_mcp_json(&paths.project, None),
    )?;
    let mut merged = user;
    merged.extend(project_root);
    merged.extend(project);
    Ok(merged)
}

async fn find_project_root(cwd: &Path) -> Result<PathBuf, McpConfigLoadError> {
    let start = normalize_path(cwd);
    let mut current = start.clone();
    loop {
        if path_exists(&current.join(".git")).await.map_err(|source| {
            McpConfigLoadError::ResolvePaths {
                cwd: current.display().to_string(),
                source,
            }
        })? {
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

async fn path_exists(path: &Path) -> std::io::Result<bool> {
    match fs::metadata(path).await {
        Ok(_) => Ok(true),
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
            ) =>
        {
            Ok(false)
        }
        Err(error) => Err(error),
    }
}

fn resolve_kimi_home(home_dir: Option<&Path>) -> Result<PathBuf, McpConfigLoadError> {
    if let Some(home_dir) = home_dir {
        return Ok(home_dir.into());
    }
    if let Some(home_dir) = std::env::var_os("KIMI_CODE_HOME") {
        return Ok(PathBuf::from(home_dir));
    }
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .or_else(|| {
            #[cfg(unix)]
            {
                use uzers::os::unix::UserExt;
                uzers::get_user_by_uid(uzers::get_current_uid()).map(|user| user.home_dir().into())
            }
            #[cfg(not(unix))]
            {
                None
            }
        })
        .ok_or(McpConfigLoadError::HomeDirectoryUnavailable)?;
    Ok(home.join(".kimi-code"))
}

async fn read_mcp_json(
    path: &Path,
    stdio_cwd_base: Option<&Path>,
) -> Result<HashMap<String, McpServerConfig>, McpConfigLoadError> {
    let text = match fs::read_to_string(path).await {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(HashMap::new()),
        Err(source) => {
            return Err(McpConfigLoadError::Read {
                path: path.display().to_string(),
                source,
            });
        }
    };
    if text.trim().is_empty() {
        return Ok(HashMap::new());
    }
    let file =
        serde_json::from_str::<McpJsonFile>(&text).map_err(|error| McpConfigLoadError::Json {
            path: path.display().to_string(),
            message: error.to_string(),
        })?;
    file.mcp_servers
        .into_iter()
        .map(|(name, raw)| {
            let config = parse_mcp_server_config(&raw).map_err(|error| {
                McpConfigLoadError::ServerConfig {
                    path: path.display().to_string(),
                    message: error.to_string(),
                }
            })?;
            Ok((name, normalize_stdio_cwd(config, stdio_cwd_base)))
        })
        .collect()
}

fn normalize_stdio_cwd(config: McpServerConfig, base: Option<&Path>) -> McpServerConfig {
    let Some(base) = base else {
        return config;
    };
    let McpServerConfig::Stdio(mut stdio) = config else {
        return config;
    };
    stdio.cwd = Some(match stdio.cwd.take() {
        Some(cwd) => resolve_path(base, Path::new(&cwd)).display().to_string(),
        None => normalize_path(base).display().to_string(),
    });
    McpServerConfig::Stdio(stdio)
}

fn resolve_path(base: &Path, value: &Path) -> PathBuf {
    if value.is_absolute() {
        normalize_path(value)
    } else {
        normalize_path(&base.join(value))
    }
}

// Mirrors pathe's lexical normalize for the source's `.git` parent walk and
// stdio cwd resolution without requiring filesystem paths to already exist.
fn normalize_path(path: &Path) -> PathBuf {
    let mut output = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => output.push(prefix.as_os_str()),
            Component::RootDir => output.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                output.pop();
            }
            Component::Normal(segment) => output.push(segment),
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

    fn temporary_directory() -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("kimi-mcp-config-test-{nonce}"))
    }

    #[tokio::test]
    async fn loads_layers_in_order_and_normalizes_project_root_stdio_cwd() {
        let root = temporary_directory();
        let cwd = root.join("nested/project");
        let home = root.join("home");
        fs::create_dir_all(root.join(".git")).await.unwrap();
        fs::create_dir_all(cwd.join(".kimi-code")).await.unwrap();
        fs::create_dir_all(&home).await.unwrap();
        fs::write(
            home.join("mcp.json"),
            r#"{"mcpServers":{"same":{"command":"user"},"user":{"command":"user"}}}"#,
        )
        .await
        .unwrap();
        fs::write(
            root.join(".mcp.json"),
            r#"{"mcpServers":{"same":{"command":"root","cwd":"."},"root":{"command":"root"}}}"#,
        )
        .await
        .unwrap();
        fs::write(
            cwd.join(".kimi-code/mcp.json"),
            r#"{"mcpServers":{"same":{"command":"project"},"project":{"command":"project"}}}"#,
        )
        .await
        .unwrap();
        let servers = load_mcp_servers(&LoadMcpServersInput {
            cwd: cwd.clone(),
            home_dir: Some(home),
        })
        .await
        .unwrap();
        assert_eq!(servers.len(), 4);
        let McpServerConfig::Stdio(same) = &servers["same"] else {
            panic!("same must be stdio");
        };
        assert_eq!(same.command, "project");
        let McpServerConfig::Stdio(root_server) = &servers["root"] else {
            panic!("root must be stdio");
        };
        assert_eq!(
            root_server.cwd.as_deref(),
            Some(root.as_path().to_str().unwrap())
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn ignores_missing_and_empty_files_but_reports_invalid_server_entries() {
        let root = temporary_directory();
        let cwd = root.join("work");
        let home = root.join("home");
        fs::create_dir_all(&cwd).await.unwrap();
        fs::create_dir_all(&home).await.unwrap();
        assert!(
            load_mcp_servers(&LoadMcpServersInput {
                cwd: cwd.clone(),
                home_dir: Some(home.clone()),
            })
            .await
            .unwrap()
            .is_empty()
        );
        fs::write(
            home.join("mcp.json"),
            r#"{"mcpServers":{"bad":{"command":""}}}"#,
        )
        .await
        .unwrap();
        assert!(matches!(
            load_mcp_servers(&LoadMcpServersInput {
                cwd,
                home_dir: Some(home),
            })
            .await,
            Err(McpConfigLoadError::ServerConfig { .. })
        ));
        std::fs::remove_dir_all(root).unwrap();
    }
}
