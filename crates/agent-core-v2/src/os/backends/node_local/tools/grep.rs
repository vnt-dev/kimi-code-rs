//! The model-facing content search tool backed by ripgrep.
//!
//! Original: `packages/agent-core-v2/src/os/backends/node-local/tools/grep.ts`.

use std::{
    path::Path,
    sync::{Arc, LazyLock},
};

use async_trait::async_trait;
use futures_util::{StreamExt, future::BoxFuture, stream};
use indexmap::IndexSet;
use regex::Regex;
use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::{
    _base::{
        di::instantiation::ServicesAccessorExt, exec_env::environment_probe::PathClass,
        utils::abort::AbortSignal,
    },
    agent::tool_registry::{ToolContributionOptions, register_tool},
    app::telemetry::{
        GrepToolRgFallbackEvent, RgBinarySource, RgFallbackOutcome, TELEMETRY_SERVICE_ID,
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
                DEFAULT_TIMEOUT, MAX_OUTPUT_BYTES, RunRgError, RunRgOutcome, RunRgResult,
                run_rg_once, should_retry_ripgrep_eagain,
            },
        },
        interface::{
            host_environment::{HOST_ENVIRONMENT_SERVICE_ID, HostEnvironmentHandle},
            host_file_system::{HOST_FILE_SYSTEM_SERVICE_ID, HostFileSystemServiceHandle},
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
            is_sensitive_file, resolve_path_access_path,
        },
        result_builder::ToolResultBuilder,
        rule_match::{literal_rule_pattern, matches_glob_rule_subject},
    },
};
use kimi_code_protocol::{FileIoOperation, ToolInputDisplay};

const GREP_DESCRIPTION: &str = include_str!("grep.md");
const RG_MAX_COLUMNS: usize = 500;
const DEFAULT_HEAD_LIMIT: usize = 250;
const MTIME_STAT_CONCURRENCY: usize = 32;
const VCS_DIRECTORIES_TO_EXCLUDE: &[&str] = &[".git", ".svn", ".hg", ".bzr", ".jj", ".sl"];
const SENSITIVE_KEY_BASENAMES: &[&str] = &["id_rsa", "id_ed25519", "id_ecdsa"];

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum GrepMode {
    Content,
    #[default]
    FilesWithMatches,
    CountMatches,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub struct GrepInput {
    pub pattern: String,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub glob: Option<String>,
    #[serde(default, rename = "type")]
    pub file_type: Option<String>,
    #[serde(default)]
    pub output_mode: Option<GrepMode>,
    #[serde(default, rename = "-i")]
    pub case_insensitive: Option<bool>,
    #[serde(default, rename = "-n")]
    pub line_numbers: Option<bool>,
    #[serde(default, rename = "-A")]
    pub after_context: Option<usize>,
    #[serde(default, rename = "-B")]
    pub before_context: Option<usize>,
    #[serde(default, rename = "-C")]
    pub context: Option<usize>,
    #[serde(default)]
    pub head_limit: Option<usize>,
    #[serde(default)]
    pub offset: Option<usize>,
    #[serde(default)]
    pub multiline: Option<bool>,
    #[serde(default)]
    pub include_ignored: Option<bool>,
}

pub fn grep_parameters() -> Map<String, Value> {
    to_input_json_schema(
        json!({
            "type": "object",
            "properties": {
                "pattern": { "type": "string", "description": "Regular expression to search for." },
                "path": { "type": "string", "description": "File or directory to search. Accepts an absolute path, or a path relative to the current working directory. Omit to search the current working directory. Use Read instead when you already know a concrete file path and need its contents." },
                "glob": { "type": "string", "description": "Optional glob filter for which files to search, e.g. `*.ts`. Matched against each file's full absolute path, so a path-anchored pattern like `src/**/*.ts` silently matches nothing — use a basename pattern (`*.ts`), or anchor with `**/` (`**/src/**/*.ts`). To scope the search to a directory, use `path` instead." },
                "type": { "type": "string", "description": "Optional ripgrep file type filter, such as ts or py. Prefer this over `glob` when filtering by language or file kind: it is more efficient and less error-prone than an equivalent glob pattern." },
                "output_mode": {
                    "type": "string",
                    "enum": ["content", "files_with_matches", "count_matches"],
                    "description": "Shape of the result. `content` shows matching lines (honors `-A`, `-B`, `-C`, `-n`, and `head_limit`); `files_with_matches` shows only the paths of files that contain a match, most-recently-modified first (honors `head_limit`); `count_matches` shows per-file match counts as `path:count` lines, preceded by an aggregate total line. Defaults to `files_with_matches`."
                },
                "-i": { "type": "boolean", "description": "Perform a case-insensitive search. Defaults to false." },
                "-n": { "type": "boolean", "description": "Prefix each matching line with its line number. Applies only when `output_mode` is `content`. Defaults to true." },
                "-A": { "type": "integer", "minimum": 0, "description": "Number of lines to show after each match. Applies only when `output_mode` is `content`." },
                "-B": { "type": "integer", "minimum": 0, "description": "Number of lines to show before each match. Applies only when `output_mode` is `content`." },
                "-C": { "type": "integer", "minimum": 0, "description": "Number of lines to show before and after each match. Applies only when `output_mode` is `content`; takes precedence over `-A` and `-B`." },
                "head_limit": { "type": "integer", "minimum": 0, "description": "Limit output to the first N lines/entries after offset. Defaults to 250. Pass 0 for unlimited." },
                "offset": { "type": "integer", "minimum": 0, "description": "Number of leading lines/entries to skip before applying `head_limit`. Use it together with `head_limit` to page through large result sets. Defaults to 0." },
                "multiline": { "type": "boolean", "description": "Enable multiline matching, where the pattern can span line boundaries and `.` also matches newlines. Defaults to false." },
                "include_ignored": { "type": "boolean", "description": "Also search files excluded by ignore files such as `.gitignore`, `.ignore`, and `.rgignore` (for example `node_modules` or build outputs). Sensitive files (such as `.env`) remain filtered out for safety. VCS metadata directories (`.git` and similar) are always skipped, even when this is true. Defaults to false." }
            },
            "required": ["pattern"]
        })
        .as_object()
        .cloned()
        .expect("Grep schema is an object"),
    )
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum ParsedGrepLine {
    Record { file_path: String, payload: String },
    Separator,
    Legacy(String),
}

#[derive(Clone)]
pub struct GrepTool {
    process_service: HostProcessServiceHandle,
    fs: HostFileSystemServiceHandle,
    environment: HostEnvironmentHandle,
    workspace_context: SessionWorkspaceContextHandle,
    telemetry: TelemetryServiceHandle,
    skill_catalog: Option<SessionSkillCatalogHandle>,
    definition: Tool,
}

impl GrepTool {
    pub fn new(
        process_service: HostProcessServiceHandle,
        fs: HostFileSystemServiceHandle,
        environment: HostEnvironmentHandle,
        workspace_context: SessionWorkspaceContextHandle,
        telemetry: TelemetryServiceHandle,
        skill_catalog: Option<SessionSkillCatalogHandle>,
    ) -> Self {
        Self {
            process_service,
            fs,
            environment,
            workspace_context,
            telemetry,
            skill_catalog,
            definition: Tool {
                name: "Grep".into(),
                description: GREP_DESCRIPTION.into(),
                parameters: grep_parameters(),
                deferred: None,
            },
        }
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
        args: GrepInput,
        signal: AbortSignal,
        search_paths: Vec<String>,
        workspace: WorkspaceConfig,
        path_class: PathClass,
    ) -> ExecutableToolResult {
        if signal.aborted() {
            return ExecutableToolResult::error("Aborted before search started");
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
                    return ExecutableToolResult::error("Grep aborted");
                }
                let _ = self.telemetry.track_event(&GrepToolRgFallbackEvent {
                    source: None,
                    outcome: RgFallbackOutcome::Failed,
                });
                return ExecutableToolResult::error(rg_unavailable_message(error));
            }
        };
        if let Some(source) = telemetry_source(resolution.source) {
            let _ = self.telemetry.track_event(&GrepToolRgFallbackEvent {
                source: Some(source),
                outcome: RgFallbackOutcome::Resolved,
            });
        }

        let rg_path = resolution.path.to_string_lossy().into_owned();
        let mut run = match run_rg_once(
            self.process_service.0.as_ref(),
            &build_rg_args(&rg_path, &args, &search_paths, false),
            &signal,
            None,
        )
        .await
        {
            Ok(RunRgOutcome::Result(result)) => result,
            Ok(RunRgOutcome::Aborted) => return ExecutableToolResult::error("Grep aborted"),
            Err(error) => return ExecutableToolResult::error(format_spawn_error(&error)),
        };
        if should_retry_ripgrep_eagain(&run) {
            run = match run_rg_once(
                self.process_service.0.as_ref(),
                &build_rg_args(&rg_path, &args, &search_paths, true),
                &signal,
                None,
            )
            .await
            {
                Ok(RunRgOutcome::Result(result)) => result,
                Ok(RunRgOutcome::Aborted) => return ExecutableToolResult::error("Grep aborted"),
                Err(error) => return ExecutableToolResult::error(format_spawn_error(&error)),
            };
        }
        self.finish_run(args, signal, run, workspace, path_class)
            .await
    }

    async fn finish_run(
        &self,
        args: GrepInput,
        signal: AbortSignal,
        run: RunRgResult,
        workspace: WorkspaceConfig,
        path_class: PathClass,
    ) -> ExecutableToolResult {
        if !matches!(run.exit_code, 0 | 1) && !run.timed_out {
            return ExecutableToolResult::error(format_ripgrep_error(
                run.exit_code,
                &run.stderr_text,
                run.stderr_truncated,
            ));
        }
        let mode = args.output_mode.unwrap_or_default();
        let mut stdout = run.stdout_text;
        if run.buffer_truncated || run.timed_out {
            stdout = omit_incomplete_trailing_record(&stdout, mode);
        }
        if run.timed_out && stdout.trim().is_empty() {
            return ExecutableToolResult::error(format!(
                "Grep timed out after {}s. Try a more specific path or pattern.",
                DEFAULT_TIMEOUT.as_secs()
            ));
        }
        if signal.aborted() {
            return ExecutableToolResult::error("Grep aborted");
        }

        let raw_lines = parse_ripgrep_output(&stdout, mode);
        let mut filtered_sensitive = IndexSet::new();
        let kept = filter_sensitive_lines(&raw_lines, mode, &mut filtered_sensitive, path_class);
        let ordered = if mode == GrepMode::FilesWithMatches && !run.timed_out {
            match self.sort_files_with_matches_by_mtime(kept, &signal).await {
                Some(lines) => lines,
                None => return ExecutableToolResult::error("Grep aborted"),
            }
        } else {
            kept
        };

        let offset = args.offset.unwrap_or(0);
        let head_limit = args.head_limit.unwrap_or(DEFAULT_HEAD_LIMIT);
        let after_offset = ordered.iter().skip(offset).cloned().collect::<Vec<_>>();
        let limit_active = head_limit > 0;
        let limited = if limit_active {
            after_offset
                .iter()
                .take(head_limit)
                .cloned()
                .collect::<Vec<_>>()
        } else {
            after_offset.clone()
        };
        let pagination_truncated = limit_active && after_offset.len() > head_limit;

        let mut headers = Vec::new();
        let mut messages = Vec::new();
        if !filtered_sensitive.is_empty() {
            let paths = filtered_sensitive
                .iter()
                .map(|path| relativize_if_under(path, &workspace.workspace_dir, path_class))
                .collect::<Vec<_>>();
            messages.push(format!(
                "Filtered {} sensitive file(s): {}",
                filtered_sensitive.len(),
                paths.join(", ")
            ));
        }
        if mode == GrepMode::CountMatches && !ordered.is_empty() {
            headers.push(format_count_summary(
                &ordered,
                !filtered_sensitive.is_empty(),
            ));
        }
        if pagination_truncated {
            let total = after_offset.len() + offset;
            let next_offset = offset + head_limit;
            let notice = format!(
                "Results truncated to {head_limit} lines (total: {total}). Use offset={next_offset} to see more."
            );
            if mode == GrepMode::CountMatches {
                headers.push(notice);
            } else {
                messages.push(notice);
            }
        }
        if run.buffer_truncated {
            messages.push(format!(
                "[stdout truncated at {MAX_OUTPUT_BYTES} bytes; incomplete trailing line omitted]"
            ));
        }
        if run.timed_out {
            messages.push(format!(
                "Grep timed out after {}s; partial results returned",
                DEFAULT_TIMEOUT.as_secs()
            ));
        }
        let content_includes_line_numbers =
            mode == GrepMode::Content && args.line_numbers != Some(false);
        let body = limited
            .iter()
            .map(|line| {
                format_display_line(
                    line,
                    mode,
                    &workspace.workspace_dir,
                    path_class,
                    content_includes_line_numbers,
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        let visible_body = if ordered.is_empty() && !filtered_sensitive.is_empty() {
            "No non-sensitive matches found".into()
        } else {
            body
        };
        let body = if visible_body.is_empty() && headers.is_empty() && messages.is_empty() {
            "No non-sensitive matches found".into()
        } else {
            visible_body
        };
        let combined = headers
            .into_iter()
            .chain(std::iter::once(body))
            .chain(messages)
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join("\n");
        let mut builder = ToolResultBuilder::default();
        builder.write(&combined);
        let built = builder.ok("", None);
        let mut result = ExecutableToolResult::success(built.output);
        result.truncated = built.truncated.then_some(true);
        result
    }

    async fn sort_files_with_matches_by_mtime(
        &self,
        lines: Vec<ParsedGrepLine>,
        signal: &AbortSignal,
    ) -> Option<Vec<ParsedGrepLine>> {
        if signal.aborted() {
            return None;
        }
        let fs = self.fs.clone();
        let task_signal = signal.clone();
        let mut entries = stream::iter(lines.into_iter().enumerate())
            .map(move |(index, line)| {
                let fs = fs.clone();
                let signal = task_signal.clone();
                async move {
                    let path = match &line {
                        ParsedGrepLine::Record { file_path, .. } => Some(file_path.as_str()),
                        ParsedGrepLine::Legacy(text) => Some(text.as_str()),
                        ParsedGrepLine::Separator => None,
                    };
                    let mtime = if signal.aborted() {
                        0
                    } else if let Some(path) = path {
                        fs.stat(Path::new(path))
                            .await
                            .ok()
                            .and_then(|stat| stat.modified_millis)
                            .map_or(0, |millis| (millis / 1_000.0).trunc() as i64)
                    } else {
                        0
                    };
                    (line, mtime, index)
                }
            })
            .buffer_unordered(MTIME_STAT_CONCURRENCY)
            .collect::<Vec<_>>()
            .await;
        if signal.aborted() {
            return None;
        }
        entries.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.2.cmp(&right.2)));
        Some(entries.into_iter().map(|entry| entry.0).collect())
    }
}

#[async_trait]
impl ExecutableTool for GrepTool {
    type Input = GrepInput;

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
        let search_path = if let Some(path) = &args.path {
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
        let display_path = args
            .path
            .clone()
            .unwrap_or_else(|| workspace.workspace_dir.clone());
        let pattern = args.pattern.clone();
        let tool = self.clone();
        let execution_args = args.clone();
        let execution_paths = vec![search_path.clone()];
        let execution_workspace = workspace.clone();
        let path_class = info.path_class;
        let mut execution = RunnableToolExecution::new(
            literal_rule_pattern("Grep", &args.pattern),
            Arc::new(move |context: ExecutableToolContext| {
                let tool = tool.clone();
                let args = execution_args.clone();
                let paths = execution_paths.clone();
                let workspace = execution_workspace.clone();
                Box::pin(async move {
                    tool.execute(args, context.signal, paths, workspace, path_class)
                        .await
                }) as BoxFuture<'static, ExecutableToolResult>
            }),
        );
        execution.accesses = Some(ToolAccess::search_tree(search_path.clone()));
        execution.description = Some(format!(
            "Searching for '{}' in {display_path}",
            args.pattern
        ));
        execution.display = Some(ToolInputDisplay::FileIo {
            operation: FileIoOperation::Grep,
            path: search_path,
            detail: None,
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

pub fn register_grep_tool() {
    register_tool(
        Arc::new(|accessor| {
            let skill_catalog = accessor
                .get(SESSION_SKILL_CATALOG_ID)
                .ok()
                .map(|catalog| (*catalog).clone());
            Ok(Arc::new(GrepTool::new(
                (*accessor.get(HOST_PROCESS_SERVICE_ID)?).clone(),
                (*accessor.get(HOST_FILE_SYSTEM_SERVICE_ID)?).clone(),
                (*accessor.get(HOST_ENVIRONMENT_SERVICE_ID)?).clone(),
                (*accessor.get(SESSION_WORKSPACE_CONTEXT_ID)?).clone(),
                (*accessor.get(TELEMETRY_SERVICE_ID)?).clone(),
                skill_catalog,
            )) as Arc<dyn crate::tool::ErasedExecutableTool>)
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

fn build_rg_args(
    rg_path: &str,
    args: &GrepInput,
    search_paths: &[String],
    single_threaded: bool,
) -> Vec<String> {
    let mut command = vec![rg_path.into()];
    if single_threaded {
        command.extend(["-j".into(), "1".into()]);
    }
    command.push("--hidden".into());
    let mode = args.output_mode.unwrap_or_default();
    if mode != GrepMode::Content {
        command.extend(["--max-columns".into(), RG_MAX_COLUMNS.to_string()]);
    }
    command.push("--null".into());
    for directory in VCS_DIRECTORIES_TO_EXCLUDE {
        command.extend(["--glob".into(), format!("!{directory}")]);
    }
    match mode {
        GrepMode::FilesWithMatches => command.push("-l".into()),
        GrepMode::CountMatches => {
            command.extend(["--count-matches".into(), "--with-filename".into()])
        }
        GrepMode::Content => {}
    }
    if args.case_insensitive == Some(true) {
        command.push("-i".into());
    }
    if mode == GrepMode::Content {
        command.push("--with-filename".into());
        if args.line_numbers != Some(false) {
            command.push("-n".into());
        } else {
            command.extend(["--field-context-separator".into(), ":".into()]);
        }
        if let Some(context) = args.context {
            command.extend(["-C".into(), context.to_string()]);
        } else {
            if let Some(after) = args.after_context {
                command.extend(["-A".into(), after.to_string()]);
            }
            if let Some(before) = args.before_context {
                command.extend(["-B".into(), before.to_string()]);
            }
        }
    }
    if let Some(glob) = &args.glob {
        command.extend(["--glob".into(), glob.clone()]);
    }
    if let Some(file_type) = &args.file_type {
        command.extend(["--type".into(), file_type.clone()]);
    }
    if args.multiline == Some(true) {
        command.extend(["-U".into(), "--multiline-dotall".into()]);
    }
    if args.include_ignored == Some(true) {
        command.push("--no-ignore".into());
    }
    for glob in sensitive_globs_to_exclude() {
        command.extend(["--glob".into(), format!("!{glob}")]);
    }
    command.extend(["--".into(), args.pattern.clone()]);
    command.extend(search_paths.iter().cloned());
    command
}

fn split_rg_lines(text: &str) -> Vec<String> {
    let mut lines = text.split('\n').collect::<Vec<_>>();
    while lines.last() == Some(&"") {
        lines.pop();
    }
    lines
        .into_iter()
        .map(strip_trailing_carriage_return)
        .map(str::to_owned)
        .collect()
}

fn parse_ripgrep_output(text: &str, mode: GrepMode) -> Vec<ParsedGrepLine> {
    if text.is_empty() {
        return Vec::new();
    }
    if !text.contains('\0') {
        return split_rg_lines(text)
            .into_iter()
            .map(|line| {
                if mode == GrepMode::Content && line == "--" {
                    ParsedGrepLine::Separator
                } else {
                    ParsedGrepLine::Legacy(line)
                }
            })
            .collect();
    }
    if mode == GrepMode::FilesWithMatches {
        return text
            .split('\0')
            .map(strip_trailing_carriage_return)
            .filter(|path| !path.is_empty())
            .map(|file_path| ParsedGrepLine::Record {
                file_path: file_path.into(),
                payload: String::new(),
            })
            .collect();
    }

    let mut records = Vec::new();
    let mut cursor = 0;
    while cursor < text.len() {
        if text[cursor..].starts_with('\n') {
            cursor += 1;
            continue;
        }
        if text[cursor..].starts_with("--\r\n") {
            records.push(ParsedGrepLine::Separator);
            cursor += 4;
            continue;
        }
        if text[cursor..].starts_with("--\n") {
            records.push(ParsedGrepLine::Separator);
            cursor += 3;
            continue;
        }
        let Some(relative_nul) = text[cursor..].find('\0') else {
            let tail = strip_trailing_carriage_return(&text[cursor..]);
            if !tail.is_empty() {
                records.push(ParsedGrepLine::Legacy(tail.into()));
            }
            break;
        };
        let nul = cursor + relative_nul;
        let line_end = text[nul + 1..].find('\n').map(|index| nul + 1 + index);
        let payload_end = line_end.unwrap_or(text.len());
        records.push(ParsedGrepLine::Record {
            file_path: text[cursor..nul].into(),
            payload: strip_trailing_carriage_return(&text[nul + 1..payload_end]).into(),
        });
        cursor = line_end.map_or(text.len(), |index| index + 1);
    }
    records
}

fn format_display_line(
    line: &ParsedGrepLine,
    mode: GrepMode,
    workspace_dir: &str,
    path_class: PathClass,
    content_includes_line_numbers: bool,
) -> String {
    match line {
        ParsedGrepLine::Separator => "--".into(),
        ParsedGrepLine::Record { file_path, payload } => {
            let path = relativize_if_under(file_path, workspace_dir, path_class);
            match mode {
                GrepMode::FilesWithMatches => path,
                GrepMode::CountMatches => format!("{path}:{payload}"),
                GrepMode::Content => {
                    let separator = if content_includes_line_numbers {
                        content_payload_path_separator(payload)
                    } else {
                        ':'
                    };
                    format!("{path}{separator}{payload}")
                }
            }
        }
        ParsedGrepLine::Legacy(text) => match mode {
            GrepMode::FilesWithMatches => relativize_if_under(text, workspace_dir, path_class),
            GrepMode::CountMatches => text.rfind(':').filter(|index| *index > 0).map_or_else(
                || text.clone(),
                |index| {
                    format!(
                        "{}{}",
                        relativize_if_under(&text[..index], workspace_dir, path_class),
                        &text[index..]
                    )
                },
            ),
            GrepMode::Content => extract_content_file_path(text, path_class).map_or_else(
                || text.clone(),
                |path| {
                    format!(
                        "{}{}",
                        relativize_if_under(&path, workspace_dir, path_class),
                        &text[path.len()..]
                    )
                },
            ),
        },
    }
}

fn relativize_if_under(candidate: &str, base: &str, path_class: PathClass) -> String {
    let candidate = normalize_path(candidate);
    let base = normalize_path(base).trim_end_matches('/').to_owned();
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
        return ".".into();
    }
    let prefix = format!("{comparable_base}/");
    if comparable_candidate.starts_with(&prefix) {
        candidate[prefix.len()..].to_owned()
    } else {
        candidate
    }
}

fn omit_incomplete_trailing_record(text: &str, mode: GrepMode) -> String {
    if !text.contains('\0') {
        return omit_incomplete_trailing_line(text);
    }
    if mode == GrepMode::FilesWithMatches {
        return text
            .rfind('\0')
            .map_or_else(String::new, |index| text[..=index].into());
    }
    let mut cursor = 0;
    let mut last_complete_end = 0;
    while cursor < text.len() {
        if text[cursor..].starts_with('\n') {
            cursor += 1;
            last_complete_end = cursor;
            continue;
        }
        if text[cursor..].starts_with("--\r\n") {
            cursor += 4;
            last_complete_end = cursor;
            continue;
        }
        if text[cursor..].starts_with("--\n") {
            cursor += 3;
            last_complete_end = cursor;
            continue;
        }
        let Some(nul) = text[cursor..].find('\0').map(|index| cursor + index) else {
            break;
        };
        let Some(end) = text[nul + 1..].find('\n').map(|index| nul + 1 + index) else {
            break;
        };
        cursor = end + 1;
        last_complete_end = cursor;
    }
    text[..last_complete_end].into()
}

fn omit_incomplete_trailing_line(text: &str) -> String {
    text.rfind('\n')
        .map_or_else(String::new, |index| text[..index].into())
}

fn format_ripgrep_error(exit_code: i32, stderr: &str, stderr_truncated: bool) -> String {
    let stderr = stderr.trim();
    if stderr.is_empty() {
        return format!("Failed to grep: ripgrep exited with code {exit_code}");
    }
    let mut lines = vec![
        format!("Failed to grep: {}", summarize_ripgrep_stderr(stderr)),
        String::new(),
        "ripgrep stderr:".into(),
        stderr.into(),
    ];
    if stderr_truncated {
        lines.push(format!("[stderr truncated at {MAX_OUTPUT_BYTES} bytes]"));
    }
    lines.join("\n")
}

fn summarize_ripgrep_stderr(stderr: &str) -> String {
    let lines = split_rg_lines(stderr)
        .into_iter()
        .map(|line| line.trim().to_owned())
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>();
    lines
        .iter()
        .rev()
        .find(|line| line.to_ascii_lowercase().starts_with("error:"))
        .or_else(|| lines.last())
        .cloned()
        .unwrap_or_else(|| "ripgrep error".into())
}

fn filter_sensitive_lines(
    lines: &[ParsedGrepLine],
    mode: GrepMode,
    filtered_paths: &mut IndexSet<String>,
    path_class: PathClass,
) -> Vec<ParsedGrepLine> {
    let mut kept = Vec::new();
    for line in lines {
        if *line == ParsedGrepLine::Separator {
            kept.push(line.clone());
            continue;
        }
        if let Some(path) = parsed_file_path(line, mode, path_class)
            && is_sensitive_file(&path)
        {
            filtered_paths.insert(path);
            continue;
        }
        kept.push(line.clone());
    }
    if mode == GrepMode::Content {
        normalize_context_separators(kept)
    } else {
        kept
    }
}

fn normalize_context_separators(lines: Vec<ParsedGrepLine>) -> Vec<ParsedGrepLine> {
    let mut normalized = Vec::new();
    for line in lines {
        if line == ParsedGrepLine::Separator
            && (normalized.is_empty() || normalized.last() == Some(&ParsedGrepLine::Separator))
        {
            continue;
        }
        normalized.push(line);
    }
    while normalized.last() == Some(&ParsedGrepLine::Separator) {
        normalized.pop();
    }
    normalized
}

fn parsed_file_path(
    line: &ParsedGrepLine,
    mode: GrepMode,
    path_class: PathClass,
) -> Option<String> {
    match line {
        ParsedGrepLine::Record { file_path, .. } => Some(normalize_path(file_path)),
        ParsedGrepLine::Separator => None,
        ParsedGrepLine::Legacy(text) => match mode {
            GrepMode::FilesWithMatches => Some(normalize_path(text)),
            GrepMode::CountMatches => Some(normalize_path(
                text.rfind(':')
                    .filter(|index| *index > 0)
                    .map_or(text, |index| &text[..index]),
            )),
            GrepMode::Content => extract_content_file_path(text, path_class),
        },
    }
}

fn extract_content_file_path(line: &str, path_class: PathClass) -> Option<String> {
    static CONTENT_LINE_RE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"^(.*?)([:-])(\d+)([:-])").expect("static content regex"));
    if let Some(captures) = CONTENT_LINE_RE.captures(line)
        && captures.get(2).map(|value| value.as_str())
            == captures.get(4).map(|value| value.as_str())
    {
        return captures.get(1).map(|value| normalize_path(value.as_str()));
    }
    let index = no_line_number_content_separator_index(line, path_class);
    (index > 0).then(|| normalize_path(&line[..index]))
}

fn no_line_number_content_separator_index(line: &str, path_class: PathClass) -> usize {
    let from = if path_class == PathClass::Win32
        && line.as_bytes().get(1) == Some(&b':')
        && line.as_bytes().first().is_some_and(u8::is_ascii_alphabetic)
    {
        2
    } else {
        0
    };
    line[from..].find(':').map_or(0, |index| from + index)
}

fn content_payload_path_separator(payload: &str) -> char {
    let bytes = payload.as_bytes();
    let digits = bytes
        .iter()
        .take_while(|byte| byte.is_ascii_digit())
        .count();
    if bytes.get(digits) == Some(&b'-') {
        '-'
    } else {
        ':'
    }
}

fn strip_trailing_carriage_return(value: &str) -> &str {
    value.strip_suffix('\r').unwrap_or(value)
}

fn format_count_summary(lines: &[ParsedGrepLine], redacted_sensitive: bool) -> String {
    let mut total_matches = 0_u64;
    let mut total_files = 0_u64;
    for line in lines {
        let raw = match line {
            ParsedGrepLine::Record { payload, .. } => Some(payload.as_str()),
            ParsedGrepLine::Legacy(text) => text
                .rfind(':')
                .filter(|index| *index > 0)
                .map(|index| &text[index + 1..]),
            ParsedGrepLine::Separator => None,
        };
        if let Some(count) = raw.and_then(|raw| raw.parse::<u64>().ok()) {
            total_matches += count;
            total_files += 1;
        }
    }
    format!(
        "Found {total_matches} {} {} across {total_files} {}.",
        if redacted_sensitive {
            "total non-sensitive"
        } else {
            "total"
        },
        if total_matches == 1 {
            "occurrence"
        } else {
            "occurrences"
        },
        if total_files == 1 { "file" } else { "files" }
    )
}

fn normalize_path(path: &str) -> String {
    path.replace('\\', "/")
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nul_records_parse_and_format_like_ripgrep_output() {
        let parsed = parse_ripgrep_output(concat!("/w/a.rs\0", "12:hit\n"), GrepMode::Content);
        assert_eq!(
            parsed,
            vec![ParsedGrepLine::Record {
                file_path: "/w/a.rs".into(),
                payload: "12:hit".into()
            }]
        );
        assert_eq!(
            format_display_line(&parsed[0], GrepMode::Content, "/w", PathClass::Posix, true),
            "a.rs:12:hit"
        );
        assert_eq!(
            omit_incomplete_trailing_record("a\0x\nb\0partial", GrepMode::Content),
            "a\0x\n"
        );
    }

    #[test]
    fn count_summary_and_sensitive_filter_match_source() {
        let lines = vec![
            ParsedGrepLine::Record {
                file_path: "/w/a".into(),
                payload: "2".into(),
            },
            ParsedGrepLine::Record {
                file_path: "/w/b".into(),
                payload: "1".into(),
            },
        ];
        assert_eq!(
            format_count_summary(&lines, false),
            "Found 3 total occurrences across 2 files."
        );
    }
}
