//! Stdio MCP client process setup helpers.
//!
//! Original: `agent/mcp/client-stdio.ts`:
//! `BoundedTail`, `resolveStdioCwd()`, and `mergeStdioEnv()`.

use std::path::{Component, Path, PathBuf};

use crate::_base::utils::proxy::{Env, proxy_env_for_child, reconcile_child_no_proxy};

pub const STDERR_BUFFER_CAPACITY: usize = 4 * 1024;

/// A bounded stderr tail used when a child MCP server closes unexpectedly.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BoundedTail {
    capacity: usize,
    buffer: String,
}

impl BoundedTail {
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity,
            buffer: String::new(),
        }
    }

    // Original: BoundedTail.push().
    pub fn push(&mut self, chunk: &str) {
        self.buffer.push_str(chunk);
        let character_count = self.buffer.chars().count();
        if character_count > self.capacity {
            self.buffer = self
                .buffer
                .chars()
                .skip(character_count - self.capacity)
                .collect();
        }
    }

    // Original: BoundedTail.snapshot().
    pub fn snapshot(&self) -> String {
        self.buffer.clone()
    }
}

// Original: resolveStdioCwd(). `PathBuf` makes the resolved value directly
// usable by Tokio's child-process command builder.
pub fn resolve_stdio_cwd(config_cwd: Option<&str>, default_cwd: Option<&Path>) -> Option<PathBuf> {
    let config_cwd = config_cwd?;
    let config_path = Path::new(config_cwd);
    if config_path.is_absolute() {
        return Some(config_path.into());
    }

    let Some(default_cwd) = default_cwd else {
        return Some(config_path.into());
    };
    let base = if default_cwd.is_absolute() {
        default_cwd.into()
    } else {
        std::env::current_dir()
            .unwrap_or_default()
            .join(default_cwd)
    };
    Some(normalize_lexically(&base.join(config_path)))
}

fn normalize_lexically(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            Component::RootDir | Component::Prefix(_) | Component::Normal(_) => {
                normalized.push(component.as_os_str());
            }
        }
    }
    normalized
}

// Original: mergeStdioEnv().
pub fn merge_stdio_env(config_env: Option<&Env>) -> Env {
    merge_stdio_env_with_parent(config_env, std::env::vars())
}

fn merge_stdio_env_with_parent(
    config_env: Option<&Env>,
    parent_env: impl IntoIterator<Item = (String, String)>,
) -> Env {
    let mut merged: Env = parent_env.into_iter().collect();
    if let Some(config_env) = config_env {
        merged.extend(config_env.clone());
    }
    merged.extend(proxy_env_for_child(&merged));
    reconcile_child_no_proxy(&mut merged, config_env);
    merged
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(entries: &[(&str, &str)]) -> Env {
        entries
            .iter()
            .map(|(key, value)| ((*key).into(), (*value).into()))
            .collect()
    }

    #[test]
    fn retains_only_the_configured_stderr_tail() {
        let mut tail = BoundedTail::new(4);
        tail.push("ab");
        tail.push("cdef");
        assert_eq!(tail.snapshot(), "cdef");
    }

    #[test]
    fn resolves_relative_stdio_cwd_against_default_cwd() {
        assert_eq!(
            resolve_stdio_cwd(
                Some("servers/../mcp"),
                Some(Path::new("/workspace/project"))
            ),
            Some(PathBuf::from("/workspace/project/mcp"))
        );
        assert_eq!(
            resolve_stdio_cwd(Some("/tmp/mcp"), Some(Path::new("/workspace/project"))),
            Some(PathBuf::from("/tmp/mcp"))
        );
        assert_eq!(
            resolve_stdio_cwd(None, Some(Path::new("/workspace/project"))),
            None
        );
    }

    #[test]
    fn merges_overrides_and_reconciles_proxy_environment() {
        let config = env(&[
            ("HTTP_PROXY", "http://configured:8080"),
            ("NO_PROXY", "example.com"),
        ]);
        let result = merge_stdio_env_with_parent(
            Some(&config),
            env(&[("HTTP_PROXY", "http://parent:8080"), ("PATH", "/bin")]),
        );
        assert_eq!(result.get("PATH").map(String::as_str), Some("/bin"));
        assert_eq!(
            result.get("HTTP_PROXY").map(String::as_str),
            Some("http://configured:8080")
        );
        assert_eq!(
            result.get("NO_PROXY").map(String::as_str),
            Some("example.com,localhost,127.0.0.1,::1,[::1]")
        );
        assert_eq!(result.get("no_proxy"), result.get("NO_PROXY"));
        assert_eq!(
            result.get("NODE_USE_ENV_PROXY").map(String::as_str),
            Some("1")
        );
    }
}
