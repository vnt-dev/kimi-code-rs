//! The model-facing file pattern matcher backed by ripgrep.
//!
//! Original: `packages/agent-core-v2/src/os/backends/node-local/tools/glob.ts`.

use std::{path::Path, sync::Arc};

use async_trait::async_trait;
use futures_util::future::BoxFuture;
use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::{
    _base::{
        di::instantiation::ServicesAccessorExt, exec_env::environment_probe::PathClass,
        utils::abort::AbortSignal,
    },
    agent::tool_registry::{ToolContributionOptions, register_tool},
    app::telemetry::{
        GlobToolRgFallbackEvent, RgBinarySource, RgFallbackOutcome, TELEMETRY_SERVICE_ID,
        TelemetryServiceEventExt, TelemetryServiceHandle,
    },
    kosong::contract::tool::Tool,
    os::{
        backends::node_local::tools::{
            rg_locator::{
                EnsureRgPathOptions, RgResolutionSource, ensure_rg_path, rg_unavailable_message,
            },
            rg_probe::ProcessRgProbe,
            run_rg::{
                DEFAULT_TIMEOUT, MAX_OUTPUT_BYTES, RunRgError, RunRgOutcome, run_rg_once,
                should_retry_ripgrep_eagain,
            },
        },
        interface::{
            host_environment::{HOST_ENVIRONMENT_SERVICE_ID, HostEnvironmentHandle},
            host_file_system::{HOST_FILE_SYSTEM_SERVICE_ID, HostFileSystemServiceHandle},
            host_fs_errors::OS_FS_NOT_FOUND,
            host_process::{HOST_PROCESS_SERVICE_ID, HostProcessServiceHandle},
        },
    },
    session::{
        skill_catalog::{SESSION_SKILL_CATALOG_ID, SessionSkillCatalogHandle},
        workspace_context::{SESSION_WORKSPACE_CONTEXT_ID, SessionWorkspaceContextHandle},
    },
    tool::{
        ExecutableTool, ExecutableToolContext, ExecutableToolResult, RunnableToolExecution,
        ToolAccess, ToolExecution,
        input_schema::to_input_json_schema,
        path_access::{
            PathAccessOperation, SENSITIVE_DOT_VARIANT_SUFFIXES, WorkspaceAccessPolicy,
            WorkspaceConfig, WorkspaceGuardMode, extend_workspace_with_skill_roots,
            is_sensitive_file, is_within_directory, resolve_path_access_path,
        },
        rule_match::{literal_rule_pattern, matches_glob_rule_subject},
    },
};
use kimi_code_protocol::{FileIoOperation, ToolInputDisplay};

pub const MAX_MATCHES: usize = 100;
pub const WINDOWS_PATH_HINT: &str = "\n\nWindows note: the `path` argument accepts both Windows paths (e.g. `C:\\Users\\foo`) and POSIX-style paths (e.g. `/c/Users/foo`). Matched paths are returned in Windows backslash form; convert them to forward slashes before using them in a Bash command.";
const GLOB_DESCRIPTION: &str = include_str!("glob.md");
const VCS_DIRECTORIES_TO_EXCLUDE: &[&str] = &[".git", ".svn", ".hg", ".bzr", ".jj", ".sl"];
const SENSITIVE_KEY_BASENAMES: &[&str] = &["id_rsa", "id_ed25519", "id_ecdsa"];

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub struct GlobInput {
    pub pattern: String,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub include_ignored: Option<bool>,
    #[serde(default)]
    pub include_dirs: Option<bool>,
}

pub fn glob_parameters() -> Map<String, Value> {
    to_input_json_schema(
        json!({
            "type": "object",
            "properties": {
                "pattern": { "type": "string", "description": "Glob pattern to match files." },
                "path": {
                    "type": "string",
                    "description": "Directory to search. Accepts an absolute path, or a path relative to the current working directory. Defaults to the current working directory."
                },
                "include_ignored": {
                    "type": "boolean",
                    "description": "Also match files excluded by ignore files such as `.gitignore`, `.ignore`, and `.rgignore` (for example `node_modules` or build outputs). Sensitive files (such as `.env`) remain filtered out for safety. VCS metadata directories (`.git` and similar) are always skipped, even when this is true. Defaults to false."
                },
                "include_dirs": {
                    "type": "boolean",
                    "description": "Deprecated and ignored. Results are always files-only — directories are never listed. Accepted only so older calls that still pass this flag are not rejected by parameter validation."
                }
            },
            "required": ["pattern"]
        })
        .as_object()
        .cloned()
        .expect("Glob schema is an object"),
    )
}

#[derive(Clone)]
pub struct GlobTool {
    fs: HostFileSystemServiceHandle,
    environment: HostEnvironmentHandle,
    process_service: HostProcessServiceHandle,
    workspace_context: SessionWorkspaceContextHandle,
    telemetry: TelemetryServiceHandle,
    skill_catalog: Option<SessionSkillCatalogHandle>,
    definition: Tool,
}

impl GlobTool {
    pub fn new(
        fs: HostFileSystemServiceHandle,
        environment: HostEnvironmentHandle,
        process_service: HostProcessServiceHandle,
        workspace_context: SessionWorkspaceContextHandle,
        telemetry: TelemetryServiceHandle,
        skill_catalog: Option<SessionSkillCatalogHandle>,
    ) -> Result<Self, String> {
        let description = if environment
            .path_class()
            .map_err(|error| error.to_string())?
            == PathClass::Win32
        {
            format!("{GLOB_DESCRIPTION}{WINDOWS_PATH_HINT}")
        } else {
            GLOB_DESCRIPTION.into()
        };
        Ok(Self {
            fs,
            environment,
            process_service,
            workspace_context,
            telemetry,
            skill_catalog,
            definition: Tool {
                name: "Glob".into(),
                description,
                parameters: glob_parameters(),
                deferred: None,
            },
        })
    }

    fn workspace_config(&self) -> Result<WorkspaceConfig, String> {
        let info = self.environment.info().map_err(|error| error.to_string())?;
        let workspace = WorkspaceConfig {
            workspace_dir: self
                .workspace_context
                .work_dir()
                .to_string_lossy()
                .into_owned(),
            additional_dirs: self
                .workspace_context
                .additional_dirs()
                .into_iter()
                .map(|path| path.to_string_lossy().into_owned())
                .collect(),
        };
        let roots = self
            .skill_catalog
            .as_ref()
            .map(|catalog| catalog.catalog().get_skill_roots())
            .unwrap_or_default();
        Ok(extend_workspace_with_skill_roots(&workspace, &roots, info.path_class).into_owned())
    }

    async fn execute(
        &self,
        args: GlobInput,
        signal: AbortSignal,
        search_root: String,
        workspace: WorkspaceConfig,
        path_class: PathClass,
    ) -> ExecutableToolResult {
        match self.fs.stat(Path::new(&search_root)).await {
            Ok(stat) if !stat.is_directory => {
                return ExecutableToolResult::error(format!("{search_root} is not a directory"));
            }
            Err(error) if error.code() == OS_FS_NOT_FOUND => {
                return ExecutableToolResult::error(format!("{search_root} does not exist"));
            }
            Err(error) => return ExecutableToolResult::error(error.to_string()),
            Ok(_) => {}
        }
        if signal.aborted() {
            return ExecutableToolResult::error("Glob aborted");
        }

        let resolution = match ensure_rg_path(
            &ProcessRgProbe::new(self.process_service.clone()),
            EnsureRgPathOptions {
                signal: Some(signal.clone()),
                allow_cached_fallback: true,
                ..EnsureRgPathOptions::default()
            },
        )
        .await
        {
            Ok(resolution) => resolution,
            Err(error) => {
                if signal.aborted() {
                    return ExecutableToolResult::error("Glob aborted");
                }
                let _ = self.telemetry.track_event(&GlobToolRgFallbackEvent {
                    source: None,
                    outcome: RgFallbackOutcome::Failed,
                });
                return ExecutableToolResult::error(rg_unavailable_message(error));
            }
        };
        if let Some(source) = telemetry_source(resolution.source) {
            let _ = self.telemetry.track_event(&GlobToolRgFallbackEvent {
                source: Some(source),
                outcome: RgFallbackOutcome::Resolved,
            });
        }

        let rg_path = resolution.path.to_string_lossy().into_owned();
        let mut run = match run_rg_once(
            self.process_service.0.as_ref(),
            &build_rg_args(&rg_path, &args, false),
            &signal,
            Some(search_root.clone()),
        )
        .await
        {
            Ok(RunRgOutcome::Result(result)) => result,
            Ok(RunRgOutcome::Aborted) => return ExecutableToolResult::error("Glob aborted"),
            Err(error) => return ExecutableToolResult::error(format_spawn_error(&error)),
        };
        if should_retry_ripgrep_eagain(&run) {
            run = match run_rg_once(
                self.process_service.0.as_ref(),
                &build_rg_args(&rg_path, &args, true),
                &signal,
                Some(search_root.clone()),
            )
            .await
            {
                Ok(RunRgOutcome::Result(result)) => result,
                Ok(RunRgOutcome::Aborted) => return ExecutableToolResult::error("Glob aborted"),
                Err(error) => return ExecutableToolResult::error(format_spawn_error(&error)),
            };
        }

        let mut traversal_warning = None;
        if !matches!(run.exit_code, 0 | 1) && !run.timed_out {
            if split_complete_paths(&run.stdout_text, true).is_empty() {
                return ExecutableToolResult::error(format_glob_error(
                    &search_root,
                    &run.stderr_text,
                ));
            }
            traversal_warning = Some(format_glob_warning(&run.stderr_text));
        }
        if signal.aborted() {
            return ExecutableToolResult::error("Glob aborted");
        }

        let raw_paths =
            split_complete_paths(&run.stdout_text, run.buffer_truncated || run.timed_out)
                .into_iter()
                .map(|path| {
                    Path::new(&search_root)
                        .join(path)
                        .to_string_lossy()
                        .into_owned()
                });
        let mut kept = Vec::new();
        let mut filtered_sensitive = 0;
        for path in raw_paths {
            if is_sensitive_file(&path) {
                filtered_sensitive += 1;
            } else {
                kept.push(path);
            }
        }
        let truncated = kept.len() > MAX_MATCHES;
        kept.truncate(MAX_MATCHES);
        if kept.is_empty() && !run.timed_out {
            return ExecutableToolResult::success(if filtered_sensitive > 0 {
                format!(
                    "No non-sensitive matches found ({filtered_sensitive} sensitive file(s) filtered)."
                )
            } else {
                "No matches found".into()
            });
        }

        let should_relativize =
            is_within_directory(&search_root, &workspace.workspace_dir, path_class);
        let display_lines = kept.iter().map(|path| {
            if should_relativize {
                relativize_if_under(path, &search_root, path_class)
            } else {
                path.clone()
            }
        });
        let mut lines = Vec::new();
        if run.timed_out {
            lines.push(format!(
                "Glob timed out after {}s; partial results returned.",
                DEFAULT_TIMEOUT.as_secs()
            ));
        }
        if run.buffer_truncated {
            lines.push(format!(
                "[stdout truncated at {MAX_OUTPUT_BYTES} bytes; results may be incomplete — use a more specific pattern]"
            ));
        }
        if let Some(warning) = traversal_warning {
            lines.push(warning);
        }
        if truncated {
            lines.push(format!(
                "[Truncated at {MAX_MATCHES} matches — use a more specific pattern]"
            ));
            lines.push(format!(
                "Only the first {MAX_MATCHES} matches are returned."
            ));
        }
        lines.extend(display_lines);
        if filtered_sensitive > 0 {
            lines.push(format!("Filtered {filtered_sensitive} sensitive file(s)."));
        }
        if !truncated && kept.len() == MAX_MATCHES {
            lines.push(format!("Found {} matches", kept.len()));
        }
        ExecutableToolResult::success(lines.join("\n"))
    }
}

#[async_trait]
impl ExecutableTool for GlobTool {
    type Input = GlobInput;

    fn tool(&self) -> &Tool {
        &self.definition
    }

    async fn resolve_execution(&self, args: Self::Input) -> ToolExecution {
        let info = match self.environment.info() {
            Ok(info) => info,
            Err(error) => {
                return ToolExecution::Error(ExecutableToolResult::error(error.to_string()));
            }
        };
        let workspace = match self.workspace_config() {
            Ok(workspace) => workspace,
            Err(error) => return ToolExecution::Error(ExecutableToolResult::error(error)),
        };
        let search_root = if let Some(path) = &args.path {
            match resolve_path_access_path(
                path,
                &info,
                &workspace,
                PathAccessOperation::Search,
                WorkspaceAccessPolicy {
                    guard_mode: WorkspaceGuardMode::AbsoluteOutsideAllowed,
                    check_sensitive: false,
                },
                true,
            ) {
                Ok(path) => path,
                Err(error) => {
                    return ToolExecution::Error(ExecutableToolResult::error(error.to_string()));
                }
            }
        } else {
            workspace.workspace_dir.clone()
        };
        let mut details = vec![format!("pattern: {}", args.pattern)];
        if let Some(path) = &args.path {
            details.push(format!("path: {path}"));
        }
        if args.include_ignored == Some(true) {
            details.push("include_ignored: true".into());
        }
        let pattern = args.pattern.clone();
        let tool = self.clone();
        let execution_args = args.clone();
        let execution_root = search_root.clone();
        let execution_workspace = workspace.clone();
        let path_class = info.path_class;
        let mut execution = RunnableToolExecution::new(
            literal_rule_pattern("Glob", &args.pattern),
            Arc::new(move |context: ExecutableToolContext| {
                let tool = tool.clone();
                let args = execution_args.clone();
                let root = execution_root.clone();
                let workspace = execution_workspace.clone();
                Box::pin(async move {
                    tool.execute(args, context.signal, root, workspace, path_class)
                        .await
                }) as BoxFuture<'static, ExecutableToolResult>
            }),
        );
        execution.accesses = Some(ToolAccess::search_tree(search_root.clone()));
        execution.description = Some(format!("Searching {}", args.pattern));
        execution.display = Some(ToolInputDisplay::FileIo {
            operation: FileIoOperation::Glob,
            path: search_root,
            detail: Some(details.join(", ")),
            content: None,
            before: None,
            after: None,
        });
        execution.matches_rule = Some(Arc::new(move |rule_args| {
            matches_glob_rule_subject(rule_args, &pattern)
        }));
        ToolExecution::Runnable(execution)
    }
}

pub fn register_glob_tool() {
    register_tool(
        Arc::new(|accessor| {
            let skill_catalog = accessor
                .get(SESSION_SKILL_CATALOG_ID)
                .ok()
                .map(|catalog| (*catalog).clone());
            GlobTool::new(
                (*accessor.get(HOST_FILE_SYSTEM_SERVICE_ID)?).clone(),
                (*accessor.get(HOST_ENVIRONMENT_SERVICE_ID)?).clone(),
                (*accessor.get(HOST_PROCESS_SERVICE_ID)?).clone(),
                (*accessor.get(SESSION_WORKSPACE_CONTEXT_ID)?).clone(),
                (*accessor.get(TELEMETRY_SERVICE_ID)?).clone(),
                skill_catalog,
            )
            .map(|tool| Arc::new(tool) as Arc<dyn crate::tool::ErasedExecutableTool>)
            .map_err(crate::_base::di::errors::DiError::Factory)
        }),
        ToolContributionOptions::default(),
    );
}

fn telemetry_source(source: RgResolutionSource) -> Option<RgBinarySource> {
    match source {
        RgResolutionSource::SystemPath => None,
        RgResolutionSource::Vendor => Some(RgBinarySource::Vendor),
        RgResolutionSource::ShareBinCached => Some(RgBinarySource::ShareBinCached),
        RgResolutionSource::ShareBinDownloaded => Some(RgBinarySource::ShareBinDownloaded),
    }
}

fn build_rg_args(rg_path: &str, args: &GlobInput, single_threaded: bool) -> Vec<String> {
    let mut command = vec![rg_path.into()];
    if single_threaded {
        command.extend(["-j".into(), "1".into()]);
    }
    command.extend([
        "--files".into(),
        "--hidden".into(),
        "--sortr=modified".into(),
    ]);
    for directory in VCS_DIRECTORIES_TO_EXCLUDE {
        command.extend(["--glob".into(), format!("!{directory}")]);
    }
    command.extend(["--glob".into(), args.pattern.clone()]);
    for glob in sensitive_globs_to_exclude() {
        command.extend(["--glob".into(), format!("!{glob}")]);
    }
    if args.include_ignored == Some(true) {
        command.push("--no-ignore".into());
    }
    command.push(".".into());
    command
}

fn sensitive_globs_to_exclude() -> Vec<String> {
    let mut globs = vec!["**/.env".into()];
    for name in SENSITIVE_KEY_BASENAMES {
        globs.push(format!("**/{name}"));
        globs.push(format!("**/{name}[-_]*"));
        globs.extend(
            SENSITIVE_DOT_VARIANT_SUFFIXES
                .iter()
                .map(|suffix| format!("**/{name}{suffix}")),
        );
    }
    globs.extend([
        "**/.aws/credentials".into(),
        "**/.aws/credentials/**".into(),
        "**/.gcp/credentials".into(),
        "**/.gcp/credentials/**".into(),
    ]);
    globs
}

fn format_glob_error(search_root: &str, stderr: &str) -> String {
    let trimmed = stderr.trim();
    if trimmed
        .to_ascii_lowercase()
        .contains("no such file or directory")
    {
        format!("{search_root} does not exist")
    } else if trimmed.is_empty() {
        "Glob failed".into()
    } else {
        format!("Glob failed: {trimmed}")
    }
}

fn format_glob_warning(stderr: &str) -> String {
    let trimmed = stderr.trim();
    if trimmed.is_empty() {
        "Glob completed with warnings; some directories could not be read.".into()
    } else {
        format!("Glob completed with warnings; some directories could not be read: {trimmed}")
    }
}

fn format_spawn_error(error: &RunRgError) -> String {
    match error {
        RunRgError::Process(process)
            if process
                .error()
                .details
                .as_ref()
                .and_then(|details| details.get("errno"))
                .and_then(Value::as_str)
                == Some("ENOENT") =>
        {
            rg_unavailable_message(error)
        }
        _ => error.to_string(),
    }
}

pub fn split_complete_paths(stdout_text: &str, truncated_output: bool) -> Vec<String> {
    let mut text = stdout_text;
    if truncated_output && !text.ends_with('\n') {
        text = text.rfind('\n').map_or("", |index| &text[..=index]);
    }
    text.split('\n')
        .filter(|path| !path.is_empty())
        .map(str::to_owned)
        .collect()
}

fn relativize_if_under(candidate: &str, base: &str, path_class: PathClass) -> String {
    let candidate = candidate.replace('\\', "/");
    let base = base.replace('\\', "/").trim_end_matches('/').to_owned();
    let comparable_candidate = if path_class == PathClass::Win32 {
        candidate.to_ascii_lowercase()
    } else {
        candidate.clone()
    };
    let comparable_base = if path_class == PathClass::Win32 {
        base.to_ascii_lowercase()
    } else {
        base.clone()
    };
    if comparable_candidate == comparable_base {
        ".".into()
    } else {
        let prefix = format!("{comparable_base}/");
        if comparable_candidate.starts_with(&prefix) {
            candidate[prefix.len()..].to_owned()
        } else {
            candidate
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn incomplete_paths_and_rg_args_match_source() {
        assert_eq!(
            split_complete_paths("a\npartial", true),
            vec!["a".to_owned()]
        );
        let args = build_rg_args(
            "rg",
            &GlobInput {
                pattern: "*.rs".into(),
                path: None,
                include_ignored: Some(true),
                include_dirs: None,
            },
            false,
        );
        assert!(args.contains(&"--sortr=modified".into()));
        assert!(args.contains(&"--no-ignore".into()));
        assert!(args.contains(&"!**/.env".into()));
    }
}
