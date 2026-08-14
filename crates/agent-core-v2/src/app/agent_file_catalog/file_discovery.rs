//! Filesystem-backed agent-file discovery.
//!
//! Original: `packages/agent-core-v2/src/app/agentFileCatalog/agentFileDiscovery.ts`.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use futures_util::future::BoxFuture;

use crate::os::interface::{
    host_file_system::HostFileSystemService,
    host_fs_errors::{HostFsError, OS_FS_NOT_DIRECTORY, OS_FS_NOT_FOUND, OS_FS_UNAVAILABLE},
};

use super::{
    AgentFileDefinition, AgentFileDiscoveryResult, AgentFileRoot, SkippedAgentFile,
    is_directory_path, is_file_path, parse_agent_file_text,
};

const MAX_AGENT_SCAN_DEPTH: usize = 8;
const MAX_SKIP_WARNINGS: usize = 5;

pub type DiscoverAgentFilesWarn<'a> = dyn Fn(&str, Option<&str>) + Send + Sync + 'a;

// Original: discoverAgentFiles(). This remains sequential: source root order
// determines which same-named definition wins.
pub async fn discover_agent_files(
    fs: &dyn HostFileSystemService,
    roots: &[AgentFileRoot],
    warn: Option<&DiscoverAgentFilesWarn<'_>>,
) -> Result<AgentFileDiscoveryResult, HostFsError> {
    let mut walker = AgentFileWalker {
        fs,
        by_name: HashMap::new(),
        skipped: Vec::new(),
        emitted_warnings: 0,
        suppressed_warnings: 0,
        suppressed_subjects: Vec::new(),
        warn,
    };
    for root in roots {
        match walker.walk(PathBuf::from(&root.path), root, 0).await {
            Ok(()) => {}
            Err(error) if is_unavailable(&error) => return Err(error),
            Err(error) => walker.warn_capped(
                &root.path,
                format!("Skipping unreadable agent root {}: {error}", root.path),
                Some(&error.to_string()),
            ),
        }
    }
    walker.emit_suppression_summary();

    let mut agents = walker.by_name.into_values().collect::<Vec<_>>();
    // Node's `localeCompare` is represented by deterministic Unicode scalar
    // ordering here, matching the existing Rust profile-catalog adaptation.
    agents.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(AgentFileDiscoveryResult {
        agents,
        skipped: walker.skipped,
        scanned_roots: roots.iter().map(|root| root.path.clone()).collect(),
    })
}

struct AgentFileWalker<'a> {
    fs: &'a dyn HostFileSystemService,
    by_name: HashMap<String, AgentFileDefinition>,
    skipped: Vec<SkippedAgentFile>,
    emitted_warnings: usize,
    suppressed_warnings: usize,
    suppressed_subjects: Vec<String>,
    warn: Option<&'a DiscoverAgentFilesWarn<'a>>,
}

impl AgentFileWalker<'_> {
    fn walk<'a>(
        &'a mut self,
        dir_path: PathBuf,
        root: &'a AgentFileRoot,
        depth: usize,
    ) -> BoxFuture<'a, Result<(), HostFsError>> {
        Box::pin(async move {
            if depth > MAX_AGENT_SCAN_DEPTH {
                return Ok(());
            }

            let mut entries = match self.fs.read_dir(&dir_path).await {
                Ok(entries) => entries
                    .into_iter()
                    .map(|entry| entry.name)
                    .collect::<Vec<_>>(),
                Err(error) if depth > 0 => {
                    self.warn_capped(
                        &dir_path.to_string_lossy(),
                        format!(
                            "Skipping unreadable directory {}: {error}",
                            dir_path.display()
                        ),
                        Some(&error.to_string()),
                    );
                    return Ok(());
                }
                Err(error) if is_missing_path(&error) => return Ok(()),
                Err(error) => return Err(error),
            };
            entries.sort();

            for entry in entries {
                if entry.starts_with('.') || entry == "node_modules" {
                    continue;
                }
                let entry_path = dir_path.join(&entry);
                let result: Result<(), HostFsError> = async {
                    if is_directory_path(self.fs, &entry_path).await? {
                        self.walk(entry_path.clone(), root, depth + 1).await
                    } else if !entry.ends_with(".md") || !is_file_path(self.fs, &entry_path).await?
                    {
                        Ok(())
                    } else {
                        self.parse_and_register(&entry_path, root).await
                    }
                }
                .await;
                if let Err(error) = result {
                    if is_unavailable(&error) {
                        return Err(error);
                    }
                    self.warn_capped(
                        &entry_path.to_string_lossy(),
                        format!(
                            "Skipping unreadable agent path {}: {error}",
                            entry_path.display()
                        ),
                        Some(&error.to_string()),
                    );
                }
            }
            Ok(())
        })
    }

    async fn parse_and_register(
        &mut self,
        file_path: &Path,
        root: &AgentFileRoot,
    ) -> Result<(), HostFsError> {
        let path = file_path.to_string_lossy().replace('\\', "/");
        let text = match self.fs.read_text(file_path, None).await {
            Ok(text) => text,
            Err(error) if is_unavailable(&error) => return Err(error),
            Err(error) => {
                self.warn_capped(
                    &path,
                    format!("Skipping agent file at {path} due to unexpected error"),
                    Some(&error.to_string()),
                );
                return Ok(());
            }
        };
        match parse_agent_file_text(super::ParseAgentFileOptions {
            path: &path,
            source: root.source,
            text: &text,
        }) {
            Ok(agent) => {
                self.by_name.entry(agent.name.clone()).or_insert(agent);
            }
            Err(error) => {
                self.skipped.push(SkippedAgentFile {
                    path: path.clone(),
                    reason: error.message.clone(),
                });
                self.warn_capped(
                    &path,
                    format!("Skipping invalid agent file at {path}: {}", error.message),
                    Some(&error.to_string()),
                );
            }
        }
        Ok(())
    }

    fn warn_capped(&mut self, subject: &str, message: String, error: Option<&str>) {
        if self.emitted_warnings < MAX_SKIP_WARNINGS {
            self.emitted_warnings += 1;
            if let Some(warn) = self.warn {
                warn(&message, error);
            }
        } else {
            self.suppressed_warnings += 1;
            if self.suppressed_subjects.len() < 3 {
                self.suppressed_subjects.push(subject.into());
            }
        }
    }

    fn emit_suppression_summary(&self) {
        if self.suppressed_warnings == 0 {
            return;
        }
        let examples = self
            .suppressed_subjects
            .iter()
            .map(|subject| format!("\"{subject}\""))
            .collect::<Vec<_>>()
            .join(", ");
        if let Some(warn) = self.warn {
            warn(
                &format!(
                    "Suppressed {} further agent-discovery skip warnings (e.g. {examples}); fix or remove the offending files/directories to silence them",
                    self.suppressed_warnings
                ),
                None,
            );
        }
    }
}

fn is_unavailable(error: &HostFsError) -> bool {
    error.code() == OS_FS_UNAVAILABLE
}

fn is_missing_path(error: &HostFsError) -> bool {
    matches!(error.code(), OS_FS_NOT_FOUND | OS_FS_NOT_DIRECTORY)
}

#[cfg(test)]
mod tests {
    use parking_lot::Mutex;
    use std::sync::Arc;
    use std::time::{SystemTime, UNIX_EPOCH};

    use crate::{
        app::agent_file_catalog::AgentFileSource,
        os::backends::node_local::host_fs_service::HostFileSystem,
    };

    use super::*;

    fn valid_agent(name: &str, description: &str) -> String {
        format!("---\nname: {name}\ndescription: {description}\n---\nPrompt\n")
    }

    #[tokio::test]
    async fn discovery_keeps_first_name_sorts_results_and_caps_warnings() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let base = std::env::temp_dir().join(format!(
            "kimi-agent-discovery-{}-{nonce}",
            std::process::id()
        ));
        let first = base.join("first");
        let second = base.join("second");
        tokio::fs::create_dir_all(&first).await.unwrap();
        tokio::fs::create_dir_all(second.join("node_modules"))
            .await
            .unwrap();
        tokio::fs::write(first.join("review.md"), valid_agent("review", "first"))
            .await
            .unwrap();
        tokio::fs::write(second.join("review.md"), valid_agent("review", "second"))
            .await
            .unwrap();
        tokio::fs::write(second.join(".hidden.md"), valid_agent("hidden", "hidden"))
            .await
            .unwrap();
        tokio::fs::write(
            second.join("node_modules/ignored.md"),
            valid_agent("ignored", "ignored"),
        )
        .await
        .unwrap();
        for index in 0..6 {
            tokio::fs::write(second.join(format!("invalid-{index}.md")), "not an agent")
                .await
                .unwrap();
        }

        let warnings = Arc::new(Mutex::new(Vec::new()));
        let captured = Arc::clone(&warnings);
        let warn = move |message: &str, _: Option<&str>| captured.lock().push(message.to_owned());
        let roots = vec![
            AgentFileRoot {
                path: first.to_string_lossy().into_owned(),
                source: AgentFileSource::User,
            },
            AgentFileRoot {
                path: second.to_string_lossy().into_owned(),
                source: AgentFileSource::Project,
            },
        ];
        let result = discover_agent_files(&HostFileSystem, &roots, Some(&warn))
            .await
            .unwrap();

        assert_eq!(result.agents.len(), 1);
        assert_eq!(result.agents[0].name, "review");
        assert_eq!(result.agents[0].description, "first");
        assert_eq!(result.skipped.len(), 6);
        {
            let warnings = warnings.lock();
            assert_eq!(warnings.len(), 6);
            assert!(
                warnings
                    .last()
                    .unwrap()
                    .starts_with("Suppressed 1 further agent-discovery")
            );
        }

        tokio::fs::remove_dir_all(base).await.unwrap();
    }
}
