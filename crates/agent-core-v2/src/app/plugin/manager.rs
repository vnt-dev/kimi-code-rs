//! Installed plugin state, lifecycle, and contribution management.
//!
//! Original: `packages/agent-core-v2/src/app/plugin/manager.ts`.

use std::{
    collections::HashMap,
    error::Error,
    path::{Path, PathBuf},
    sync::Arc,
    time::SystemTime,
};

use chrono::{DateTime, SecondsFormat, Utc};
use futures_util::future::join_all;
use indexmap::IndexMap;
use serde_json::{Map, Value};
use thiserror::Error;

use crate::{
    _base::errors::errors::{Error2, Error2Options},
    agent::{external_hooks::HookDef, mcp::McpServerConfig},
    app::skill_catalog::{
        SkillDiscoveryContract, SkillPluginContext, SkillRoot, SkillSource, discover_file_skills,
    },
};

use super::{
    archive::{download_zip_with_progress, extract_zip},
    commands::{LoadPluginCommandOptions, load_plugin_command},
    errors::{PLUGIN_NOT_FOUND, ensure_plugin_errors_registered},
    github_resolver::{GithubSourceInput, resolve_github_commit_sha, resolve_github_source},
    manifest::{ParsedManifestResult, parse_manifest},
    source::{ResolvedSource, resolve_install_source},
    store::{InstalledFile, InstalledRecord, read_installed, write_installed},
    types::{
        EnabledPluginSessionStart, PluginCapabilityState, PluginCommandDef,
        PluginDiagnosticSeverity, PluginGithubMetadata, PluginGithubRef, PluginGithubRefKind,
        PluginInfo, PluginInstallPhase, PluginInstallProgress, PluginInstallProgressCallback,
        PluginMcpServerInfo, PluginMcpServerState, PluginMcpTransport, PluginRecord, PluginSource,
        PluginState, PluginSummary, PluginUpdateStatus, ReloadError, ReloadSummary,
        normalize_plugin_id,
    },
};

pub type PluginManagerError = Box<dyn Error + Send + Sync>;
pub type PluginManagerResult<T> = Result<T, PluginManagerError>;

pub struct PluginManagerOptions {
    pub kimi_home_dir: String,
    pub discover_skills: Option<Arc<dyn SkillDiscoveryContract>>,
}

#[derive(Clone, Debug)]
struct ManagedPluginCopy {
    root: String,
    previous_root: Option<String>,
}

pub struct PluginManager {
    kimi_home_dir: String,
    discover_skills: Option<Arc<dyn SkillDiscoveryContract>>,
    records: IndexMap<String, PluginRecord>,
}

impl PluginManager {
    // Original: PluginManager.constructor().
    pub fn new(options: PluginManagerOptions) -> Self {
        Self {
            kimi_home_dir: options.kimi_home_dir,
            discover_skills: options.discover_skills,
            records: IndexMap::new(),
        }
    }

    // Original: PluginManager.load(). State commits only after every record
    // materializes successfully.
    pub async fn load(&mut self) -> PluginManagerResult<()> {
        let file = read_installed(&self.kimi_home_dir).await?;
        let mut next = IndexMap::new();
        for entry in file.plugins {
            let record = self.materialize(entry).await?;
            next.insert(record.id.clone(), record);
        }
        self.records = next;
        Ok(())
    }

    pub fn list(&self) -> Vec<PluginRecord> {
        let mut records = self.records.values().cloned().collect::<Vec<_>>();
        records.sort_by(|left, right| left.id.cmp(&right.id));
        records
    }

    pub fn get(&self, id: &str) -> Option<&PluginRecord> {
        self.records.get(&normalize_plugin_id(id))
    }

    // Original: PluginManager.install().
    pub async fn install(&mut self, source: &str) -> PluginManagerResult<PluginRecord> {
        self.install_with_progress(source, None).await
    }

    pub async fn install_with_progress(
        &mut self,
        source: &str,
        progress: Option<PluginInstallProgressCallback>,
    ) -> PluginManagerResult<PluginRecord> {
        emit_install_progress(progress.as_ref(), PluginInstallPhase::Resolving, 0, None);
        let resolved = resolve_install_source(source)?;
        let mut temporary_zip_dir = None;
        let prepared = self
            .prepare_install_source(source, resolved, &mut temporary_zip_dir, progress.as_ref())
            .await;
        let result = match prepared {
            Ok(prepared) => {
                emit_install_progress(progress.as_ref(), PluginInstallPhase::Installing, 0, None);
                self.install_prepared(prepared).await
            }
            Err(error) => Err(error),
        };
        if let Some(directory) = temporary_zip_dir {
            // Temporary cleanup is not part of the install commit. In
            // particular, a scanner holding a file open on Windows must not
            // make a successfully persisted plugin look failed to callers.
            let _ = remove_dir_all_force(Path::new(&directory)).await;
        }
        if result.is_ok() {
            emit_install_progress(progress.as_ref(), PluginInstallPhase::Complete, 0, None);
        }
        result
    }

    async fn prepare_install_source(
        &self,
        source: &str,
        resolved: ResolvedSource,
        temporary_zip_dir: &mut Option<String>,
        progress: Option<&PluginInstallProgressCallback>,
    ) -> PluginManagerResult<PreparedInstall> {
        match resolved {
            ResolvedSource::LocalPath { path } => Ok(PreparedInstall {
                source_root: normalize_install_root(&path).await?,
                original_source: path,
                source: PluginSource::LocalPath,
                github: None,
            }),
            ResolvedSource::ZipUrl { path } => {
                let download_progress = |downloaded_bytes, total_bytes| {
                    emit_install_progress(
                        progress,
                        PluginInstallPhase::Downloading,
                        downloaded_bytes,
                        total_bytes,
                    );
                };
                let buffer = download_zip_with_progress(&path, Some(&download_progress)).await?;
                emit_install_progress(progress, PluginInstallPhase::Extracting, 0, None);
                let directory = create_temporary_directory("kimi-plugin-zip-").await?;
                *temporary_zip_dir = Some(path_to_string(&directory));
                let source_root = extract_zip(buffer, &directory).await?;
                Ok(PreparedInstall {
                    source_root,
                    original_source: source.trim().to_owned(),
                    source: PluginSource::ZipUrl,
                    github: None,
                })
            }
            ResolvedSource::Github {
                owner,
                repo,
                reference,
            } => {
                let resolution = resolve_github_source(&GithubSourceInput {
                    owner: owner.clone(),
                    repo: repo.clone(),
                    reference,
                })
                .await?;
                let installed_sha =
                    installed_github_sha(&owner, &repo, &resolution.reference).await?;
                let github = PluginGithubMetadata {
                    owner: owner.clone(),
                    repo: repo.clone(),
                    reference: resolution.reference,
                    installed_sha: installed_sha.clone(),
                };
                let zip_url = installed_sha.map_or(resolution.tarball_url, |sha| {
                    format!("https://codeload.github.com/{owner}/{repo}/zip/{sha}")
                });
                let download_progress = |downloaded_bytes, total_bytes| {
                    emit_install_progress(
                        progress,
                        PluginInstallPhase::Downloading,
                        downloaded_bytes,
                        total_bytes,
                    );
                };
                let buffer = download_zip_with_progress(&zip_url, Some(&download_progress)).await?;
                emit_install_progress(progress, PluginInstallPhase::Extracting, 0, None);
                let directory = create_temporary_directory("kimi-plugin-zip-").await?;
                *temporary_zip_dir = Some(path_to_string(&directory));
                let source_root = extract_zip(buffer, &directory).await?;
                Ok(PreparedInstall {
                    source_root,
                    original_source: source.trim().to_owned(),
                    source: PluginSource::Github,
                    github: Some(github),
                })
            }
        }
    }

    async fn install_prepared(
        &mut self,
        prepared: PreparedInstall,
    ) -> PluginManagerResult<PluginRecord> {
        let parsed = parse_manifest(&prepared.source_root).await;
        let Some(manifest) = &parsed.manifest else {
            let message = parsed
                .diagnostics
                .iter()
                .find(|entry| entry.severity == PluginDiagnosticSeverity::Error)
                .map(|entry| entry.message.as_str())
                .unwrap_or("no manifest");
            let message = if prepared.source == PluginSource::LocalPath {
                format!(
                    "Cannot install plugin at {}: {message}",
                    prepared.source_root
                )
            } else {
                format!(
                    "Cannot install plugin from {}: {message}",
                    prepared.original_source
                )
            };
            return Err(message_error(message));
        };
        let id = normalize_plugin_id(&manifest.name);
        let previous_root = self.records.get(&id).map(|record| record.root.as_str());
        let managed_copy = copy_plugin_to_managed_root(
            &self.kimi_home_dir,
            &id,
            &prepared.source_root,
            previous_root,
        )
        .await?;
        let result = self
            .publish_prepared_record(&id, &managed_copy, prepared)
            .await;
        match result {
            Ok(record) => {
                if let Some(previous) = &managed_copy.previous_root {
                    let _ = remove_managed_plugin_root(&self.kimi_home_dir, previous).await;
                }
                Ok(record)
            }
            Err(error) => match rollback_managed_plugin_copy(&managed_copy).await {
                Ok(()) => Err(error),
                Err(rollback) => Err(message_error_with_source(
                    "Plugin installation failed and the previous managed copy could not be restored",
                    CombinedInstallError {
                        install: error,
                        rollback,
                    },
                )),
            },
        }
    }

    async fn publish_prepared_record(
        &mut self,
        id: &str,
        managed_copy: &ManagedPluginCopy,
        prepared: PreparedInstall,
    ) -> PluginManagerResult<PluginRecord> {
        let parsed = parse_manifest(&managed_copy.root).await;
        let existing = self.records.get(id);
        let now = now_iso();
        let record = self
            .record_from(RecordInput {
                id: id.to_owned(),
                root: managed_copy.root.clone(),
                enabled: existing.is_none_or(|record| record.enabled),
                installed_at: existing
                    .map(|record| record.installed_at.clone())
                    .unwrap_or_else(|| now.clone()),
                updated_at: Some(now),
                original_source: Some(prepared.original_source),
                capabilities: existing.and_then(|record| record.capabilities.clone()),
                github: prepared.github,
                source: prepared.source,
                parsed,
            })
            .await;
        let mut next = self.records.clone();
        next.insert(id.to_owned(), record.clone());
        self.persist(&next).await?;
        self.records = next;
        Ok(record)
    }

    pub async fn set_enabled(&mut self, id: &str, enabled: bool) -> PluginManagerResult<()> {
        let key = normalize_plugin_id(id);
        let Some(current) = self.records.get(&key) else {
            return Err(plugin_not_found(id));
        };
        if current.enabled == enabled {
            return Ok(());
        }
        let mut next = self.records.clone();
        let Some(record) = next.get_mut(&key) else {
            return Err(plugin_not_found(id));
        };
        record.enabled = enabled;
        record.updated_at = Some(now_iso());
        self.persist(&next).await?;
        self.records = next;
        Ok(())
    }

    pub async fn set_mcp_server_enabled(
        &mut self,
        id: &str,
        server: &str,
        enabled: bool,
    ) -> PluginManagerResult<()> {
        let key = normalize_plugin_id(id);
        let Some(current) = self.records.get(&key) else {
            return Err(plugin_not_found(id));
        };
        if current
            .manifest
            .as_ref()
            .and_then(|manifest| manifest.mcp_servers.as_ref())
            .is_none_or(|servers| !servers.contains_key(server))
        {
            return Err(message_error(format!(
                "Plugin \"{id}\" does not declare MCP server \"{server}\""
            )));
        }
        let mut next = self.records.clone();
        let Some(record) = next.get_mut(&key) else {
            return Err(plugin_not_found(id));
        };
        record
            .capabilities
            .get_or_insert_with(PluginCapabilityState::default)
            .mcp_servers
            .get_or_insert_with(HashMap::new)
            .insert(server.to_owned(), PluginMcpServerState { enabled });
        record.updated_at = Some(now_iso());
        self.persist(&next).await?;
        self.records = next;
        Ok(())
    }

    pub async fn remove(&mut self, id: &str) -> PluginManagerResult<()> {
        let key = normalize_plugin_id(id);
        let mut next = self.records.clone();
        let Some(removed) = next.shift_remove(&key) else {
            return Err(plugin_not_found(id));
        };
        self.persist(&next).await?;
        self.records = next;
        // Registry removal is the commit point. Managed files are no longer
        // reachable by the runtime, so cleanup is best-effort and cannot turn
        // a successful uninstall into a reported failure.
        let _ = remove_managed_plugin_root(&self.kimi_home_dir, &removed.root).await;
        Ok(())
    }

    pub async fn check_updates(&self) -> Vec<PluginUpdateStatus> {
        let futures = self
            .records
            .values()
            .filter(|record| record.source == PluginSource::Github && record.github.is_some())
            .cloned()
            .map(check_github_update);
        let mut results = join_all(futures)
            .await
            .into_iter()
            .filter_map(Result::ok)
            .collect::<Vec<_>>();
        results.sort_by(|left, right| left.id.cmp(&right.id));
        results
    }

    pub async fn reload(&mut self) -> PluginManagerResult<ReloadSummary> {
        let previous_ids = self.records.keys().cloned().collect::<Vec<_>>();
        let file = read_installed(&self.kimi_home_dir).await?;
        let mut next = IndexMap::new();
        let mut errors = Vec::new();
        for entry in file.plugins {
            let id = entry.id.clone();
            match self.materialize(entry).await {
                Ok(record) => {
                    next.insert(id, record);
                }
                Err(error) => errors.push(ReloadError {
                    id,
                    message: error.to_string(),
                }),
            }
        }
        let added = next
            .keys()
            .filter(|id| !self.records.contains_key(*id))
            .cloned()
            .collect();
        let removed = previous_ids
            .into_iter()
            .filter(|id| !next.contains_key(id))
            .collect();
        self.records = next;
        Ok(ReloadSummary {
            added,
            removed,
            errors,
        })
    }

    pub fn enabled_hooks(&self) -> Vec<HookDef> {
        let mut out = Vec::new();
        for record in self.active_records() {
            for hook in record
                .manifest
                .as_ref()
                .and_then(|manifest| manifest.hooks.as_ref())
                .into_iter()
                .flatten()
            {
                out.push(HookDef {
                    event: hook.event,
                    matcher: hook.matcher.clone(),
                    command: hook.command.clone(),
                    timeout: hook.timeout,
                    cwd: Some(record.root.clone()),
                    env: Some(HashMap::from([
                        ("KIMI_CODE_HOME".to_owned(), self.kimi_home_dir.clone()),
                        ("KIMI_PLUGIN_ROOT".to_owned(), record.root.clone()),
                    ])),
                });
            }
        }
        out
    }

    pub async fn enabled_commands(&self) -> Vec<PluginCommandDef> {
        let mut out = Vec::new();
        for record in self.active_records() {
            for entry in record
                .manifest
                .as_ref()
                .and_then(|manifest| manifest.commands.as_ref())
                .into_iter()
                .flatten()
            {
                if let Some(command) = load_plugin_command(LoadPluginCommandOptions {
                    command_path: &entry.path,
                    plugin_id: &record.id,
                    fallback_name: Some(&entry.name),
                })
                .await
                {
                    out.push(command);
                }
            }
        }
        out
    }

    pub fn plugin_skill_roots(&self) -> Vec<SkillRoot> {
        let mut roots = Vec::new();
        for record in self.active_records() {
            for directory in record
                .manifest
                .as_ref()
                .and_then(|manifest| manifest.skills.as_ref())
                .into_iter()
                .flatten()
            {
                roots.push(plugin_skill_root(record, directory));
            }
        }
        roots
    }

    pub fn enabled_session_starts(&self) -> Vec<EnabledPluginSessionStart> {
        self.active_records()
            .filter_map(|record| {
                Some(EnabledPluginSessionStart {
                    plugin_id: record.id.clone(),
                    skill_name: record
                        .manifest
                        .as_ref()?
                        .session_start
                        .as_ref()?
                        .skill
                        .clone(),
                })
            })
            .collect()
    }

    pub fn enabled_mcp_servers(&self) -> HashMap<String, McpServerConfig> {
        let mut out = HashMap::new();
        for record in self.active_records() {
            for (name, config) in record
                .manifest
                .as_ref()
                .and_then(|manifest| manifest.mcp_servers.as_ref())
                .into_iter()
                .flatten()
            {
                if !is_mcp_server_enabled(record, name, config) {
                    continue;
                }
                out.insert(
                    plugin_mcp_runtime_name(&record.id, name),
                    with_plugin_mcp_runtime(
                        with_mcp_server_enabled(config.clone(), true),
                        &record.root,
                        &self.kimi_home_dir,
                    ),
                );
            }
        }
        out
    }

    pub fn summaries(&self) -> Vec<PluginSummary> {
        self.list().iter().map(record_to_summary).collect()
    }

    pub fn info(&self, id: &str) -> Option<PluginInfo> {
        self.get(id).map(record_to_info)
    }

    async fn persist(&self, records: &IndexMap<String, PluginRecord>) -> PluginManagerResult<()> {
        let plugins = records.values().map(installed_record).collect();
        write_installed(
            &self.kimi_home_dir,
            &InstalledFile {
                version: 1,
                plugins,
            },
        )
        .await?;
        Ok(())
    }

    async fn materialize(&self, entry: InstalledRecord) -> PluginManagerResult<PluginRecord> {
        let parsed = parse_manifest(&entry.root).await;
        Ok(self
            .record_from(RecordInput {
                id: entry.id,
                root: entry.root,
                source: entry.source,
                enabled: entry.enabled,
                installed_at: entry.installed_at,
                updated_at: entry.updated_at,
                original_source: entry.original_source,
                capabilities: entry.capabilities,
                github: entry.github,
                parsed,
            })
            .await)
    }

    async fn record_from(&self, input: RecordInput) -> PluginRecord {
        let has_error = input
            .parsed
            .diagnostics
            .iter()
            .any(|entry| entry.severity == PluginDiagnosticSeverity::Error);
        let skill_count = self
            .count_discovered_plugin_skills(&input.id, input.parsed.manifest.as_ref())
            .await;
        let skill_instructions = input
            .parsed
            .manifest
            .as_ref()
            .and_then(|manifest| manifest.skill_instructions.clone());
        PluginRecord {
            id: input.id,
            root: input.root,
            source: input.source,
            enabled: input.enabled,
            state: if has_error || input.parsed.manifest.is_none() {
                PluginState::Error
            } else {
                PluginState::Ok
            },
            installed_at: input.installed_at,
            updated_at: input.updated_at,
            original_source: input.original_source,
            capabilities: input.capabilities,
            github: input.github,
            skill_instructions,
            skill_count,
            manifest: input.parsed.manifest,
            manifest_kind: input.parsed.manifest_kind,
            manifest_path: input.parsed.manifest_path,
            shadowed_manifest_path: input.parsed.shadowed_manifest_path,
            diagnostics: input.parsed.diagnostics,
        }
    }

    async fn count_discovered_plugin_skills(
        &self,
        plugin_id: &str,
        manifest: Option<&super::types::PluginManifest>,
    ) -> usize {
        let Some(manifest) = manifest else { return 0 };
        let Some(directories) = manifest.skills.as_ref().filter(|dirs| !dirs.is_empty()) else {
            return 0;
        };
        let roots = directories
            .iter()
            .map(|directory| SkillRoot {
                path: directory.clone(),
                source: SkillSource::Extra,
                plugin: Some(SkillPluginContext {
                    id: plugin_id.to_owned(),
                    instructions: manifest.skill_instructions.clone(),
                }),
            })
            .collect::<Vec<_>>();
        match &self.discover_skills {
            Some(discovery) => discovery.discover(&roots).await.skills.len(),
            None => discover_file_skills(&roots, None).await.skills.len(),
        }
    }

    fn active_records(&self) -> impl Iterator<Item = &PluginRecord> {
        self.records.values().filter(|record| {
            record.enabled && record.state == PluginState::Ok && record.manifest.is_some()
        })
    }
}

fn emit_install_progress(
    progress: Option<&PluginInstallProgressCallback>,
    phase: PluginInstallPhase,
    downloaded_bytes: u64,
    total_bytes: Option<u64>,
) {
    if let Some(progress) = progress {
        progress(PluginInstallProgress {
            phase,
            downloaded_bytes,
            total_bytes,
        });
    }
}

struct PreparedInstall {
    source_root: String,
    original_source: String,
    source: PluginSource,
    github: Option<PluginGithubMetadata>,
}

struct RecordInput {
    id: String,
    root: String,
    source: PluginSource,
    enabled: bool,
    installed_at: String,
    updated_at: Option<String>,
    original_source: Option<String>,
    capabilities: Option<PluginCapabilityState>,
    github: Option<PluginGithubMetadata>,
    parsed: ParsedManifestResult,
}

async fn installed_github_sha(
    owner: &str,
    repo: &str,
    reference: &PluginGithubRef,
) -> PluginManagerResult<Option<String>> {
    if reference.kind == PluginGithubRefKind::Sha && reference.value.len() == 40 {
        return Ok(Some(reference.value.to_lowercase()));
    }
    Ok(Some(
        resolve_github_commit_sha(owner, repo, &reference.value).await?,
    ))
}

async fn check_github_update(record: PluginRecord) -> PluginManagerResult<PluginUpdateStatus> {
    let github = record
        .github
        .as_ref()
        .ok_or_else(|| message_error(format!("Plugin \"{}\" has no GitHub metadata", record.id)))?;
    let current = github.reference.clone();
    let pinned = explicit_github_ref(&record);
    if pinned.as_ref().is_some_and(|reference| {
        matches!(
            reference.kind,
            PluginGithubRefKind::Tag | PluginGithubRefKind::Sha
        )
    }) {
        return Ok(PluginUpdateStatus {
            id: record.id,
            source: PluginSource::Github,
            current: Some(current.clone()),
            latest: current.clone(),
            display_version: current.value,
            update_available: false,
        });
    }
    if let Some(reference) =
        pinned.filter(|reference| reference.kind == PluginGithubRefKind::Branch)
    {
        let latest_sha =
            resolve_github_commit_sha(&github.owner, &github.repo, &reference.value).await?;
        return Ok(PluginUpdateStatus {
            id: record.id,
            source: PluginSource::Github,
            current: Some(current.clone()),
            latest: current,
            display_version: latest_sha.chars().take(12).collect(),
            update_available: github.installed_sha.as_deref() != Some(&latest_sha),
        });
    }
    let latest = resolve_github_source(&GithubSourceInput {
        owner: github.owner.clone(),
        repo: github.repo.clone(),
        reference: None,
    })
    .await?;
    let mut update_available =
        current.kind != latest.reference.kind || current.value != latest.reference.value;
    if !update_available
        && matches!(
            latest.reference.kind,
            PluginGithubRefKind::Branch | PluginGithubRefKind::Tag
        )
    {
        let latest_sha =
            resolve_github_commit_sha(&github.owner, &github.repo, &latest.reference.value).await?;
        update_available = github.installed_sha.as_deref() != Some(&latest_sha);
    }
    Ok(PluginUpdateStatus {
        id: record.id,
        source: PluginSource::Github,
        current: Some(current),
        latest: latest.reference,
        display_version: latest.display_version,
        update_available,
    })
}

fn explicit_github_ref(record: &PluginRecord) -> Option<PluginGithubRef> {
    let fallback = record.github.as_ref().and_then(|github| {
        (github.reference.kind == PluginGithubRefKind::Sha
            || (github.reference.kind == PluginGithubRefKind::Branch
                && github.reference.value != "HEAD"))
            .then(|| github.reference.clone())
    });
    let Some(original) = &record.original_source else {
        return fallback;
    };
    match resolve_install_source(original) {
        Ok(ResolvedSource::Github { reference, .. }) => reference.or(fallback),
        _ => fallback,
    }
}

fn plugin_not_found(id: &str) -> PluginManagerError {
    ensure_plugin_errors_registered();
    Box::new(Error2::with_options(
        PLUGIN_NOT_FOUND,
        format!("Plugin \"{id}\" is not installed"),
        Error2Options {
            details: Some(Map::from_iter([(
                "id".to_owned(),
                Value::String(id.to_owned()),
            )])),
            ..Error2Options::default()
        },
    ))
}

async fn normalize_install_root(root_path: &str) -> PluginManagerResult<String> {
    let trimmed = root_path.trim();
    if !Path::new(trimmed).is_absolute() {
        return Err(message_error(format!(
            "Plugin root must be an absolute path (got \"{root_path}\")"
        )));
    }
    let resolved = tokio::fs::canonicalize(trimmed).await.map_err(|error| {
        message_error_with_source(format!("Plugin root does not exist: {trimmed}"), error)
    })?;
    if !tokio::fs::metadata(&resolved).await?.is_dir() {
        return Err(message_error(format!(
            "Plugin root is not a directory: {trimmed}"
        )));
    }
    Ok(path_to_string(resolved))
}

async fn copy_plugin_to_managed_root(
    kimi_home_dir: &str,
    id: &str,
    source_root: &str,
    previous_root: Option<&str>,
) -> PluginManagerResult<ManagedPluginCopy> {
    let managed_dir = Path::new(kimi_home_dir).join("plugins/managed");
    tokio::fs::create_dir_all(&managed_dir).await?;
    let nonce = uuid::Uuid::new_v4();
    let staging_root = managed_dir.join(format!(".{id}-{nonce}.staging"));
    let managed_root = managed_dir.join(format!("{id}-{nonce}"));
    if let Err(error) = copy_directory(source_root, &staging_root).await {
        let _ = remove_dir_all_force(&staging_root).await;
        return Err(error);
    }
    if let Err(error) = tokio::fs::rename(&staging_root, &managed_root).await {
        let _ = remove_dir_all_force(&staging_root).await;
        return Err(Box::new(error));
    }
    let canonical_root = match tokio::fs::canonicalize(&managed_root).await {
        Ok(root) => root,
        Err(error) => {
            remove_dir_all_force(&managed_root).await?;
            return Err(Box::new(error));
        }
    };
    Ok(ManagedPluginCopy {
        root: path_to_string(canonical_root),
        previous_root: previous_root.map(str::to_owned),
    })
}

async fn rollback_managed_plugin_copy(copy: &ManagedPluginCopy) -> PluginManagerResult<()> {
    remove_dir_all_force(Path::new(&copy.root)).await?;
    Ok(())
}

async fn remove_managed_plugin_root(kimi_home_dir: &str, root: &str) -> std::io::Result<()> {
    let managed_dir = Path::new(kimi_home_dir).join("plugins/managed");
    let managed_dir = match tokio::fs::canonicalize(&managed_dir).await {
        Ok(path) => path,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    let root = match tokio::fs::canonicalize(root).await {
        Ok(path) => path,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    if root.parent() != Some(managed_dir.as_path()) {
        return Ok(());
    }
    remove_dir_all_force(&root).await
}

async fn copy_directory(source: &str, destination: &Path) -> PluginManagerResult<()> {
    let source = PathBuf::from(source);
    let destination = destination.to_owned();
    tokio::task::spawn_blocking(move || copy_directory_blocking(&source, &destination)).await??;
    Ok(())
}

fn copy_directory_blocking(source: &Path, destination: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(destination)?;
    for entry in std::fs::read_dir(source)? {
        let entry = entry?;
        let source_path = entry.path();
        let destination_path = destination.join(entry.file_name());
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            copy_directory_blocking(&source_path, &destination_path)?;
        } else if file_type.is_file() {
            std::fs::copy(source_path, destination_path)?;
        } else if file_type.is_symlink() {
            copy_symlink(&source_path, &destination_path)?;
        }
    }
    Ok(())
}

#[cfg(unix)]
fn copy_symlink(source: &Path, destination: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(std::fs::read_link(source)?, destination)
}

#[cfg(windows)]
fn copy_symlink(source: &Path, destination: &Path) -> std::io::Result<()> {
    let target = std::fs::read_link(source)?;
    if std::fs::metadata(source)?.is_dir() {
        std::os::windows::fs::symlink_dir(target, destination)
    } else {
        std::os::windows::fs::symlink_file(target, destination)
    }
}

async fn remove_dir_all_force(path: &Path) -> std::io::Result<()> {
    match tokio::fs::remove_dir_all(path).await {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

async fn create_temporary_directory(prefix: &str) -> std::io::Result<PathBuf> {
    let path = std::env::temp_dir().join(format!("{prefix}{}", uuid::Uuid::new_v4()));
    tokio::fs::create_dir(&path).await?;
    Ok(path)
}

fn installed_record(record: &PluginRecord) -> InstalledRecord {
    InstalledRecord {
        id: record.id.clone(),
        root: record.root.clone(),
        source: record.source,
        enabled: record.enabled,
        installed_at: record.installed_at.clone(),
        updated_at: record.updated_at.clone(),
        original_source: record.original_source.clone(),
        capabilities: record.capabilities.clone(),
        github: record.github.clone(),
    }
}

fn record_to_summary(record: &PluginRecord) -> PluginSummary {
    let manifest = record.manifest.as_ref();
    PluginSummary {
        id: record.id.clone(),
        display_name: manifest
            .and_then(|manifest| manifest.interface.as_ref())
            .and_then(|interface| interface.display_name.clone())
            .unwrap_or_else(|| record.id.clone()),
        version: manifest.and_then(|manifest| manifest.version.clone()),
        enabled: record.enabled,
        state: record.state,
        skill_count: record.skill_count,
        mcp_server_count: manifest
            .and_then(|manifest| manifest.mcp_servers.as_ref())
            .map_or(0, HashMap::len),
        enabled_mcp_server_count: plugin_mcp_servers_info(record)
            .iter()
            .filter(|server| server.enabled)
            .count(),
        hook_count: manifest
            .and_then(|manifest| manifest.hooks.as_ref())
            .map_or(0, Vec::len),
        command_count: manifest
            .and_then(|manifest| manifest.commands.as_ref())
            .map_or(0, Vec::len),
        has_errors: record
            .diagnostics
            .iter()
            .any(|entry| entry.severity == PluginDiagnosticSeverity::Error),
        source: record.source,
        original_source: record.original_source.clone(),
        github: record.github.clone(),
    }
}

fn record_to_info(record: &PluginRecord) -> PluginInfo {
    PluginInfo {
        summary: record_to_summary(record),
        root: record.root.clone(),
        installed_at: record.installed_at.clone(),
        updated_at: record.updated_at.clone(),
        manifest_kind: record.manifest_kind,
        manifest_path: record.manifest_path.clone(),
        manifest: record.manifest.clone(),
        mcp_servers: plugin_mcp_servers_info(record),
        shadowed_manifest_path: record.shadowed_manifest_path.clone(),
        diagnostics: record.diagnostics.clone(),
    }
}

fn is_mcp_server_enabled(record: &PluginRecord, name: &str, config: &McpServerConfig) -> bool {
    record
        .capabilities
        .as_ref()
        .and_then(|capabilities| capabilities.mcp_servers.as_ref())
        .and_then(|servers| servers.get(name))
        .map_or_else(
            || config.common().enabled != Some(false),
            |state| state.enabled,
        )
}

fn plugin_mcp_servers_info(record: &PluginRecord) -> Vec<PluginMcpServerInfo> {
    let mut servers = record
        .manifest
        .as_ref()
        .and_then(|manifest| manifest.mcp_servers.as_ref())
        .into_iter()
        .flatten()
        .map(|(name, config)| plugin_mcp_server_info(record, name, config))
        .collect::<Vec<_>>();
    servers.sort_by(|left, right| left.name.cmp(&right.name));
    servers
}

fn plugin_mcp_server_info(
    record: &PluginRecord,
    name: &str,
    config: &McpServerConfig,
) -> PluginMcpServerInfo {
    let enabled = is_mcp_server_enabled(record, name, config);
    let runtime_name = plugin_mcp_runtime_name(&record.id, name);
    match config {
        McpServerConfig::Http(remote) | McpServerConfig::Sse(remote) => {
            let mut header_keys = remote
                .headers
                .as_ref()
                .map(|headers| headers.keys().cloned().collect::<Vec<_>>());
            if let Some(keys) = &mut header_keys {
                keys.sort();
            }
            PluginMcpServerInfo {
                name: name.to_owned(),
                runtime_name,
                enabled,
                transport: if matches!(config, McpServerConfig::Http(_)) {
                    PluginMcpTransport::Http
                } else {
                    PluginMcpTransport::Sse
                },
                command: None,
                args: None,
                cwd: None,
                url: Some(remote.url.clone()),
                env_keys: None,
                header_keys,
            }
        }
        McpServerConfig::Stdio(stdio) => {
            let mut env_keys = stdio
                .env
                .as_ref()
                .map(|env| env.keys().cloned().collect::<Vec<_>>());
            if let Some(keys) = &mut env_keys {
                keys.sort();
            }
            PluginMcpServerInfo {
                name: name.to_owned(),
                runtime_name,
                enabled,
                transport: PluginMcpTransport::Stdio,
                command: Some(stdio.command.clone()),
                args: stdio.args.clone(),
                cwd: stdio.cwd.clone(),
                url: None,
                env_keys,
                header_keys: None,
            }
        }
    }
}

fn plugin_mcp_runtime_name(plugin_id: &str, server_name: &str) -> String {
    format!("plugin-{plugin_id}:{server_name}")
}

fn with_mcp_server_enabled(mut config: McpServerConfig, enabled: bool) -> McpServerConfig {
    match &mut config {
        McpServerConfig::Stdio(config) => config.common.enabled = Some(enabled),
        McpServerConfig::Http(config) | McpServerConfig::Sse(config) => {
            config.common.enabled = Some(enabled);
        }
    }
    config
}

fn with_plugin_mcp_runtime(
    config: McpServerConfig,
    plugin_root: &str,
    kimi_home_dir: &str,
) -> McpServerConfig {
    let McpServerConfig::Stdio(mut stdio) = config else {
        return config;
    };
    let mut env = stdio.env.take().unwrap_or_default();
    env.insert("KIMI_CODE_HOME".to_owned(), kimi_home_dir.to_owned());
    env.insert("KIMI_PLUGIN_ROOT".to_owned(), plugin_root.to_owned());
    stdio.cwd.get_or_insert_with(|| plugin_root.to_owned());
    // Keep the manifest's executable unchanged. In particular, a desktop GUI
    // executable is not a Node-compatible host; routing `node` through
    // `current_exe()` recursively launches desktop windows.
    stdio.env = Some(env);
    McpServerConfig::Stdio(stdio)
}

fn plugin_skill_root(record: &PluginRecord, directory: &str) -> SkillRoot {
    SkillRoot {
        path: directory.to_owned(),
        source: SkillSource::Extra,
        plugin: Some(SkillPluginContext {
            id: record.id.clone(),
            instructions: record.skill_instructions.clone(),
        }),
    }
}

fn now_iso() -> String {
    DateTime::<Utc>::from(SystemTime::now()).to_rfc3339_opts(SecondsFormat::Millis, true)
}

fn path_to_string(path: impl AsRef<Path>) -> String {
    path.as_ref().to_string_lossy().into_owned()
}

fn message_error(message: impl Into<String>) -> PluginManagerError {
    Box::new(ManagerMessageError {
        message: message.into(),
        source: None,
    })
}

fn message_error_with_source(
    message: impl Into<String>,
    source: impl Error + Send + Sync + 'static,
) -> PluginManagerError {
    Box::new(ManagerMessageError {
        message: message.into(),
        source: Some(Box::new(source)),
    })
}

#[derive(Debug, Error)]
#[error("{message}")]
struct ManagerMessageError {
    message: String,
    #[source]
    source: Option<Box<dyn Error + Send + Sync>>,
}

#[derive(Debug, Error)]
#[error("Plugin installation failed and rollback also failed")]
struct CombinedInstallError {
    install: PluginManagerError,
    rollback: PluginManagerError,
}

#[cfg(test)]
mod tests {
    use crate::agent::mcp::{McpServerCommonFields, McpServerStdioConfig};

    use super::*;

    async fn write_plugin(root: &Path) {
        tokio::fs::create_dir_all(root.join("skills/example"))
            .await
            .unwrap();
        tokio::fs::create_dir_all(root.join("commands"))
            .await
            .unwrap();
        tokio::fs::write(
            root.join("skills/example/SKILL.md"),
            "---\nname: example\ndescription: demo\n---\nbody",
        )
        .await
        .unwrap();
        tokio::fs::write(root.join("commands/run.md"), "run $ARGUMENTS")
            .await
            .unwrap();
        tokio::fs::write(
            root.join("kimi.plugin.json"),
            r#"{
          "name":"demo", "skills":"./skills", "commands":"./commands",
          "mcpServers":{"server":{"command":"tool"}},
          "hooks":[{"event":"Stop","command":"cleanup"}],
          "sessionStart":{"skill":"example"}
        }"#,
        )
        .await
        .unwrap();
    }

    #[test]
    fn node_mcp_server_keeps_node_as_the_executable() {
        let runtime = with_plugin_mcp_runtime(
            McpServerConfig::Stdio(McpServerStdioConfig {
                command: "node".to_owned(),
                args: Some(vec!["./bin/server.mjs".to_owned()]),
                env: None,
                cwd: None,
                executor: None,
                common: McpServerCommonFields::default(),
            }),
            "C:/plugins/demo",
            "C:/kimi-home",
        );
        let McpServerConfig::Stdio(runtime) = runtime else {
            panic!("expected stdio config");
        };
        assert_eq!(runtime.command, "node");
        assert_eq!(runtime.args, Some(vec!["./bin/server.mjs".to_owned()]));
        assert_eq!(runtime.cwd.as_deref(), Some("C:/plugins/demo"));
        assert_eq!(
            runtime
                .env
                .as_ref()
                .and_then(|env| env.get("KIMI_PLUGIN_ROOT"))
                .map(String::as_str),
            Some("C:/plugins/demo")
        );
    }

    #[tokio::test]
    async fn installs_persists_and_exposes_enabled_contributions() {
        let base = std::env::temp_dir().join(format!("plugin-manager-{}", uuid::Uuid::new_v4()));
        let home = base.join("home");
        let source = base.join("source");
        write_plugin(&source).await;
        let mut manager = PluginManager::new(PluginManagerOptions {
            kimi_home_dir: path_to_string(&home),
            discover_skills: None,
        });
        let record = manager.install(source.to_str().unwrap()).await.unwrap();
        let first_root = record.root.clone();
        assert_eq!(record.id, "demo");
        assert_eq!(record.skill_count, 1);
        assert_eq!(manager.summaries()[0].command_count, 1);
        assert_eq!(manager.enabled_hooks().len(), 1);
        assert_eq!(manager.enabled_commands().await.len(), 1);
        assert_eq!(manager.plugin_skill_roots().len(), 1);
        assert_eq!(manager.enabled_session_starts().len(), 1);
        assert_eq!(manager.enabled_mcp_servers().len(), 1);

        manager.set_enabled("DEMO", false).await.unwrap();
        assert!(manager.enabled_hooks().is_empty());
        let mut loaded = PluginManager::new(PluginManagerOptions {
            kimi_home_dir: path_to_string(&home),
            discover_skills: None,
        });
        loaded.load().await.unwrap();
        assert!(!loaded.get("demo").unwrap().enabled);
        let updated = loaded.install(source.to_str().unwrap()).await.unwrap();
        assert_ne!(updated.root, first_root);
        assert!(!Path::new(&first_root).exists());
        let installed_root = updated.root.clone();
        loaded.remove("demo").await.unwrap();
        assert!(!Path::new(&installed_root).exists());
        tokio::fs::remove_dir_all(base).await.unwrap();
    }

    #[tokio::test]
    async fn reload_isolates_bad_records_and_reports_added_removed() {
        let base = std::env::temp_dir().join(format!("plugin-reload-{}", uuid::Uuid::new_v4()));
        let home = base.join("home");
        let good = base.join("good");
        write_plugin(&good).await;
        let file = InstalledFile {
            version: 1,
            plugins: vec![InstalledRecord {
                id: "demo".to_owned(),
                root: path_to_string(&good),
                source: PluginSource::LocalPath,
                enabled: true,
                installed_at: now_iso(),
                updated_at: None,
                original_source: None,
                capabilities: None,
                github: None,
            }],
        };
        write_installed(&home, &file).await.unwrap();
        let mut manager = PluginManager::new(PluginManagerOptions {
            kimi_home_dir: path_to_string(&home),
            discover_skills: None,
        });
        let summary = manager.reload().await.unwrap();
        assert_eq!(summary.added, ["demo"]);
        assert!(summary.errors.is_empty());
        tokio::fs::remove_dir_all(base).await.unwrap();
    }
}
