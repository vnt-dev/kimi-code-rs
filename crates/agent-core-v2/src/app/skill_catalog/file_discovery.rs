//! Filesystem-backed skill discovery.
//!
//! Original: `packages/agent-core-v2/src/app/skillCatalog/fileSkillDiscovery.ts`.

use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::Arc,
};

use async_trait::async_trait;
use futures_util::future::BoxFuture;

use crate::_base::{
    di::{
        descriptors::SyncDescriptor,
        instantiation::ServicesAccessorExt,
        scope::{InstantiationType, LifecycleScope, register_scoped_service},
    },
    log::{LOG_SERVICE_ID, LogEntryError, LogPayload, LogServiceHandle},
};

use super::{
    discovery::{
        SKILL_DISCOVERY_SERVICE_ID, SkillDiscoveryContract, SkillDiscoveryHandle,
        SkillDiscoveryResult,
    },
    parser::{ParseSkillError, ParseSkillTextOptions, parse_skill_text},
    types::{SkillDefinition, SkillRoot, SkippedSkill, normalize_skill_name},
};

const MAX_SKILL_SCAN_DEPTH: usize = 8;
const NON_SKILL_MARKDOWN_FILES: &[&str] = &[
    "README.md",
    "CHANGELOG.md",
    "LICENSE.md",
    "CONTRIBUTING.md",
    "SECURITY.md",
    "CODE_OF_CONDUCT.md",
];

type WarnCallback<'a> = dyn Fn(&str, Option<LogPayload>) + Send + Sync + 'a;

pub struct FileSkillDiscovery {
    log: LogServiceHandle,
}

impl FileSkillDiscovery {
    pub fn new(log: LogServiceHandle) -> Self {
        Self { log }
    }
}

#[async_trait]
impl SkillDiscoveryContract for FileSkillDiscovery {
    // Original: FileSkillDiscovery.discover().
    async fn discover(&self, roots: &[SkillRoot]) -> SkillDiscoveryResult {
        discover_file_skills(
            roots,
            Some(&|message, payload| self.log.0.warn(message, payload)),
        )
        .await
    }
}

// Original: discoverFileSkills(). Per-directory filesystem errors and per-skill
// parse/read errors are isolated, so discovery always returns a result.
pub async fn discover_file_skills(
    roots: &[SkillRoot],
    warn: Option<&WarnCallback<'_>>,
) -> SkillDiscoveryResult {
    let mut walker = FileSkillWalker {
        by_discovery_key: HashMap::new(),
        skipped: Vec::new(),
        warn,
    };
    for root in roots {
        walker
            .walk_skill_dir(PathBuf::from(&root.path), root, true, 0, None)
            .await;
    }
    let mut skills = walker.by_discovery_key.into_values().collect::<Vec<_>>();
    skills.sort_by(|left, right| left.name.cmp(&right.name));
    SkillDiscoveryResult {
        skills,
        skipped: walker.skipped,
        scanned_roots: roots.iter().map(|root| root.path.clone()).collect(),
    }
}

struct FileSkillWalker<'a> {
    by_discovery_key: HashMap<String, SkillDefinition>,
    skipped: Vec<SkippedSkill>,
    warn: Option<&'a WarnCallback<'a>>,
}

impl FileSkillWalker<'_> {
    fn walk_skill_dir<'a>(
        &'a mut self,
        dir_path: PathBuf,
        root: &'a SkillRoot,
        is_top_level: bool,
        depth: usize,
        sub_skill_parent_name: Option<String>,
    ) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            if depth > MAX_SKILL_SCAN_DEPTH {
                return;
            }

            let Some(entries) = read_directory_names(&dir_path).await else {
                return;
            };
            let mut directory_skills = Vec::new();
            let mut directory_skill_names = HashSet::new();
            let mut subdirs = Vec::new();
            for entry in &entries {
                let entry_path = dir_path.join(entry);
                if is_file(&entry_path.join("SKILL.md")).await {
                    directory_skills.push(entry.clone());
                    directory_skill_names.insert(entry.clone());
                }
                if entry == "node_modules" || entry.starts_with('.') {
                    continue;
                }
                if is_dir(&entry_path).await {
                    subdirs.push(entry.clone());
                }
            }

            let mut allowed_sub_skill_bundles = HashMap::new();
            for entry in &directory_skills {
                let skill = self
                    .parse_and_register(
                        dir_path.join(entry).join("SKILL.md"),
                        entry,
                        root,
                        sub_skill_parent_name.as_deref(),
                    )
                    .await;
                if let Some(skill) = skill
                    && has_sub_skill_enabled(&skill)
                {
                    allowed_sub_skill_bundles.insert(entry.clone(), skill.name);
                }
            }

            if is_top_level {
                if root.plugin.is_some() {
                    let root_skill_md = dir_path.join("SKILL.md");
                    if is_file(&root_skill_md).await {
                        let skill_dir_name = dir_path
                            .file_name()
                            .map(|name| name.to_string_lossy().into_owned())
                            .unwrap_or_default();
                        self.parse_and_register(root_skill_md, &skill_dir_name, root, None)
                            .await;
                    }
                }

                for entry in &entries {
                    if is_non_skill_markdown_file(entry) {
                        continue;
                    }
                    let Some(skill_name) = entry.strip_suffix(".md") else {
                        continue;
                    };
                    if entry == "SKILL.md" || directory_skill_names.contains(skill_name) {
                        continue;
                    }
                    let skill_md_path = dir_path.join(entry);
                    if !is_file(&skill_md_path).await {
                        continue;
                    }
                    self.parse_and_register(skill_md_path, skill_name, root, None)
                        .await;
                }
            }

            for entry in subdirs {
                if directory_skill_names.contains(&entry)
                    && !allowed_sub_skill_bundles.contains_key(&entry)
                {
                    continue;
                }
                let next_parent = allowed_sub_skill_bundles
                    .get(&entry)
                    .cloned()
                    .or_else(|| sub_skill_parent_name.clone());
                self.walk_skill_dir(dir_path.join(&entry), root, false, depth + 1, next_parent)
                    .await;
            }
        })
    }

    async fn parse_and_register(
        &mut self,
        skill_md_path: PathBuf,
        skill_dir_name: &str,
        root: &SkillRoot,
        sub_skill_parent_name: Option<&str>,
    ) -> Option<SkillDefinition> {
        let path = normalized_path(&skill_md_path);
        let text = match tokio::fs::read_to_string(&skill_md_path).await {
            Ok(text) => text,
            Err(error) => {
                self.warn_unexpected(&path, &error.to_string());
                return None;
            }
        };
        let parsed = match parse_skill_text(ParseSkillTextOptions {
            skill_md_path: &path,
            skill_dir_name,
            source: root.source,
            text: &text,
        }) {
            Ok(skill) => skill,
            Err(ParseSkillError::Unsupported(error)) => {
                self.skipped.push(SkippedSkill {
                    path,
                    kind: error.skill_type.clone(),
                    reason: format!("unsupported skill type \"{}\"", error.skill_type),
                });
                return None;
            }
            Err(ParseSkillError::Parse(error)) => {
                if let Some(warn) = self.warn {
                    warn(
                        &format!("Skipping invalid skill at {path}: {}", error.message),
                        Some(LogPayload::Error(LogEntryError {
                            message: error.message,
                            stack: None,
                        })),
                    );
                }
                return None;
            }
        };

        let mut discovered = parsed;
        if let Some(parent_name) = sub_skill_parent_name {
            discovered.name = qualify_sub_skill_name(parent_name, &discovered.name);
            discovered.metadata.is_sub_skill = Some(true);
        }
        discovered.plugin = root.plugin.clone();
        let key = skill_discovery_key(root, &discovered.name);
        self.by_discovery_key
            .entry(key)
            .or_insert_with(|| discovered.clone());
        Some(discovered)
    }

    fn warn_unexpected(&self, path: &str, error: &str) {
        if let Some(warn) = self.warn {
            warn(
                &format!("Skipping skill at {path} due to unexpected error"),
                Some(LogPayload::Error(LogEntryError {
                    message: error.into(),
                    stack: None,
                })),
            );
        }
    }
}

fn skill_discovery_key(root: &SkillRoot, name: &str) -> String {
    let normalized_name = normalize_skill_name(name);
    root.plugin
        .as_ref()
        .map_or(normalized_name.clone(), |plugin| {
            format!("{}\0{normalized_name}", plugin.id)
        })
}

fn qualify_sub_skill_name(parent_name: &str, skill_name: &str) -> String {
    if skill_name == parent_name || skill_name.starts_with(&format!("{parent_name}.")) {
        skill_name.into()
    } else {
        format!("{parent_name}.{skill_name}")
    }
}

fn has_sub_skill_enabled(skill: &SkillDefinition) -> bool {
    let direct = skill
        .metadata
        .extra
        .get("has-sub-skill")
        .or_else(|| skill.metadata.extra.get("hasSubSkill"))
        .and_then(serde_json::Value::as_bool)
        == Some(true);
    let nested = skill
        .metadata
        .extra
        .get("metadata")
        .and_then(serde_json::Value::as_object)
        .and_then(|metadata| {
            metadata
                .get("has-sub-skill")
                .or_else(|| metadata.get("hasSubSkill"))
        })
        .and_then(serde_json::Value::as_bool)
        == Some(true);
    direct || nested
}

fn is_non_skill_markdown_file(entry: &str) -> bool {
    NON_SKILL_MARKDOWN_FILES
        .iter()
        .any(|name| entry.eq_ignore_ascii_case(name))
}

async fn read_directory_names(path: &Path) -> Option<Vec<String>> {
    let mut directory = tokio::fs::read_dir(path).await.ok()?;
    let mut entries = Vec::new();
    loop {
        match directory.next_entry().await {
            Ok(Some(entry)) => entries.push(entry.file_name().to_string_lossy().into_owned()),
            Ok(None) => break,
            Err(_) => return None,
        }
    }
    entries.sort_by(|left, right| left.encode_utf16().cmp(right.encode_utf16()));
    Some(entries)
}

async fn is_dir(path: &Path) -> bool {
    tokio::fs::metadata(path)
        .await
        .is_ok_and(|metadata| metadata.is_dir())
}

async fn is_file(path: &Path) -> bool {
    tokio::fs::metadata(path)
        .await
        .is_ok_and(|metadata| metadata.is_file())
}

fn normalized_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

pub fn register_file_skill_discovery() {
    register_scoped_service(
        LifecycleScope::App,
        SKILL_DISCOVERY_SERVICE_ID,
        SyncDescriptor::new(|accessor| {
            let log = accessor.get(LOG_SERVICE_ID)?;
            let service: Arc<dyn SkillDiscoveryContract> =
                Arc::new(FileSkillDiscovery::new((*log).clone()));
            Ok(SkillDiscoveryHandle(service))
        }),
        InstantiationType::Eager,
        "skillCatalog",
    );
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Mutex,
        atomic::{AtomicU64, Ordering},
    };

    use super::*;
    use crate::app::skill_catalog::types::{SkillPluginContext, SkillSource};

    fn temp_dir() -> PathBuf {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        std::env::temp_dir().join(format!(
            "kimi-file-skill-discovery-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ))
    }

    fn root(path: &Path) -> SkillRoot {
        SkillRoot {
            path: normalized_path(path),
            source: SkillSource::User,
            plugin: None,
        }
    }

    async fn write_skill(path: &Path, frontmatter: &str, body: &str) {
        tokio::fs::create_dir_all(path.parent().unwrap())
            .await
            .unwrap();
        tokio::fs::write(path, format!("---\n{frontmatter}\n---\n{body}"))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn discovers_bundles_flat_files_and_qualified_sub_skills() {
        let directory = temp_dir();
        write_skill(
            &directory.join("parent/SKILL.md"),
            "name: parent\ndescription: Parent\nhas-sub-skill: true",
            "parent body",
        )
        .await;
        write_skill(
            &directory.join("parent/child/SKILL.md"),
            "name: child\ndescription: Child",
            "child body",
        )
        .await;
        tokio::fs::write(directory.join("flat.md"), "flat body")
            .await
            .unwrap();
        write_skill(
            &directory.join("blocked/SKILL.md"),
            "name: blocked\ndescription: Blocked",
            "blocked body",
        )
        .await;
        write_skill(
            &directory.join("blocked/nested/SKILL.md"),
            "name: hidden\ndescription: Hidden",
            "hidden body",
        )
        .await;

        let result = discover_file_skills(&[root(&directory)], None).await;
        assert_eq!(
            result
                .skills
                .iter()
                .map(|skill| skill.name.as_str())
                .collect::<Vec<_>>(),
            ["blocked", "flat", "parent", "parent.child"]
        );
        let child = result
            .skills
            .iter()
            .find(|skill| skill.name == "parent.child")
            .unwrap();
        assert_eq!(child.metadata.is_sub_skill, Some(true));
        assert_eq!(result.scanned_roots, [normalized_path(&directory)]);

        tokio::fs::remove_dir_all(directory).await.unwrap();
    }

    #[tokio::test]
    async fn excludes_repository_docs_from_plugin_flat_skills() {
        let directory = temp_dir();
        write_skill(
            &directory.join("SKILL.md"),
            "name: plugin-root\ndescription: Plugin root",
            "plugin body",
        )
        .await;
        for entry in [
            "ReadMe.md",
            "changelog.md",
            "LICENSE.md",
            "Contributing.md",
            "SECURITY.md",
            "Code_of_Conduct.md",
        ] {
            tokio::fs::write(directory.join(entry), format!("# {entry}"))
                .await
                .unwrap();
        }
        tokio::fs::write(directory.join("flat.md"), "flat body")
            .await
            .unwrap();

        let plugin = SkillRoot {
            plugin: Some(SkillPluginContext {
                id: "plugin-a".into(),
                instructions: None,
            }),
            ..root(&directory)
        };
        let result = discover_file_skills(&[plugin], None).await;

        assert_eq!(
            result
                .skills
                .iter()
                .map(|skill| skill.name.as_str())
                .collect::<Vec<_>>(),
            ["flat", "plugin-root"]
        );

        tokio::fs::remove_dir_all(directory).await.unwrap();
    }

    #[tokio::test]
    async fn isolates_unsupported_and_invalid_skills_and_scopes_plugin_names() {
        let directory = temp_dir();
        write_skill(
            &directory.join("unsupported/SKILL.md"),
            "name: unsupported\ndescription: Unsupported\ntype: future",
            "body",
        )
        .await;
        tokio::fs::create_dir_all(directory.join("invalid"))
            .await
            .unwrap();
        tokio::fs::write(directory.join("invalid/SKILL.md"), "no frontmatter")
            .await
            .unwrap();
        write_skill(
            &directory.join("same/SKILL.md"),
            "name: same\ndescription: Same",
            "body",
        )
        .await;
        let warnings = Mutex::new(Vec::new());
        let warn = |message: &str, _payload: Option<LogPayload>| {
            warnings.lock().unwrap().push(message.to_owned());
        };
        let plain = root(&directory);
        let plugin = SkillRoot {
            plugin: Some(SkillPluginContext {
                id: "plugin-a".into(),
                instructions: None,
            }),
            ..plain.clone()
        };

        let result = discover_file_skills(&[plain, plugin], Some(&warn)).await;
        assert_eq!(result.skipped.len(), 2);
        assert!(result.skipped.iter().all(|skill| skill.kind == "future"));
        assert_eq!(
            result
                .skills
                .iter()
                .filter(|skill| skill.name == "same")
                .count(),
            2
        );
        assert!(result.skills.iter().any(|skill| {
            skill
                .plugin
                .as_ref()
                .is_some_and(|plugin| plugin.id == "plugin-a")
        }));
        assert_eq!(warnings.lock().unwrap().len(), 2);

        tokio::fs::remove_dir_all(directory).await.unwrap();
    }
}
