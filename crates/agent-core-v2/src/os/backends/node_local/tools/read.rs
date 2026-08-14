//! The model-facing UTF-8 text file reader.
//!
//! Original: `packages/agent-core-v2/src/os/backends/node-local/tools/read.ts`.

use std::{collections::VecDeque, error::Error, path::Path, sync::Arc};

use async_trait::async_trait;
use futures_util::{StreamExt, future::BoxFuture};
use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::{
    _base::{
        di::instantiation::ServicesAccessorExt,
        exec_env::decode_text::{TextDecodeError, TextDecodeErrors, TextEncoding},
        text::line_endings::{LineEndingStyle, make_carriage_returns_visible},
    },
    agent::{
        media::file_type::{DetectFileTypeMode, FileTypeKind, MEDIA_SNIFF_BYTES, detect_file_type},
        tool_registry::{ToolContributionOptions, register_tool},
    },
    kosong::contract::tool::Tool,
    os::interface::{
        host_environment::{HOST_ENVIRONMENT_SERVICE_ID, HostEnvironmentHandle},
        host_file_system::{
            HOST_FILE_SYSTEM_SERVICE_ID, HostFileSystemServiceHandle, ReadTextOptions,
        },
        host_fs_errors::{HostFsError, OS_FS_NOT_DIRECTORY, OS_FS_NOT_FOUND},
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
            DEFAULT_WORKSPACE_ACCESS_POLICY, PathAccessOperation, WorkspaceConfig,
            extend_workspace_with_skill_roots, resolve_path_access_path,
        },
        rule_match::{PermissionPathMatchOptions, literal_rule_pattern, matches_path_rule_subject},
    },
};
use kimi_code_protocol::{FileIoOperation, ToolInputDisplay};

pub const MAX_LINES: usize = 1_000;
pub const MAX_LINE_LENGTH: usize = 2_000;
pub const MAX_BYTES: usize = 100 * 1_024;
const READ_DESCRIPTION_TEMPLATE: &str = include_str!("read.md");

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub struct ReadInput {
    pub path: String,
    #[serde(default)]
    pub line_offset: Option<i64>,
    #[serde(default)]
    pub n_lines: Option<usize>,
}

pub fn read_parameters() -> Map<String, Value> {
    to_input_json_schema(
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path to a text file. Relative paths resolve against the working directory; a path outside the working directory must be absolute. Directories are not supported; use `ls` via Bash for a known directory, or Glob for pattern search."
                },
                "line_offset": {
                    "anyOf": [
                        { "type": "integer", "minimum": 1 },
                        { "type": "integer", "minimum": -(MAX_LINES as i64), "maximum": -1 }
                    ],
                    "description": format!("The line number to start reading from. Omit to start at line 1. Negative values read from the end of the file; the absolute value cannot exceed {MAX_LINES}.")
                },
                "n_lines": {
                    "type": "integer",
                    "minimum": 1,
                    "description": format!("The number of lines to read; the tool also applies its internal cap. Omit to read up to the internal cap of {MAX_LINES} lines.")
                }
            },
            "required": ["path"]
        })
        .as_object()
        .cloned()
        .expect("Read schema is an object"),
    )
}

#[derive(Clone, Copy, Debug, Default)]
struct LineEndingFlags {
    has_crlf: bool,
    has_lf: bool,
    has_lone_cr: bool,
}

#[derive(Clone, Debug)]
struct ReadLineEntry {
    line_no: usize,
    raw_content: String,
}

struct RenderedLine {
    line: String,
    was_truncated: bool,
}

struct FinishReadResultInput {
    rendered_lines: Vec<String>,
    truncated_line_numbers: Vec<usize>,
    max_lines_reached: bool,
    max_bytes_reached: bool,
    line_ending_style: LineEndingStyle,
    start_line: usize,
    total_lines: usize,
    requested_lines: usize,
}

#[derive(Clone)]
pub struct ReadTool {
    fs: HostFileSystemServiceHandle,
    environment: HostEnvironmentHandle,
    workspace_context: SessionWorkspaceContextHandle,
    skill_catalog: Option<SessionSkillCatalogHandle>,
    definition: Tool,
}

impl ReadTool {
    pub fn new(
        fs: HostFileSystemServiceHandle,
        environment: HostEnvironmentHandle,
        workspace_context: SessionWorkspaceContextHandle,
        skill_catalog: Option<SessionSkillCatalogHandle>,
    ) -> Self {
        Self {
            fs,
            environment,
            workspace_context,
            skill_catalog,
            definition: Tool {
                name: "Read".into(),
                description: render_read_description(),
                parameters: read_parameters(),
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

    async fn execute(&self, args: ReadInput, safe_path: String) -> ExecutableToolResult {
        match self.execute_inner(&args, &safe_path).await {
            Ok(result) => result,
            Err(error) if is_text_decode_error(&error) => {
                ExecutableToolResult::error(not_readable_file_output(&args.path))
            }
            Err(error) => ExecutableToolResult::error(error.to_string()),
        }
    }

    async fn execute_inner(
        &self,
        args: &ReadInput,
        safe_path: &str,
    ) -> Result<ExecutableToolResult, HostFsError> {
        let stat = match self.fs.stat(Path::new(safe_path)).await {
            Ok(stat) => stat,
            Err(error) if is_file_not_found_error(&error) => {
                return Ok(ExecutableToolResult::error(format!(
                    "\"{}\" does not exist.",
                    args.path
                )));
            }
            Err(error) => return Err(error),
        };
        if !stat.is_file {
            return Ok(ExecutableToolResult::error(format!(
                "\"{}\" is not a file.",
                args.path
            )));
        }

        let header = self
            .fs
            .read_bytes(Path::new(safe_path), Some(MEDIA_SNIFF_BYTES))
            .await?;
        let file_type = detect_file_type(safe_path, Some(&header), DetectFileTypeMode::Text);
        if matches!(file_type.kind, FileTypeKind::Image | FileTypeKind::Video) {
            let kind = if file_type.kind == FileTypeKind::Image {
                "image"
            } else {
                "video"
            };
            return Ok(ExecutableToolResult::error(format!(
                "\"{}\" is a {kind} file. Use ReadMediaFile to read image or video files.",
                args.path
            )));
        }
        if file_type.kind == FileTypeKind::Unknown {
            return Ok(ExecutableToolResult::error(not_readable_file_output(
                &args.path,
            )));
        }

        let line_offset = args.line_offset.unwrap_or(1);
        let requested_lines = args.n_lines.unwrap_or(MAX_LINES);
        let effective_limit = requested_lines.min(MAX_LINES);
        if line_offset < 0 {
            self.read_tail(
                safe_path,
                &args.path,
                line_offset,
                effective_limit,
                requested_lines,
            )
            .await
        } else {
            self.read_forward(
                safe_path,
                &args.path,
                line_offset as usize,
                effective_limit,
                requested_lines,
            )
            .await
        }
    }

    async fn read_forward(
        &self,
        safe_path: &str,
        display_path: &str,
        line_offset: usize,
        effective_limit: usize,
        requested_lines: usize,
    ) -> Result<ExecutableToolResult, HostFsError> {
        let mut entries = Vec::new();
        let mut flags = LineEndingFlags::default();
        let mut current_line_no = 0;
        let mut max_lines_reached = false;
        let mut collection_closed = false;
        let mut stream = self.fs.read_lines(
            Path::new(safe_path),
            Some(ReadTextOptions {
                encoding: TextEncoding::Utf8,
                errors: TextDecodeErrors::Strict,
            }),
        );

        while let Some(raw_line) = stream.next().await {
            let raw_line = raw_line?;
            if raw_line.contains('\0') {
                return Ok(ExecutableToolResult::error(not_readable_file_output(
                    display_path,
                )));
            }
            current_line_no += 1;
            update_line_ending_flags(&mut flags, &raw_line);
            if collection_closed {
                if effective_limit >= MAX_LINES && current_line_no >= line_offset {
                    max_lines_reached = true;
                }
                continue;
            }
            if current_line_no < line_offset {
                continue;
            }
            if entries.len() >= effective_limit {
                if effective_limit >= MAX_LINES {
                    max_lines_reached = true;
                }
                collection_closed = true;
                continue;
            }
            entries.push(ReadLineEntry {
                line_no: current_line_no,
                raw_content: strip_trailing_lf(raw_line),
            });
            if entries.len() >= effective_limit {
                collection_closed = true;
            }
        }

        let style = line_ending_style_from_flags(flags);
        let (rendered_lines, truncated_line_numbers, max_bytes_reached) =
            render_entries(&entries, style);
        Ok(finish_read_result(FinishReadResultInput {
            start_line: if entries.is_empty() { 0 } else { line_offset },
            rendered_lines,
            truncated_line_numbers,
            max_lines_reached,
            max_bytes_reached,
            line_ending_style: style,
            total_lines: current_line_no,
            requested_lines,
        }))
    }

    async fn read_tail(
        &self,
        safe_path: &str,
        display_path: &str,
        line_offset: i64,
        effective_limit: usize,
        requested_lines: usize,
    ) -> Result<ExecutableToolResult, HostFsError> {
        let tail_count = line_offset.unsigned_abs() as usize;
        let mut entries = VecDeque::with_capacity(tail_count.saturating_add(1));
        let mut flags = LineEndingFlags::default();
        let mut current_line_no = 0;
        let mut stream = self.fs.read_lines(
            Path::new(safe_path),
            Some(ReadTextOptions {
                encoding: TextEncoding::Utf8,
                errors: TextDecodeErrors::Strict,
            }),
        );
        while let Some(raw_line) = stream.next().await {
            let raw_line = raw_line?;
            if raw_line.contains('\0') {
                return Ok(ExecutableToolResult::error(not_readable_file_output(
                    display_path,
                )));
            }
            current_line_no += 1;
            update_line_ending_flags(&mut flags, &raw_line);
            entries.push_back(ReadLineEntry {
                line_no: current_line_no,
                raw_content: strip_trailing_lf(raw_line),
            });
            if entries.len() > tail_count {
                entries.pop_front();
            }
        }
        Ok(finish_tail_entries(
            entries.into_iter().collect(),
            flags,
            effective_limit,
            current_line_no,
            requested_lines,
        ))
    }
}

#[async_trait]
impl ExecutableTool for ReadTool {
    type Input = ReadInput;

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
        let safe_path = match resolve_path_access_path(
            &args.path,
            &info,
            &workspace,
            PathAccessOperation::Read,
            DEFAULT_WORKSPACE_ACCESS_POLICY,
            true,
        ) {
            Ok(path) => path,
            Err(error) => {
                return ToolExecution::Error(ExecutableToolResult::error(error.to_string()));
            }
        };
        let rule_path = safe_path.clone();
        let rule_cwd = workspace.workspace_dir.clone();
        let rule_home = info.home_dir.clone();
        let path_class = info.path_class;
        let tool = self.clone();
        let execution_args = args.clone();
        let execution_path = safe_path.clone();
        let mut execution = RunnableToolExecution::new(
            literal_rule_pattern("Read", &safe_path),
            Arc::new(move |_context: ExecutableToolContext| {
                let tool = tool.clone();
                let args = execution_args.clone();
                let path = execution_path.clone();
                Box::pin(async move { tool.execute(args, path).await })
                    as BoxFuture<'static, ExecutableToolResult>
            }),
        );
        execution.accesses = Some(ToolAccess::read_file(safe_path.clone()));
        execution.description = Some(format!("Reading {}", args.path));
        execution.display = Some(ToolInputDisplay::FileIo {
            operation: FileIoOperation::Read,
            path: safe_path,
            detail: None,
            content: None,
            before: None,
            after: None,
        });
        execution.matches_rule = Some(Arc::new(move |rule_args| {
            matches_path_rule_subject(
                rule_args,
                &rule_path,
                PermissionPathMatchOptions {
                    cwd: Some(&rule_cwd),
                    path_class: Some(path_class),
                    home_dir: Some(&rule_home),
                    case_insensitive_paths: None,
                },
            )
        }));
        ToolExecution::Runnable(execution)
    }
}

pub fn register_read_tool() {
    register_tool(
        Arc::new(|accessor| {
            let skill_catalog = accessor
                .get(SESSION_SKILL_CATALOG_ID)
                .ok()
                .map(|catalog| (*catalog).clone());
            Ok(Arc::new(ReadTool::new(
                (*accessor.get(HOST_FILE_SYSTEM_SERVICE_ID)?).clone(),
                (*accessor.get(HOST_ENVIRONMENT_SERVICE_ID)?).clone(),
                (*accessor.get(SESSION_WORKSPACE_CONTEXT_ID)?).clone(),
                skill_catalog,
            )) as Arc<dyn crate::tool::ErasedExecutableTool>)
        }),
        ToolContributionOptions::default(),
    );
}

fn render_read_description() -> String {
    READ_DESCRIPTION_TEMPLATE
        .replace("${MAX_LINES}", &MAX_LINES.to_string())
        .replace("${MAX_BYTES_KB}", &(MAX_BYTES / 1_024).to_string())
        .replace("${MAX_LINE_LENGTH}", &MAX_LINE_LENGTH.to_string())
}

fn truncate_line(line: &str, max_length: usize) -> String {
    let units = line.encode_utf16().collect::<Vec<_>>();
    if units.len() <= max_length {
        return line.to_owned();
    }
    let marker = "...";
    let target = max_length.max(marker.len());
    let mut truncated = String::from_utf16_lossy(&units[..target - marker.len()]);
    truncated.push_str(marker);
    truncated
}

fn strip_trailing_lf(mut line: String) -> String {
    if line.ends_with('\n') {
        line.pop();
    }
    line
}

fn update_line_ending_flags(flags: &mut LineEndingFlags, text: &str) {
    let bytes = text.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'\r' {
            if bytes.get(index + 1) == Some(&b'\n') {
                flags.has_crlf = true;
                index += 2;
            } else {
                flags.has_lone_cr = true;
                index += 1;
            }
        } else {
            if bytes[index] == b'\n' {
                flags.has_lf = true;
            }
            index += 1;
        }
    }
}

fn line_ending_style_from_flags(flags: LineEndingFlags) -> LineEndingStyle {
    if flags.has_lone_cr || (flags.has_crlf && flags.has_lf) {
        LineEndingStyle::Mixed
    } else if flags.has_crlf {
        LineEndingStyle::CrLf
    } else {
        LineEndingStyle::Lf
    }
}

fn render_line(entry: &ReadLineEntry, style: LineEndingStyle) -> RenderedLine {
    let model_content = if style == LineEndingStyle::CrLf {
        entry
            .raw_content
            .strip_suffix('\r')
            .unwrap_or(&entry.raw_content)
    } else {
        &entry.raw_content
    };
    let truncated = truncate_line(model_content, MAX_LINE_LENGTH);
    let was_truncated = truncated != model_content;
    let content = if style == LineEndingStyle::Mixed {
        make_carriage_returns_visible(&truncated)
    } else {
        truncated
    };
    RenderedLine {
        line: format!("{}\t{content}", entry.line_no),
        was_truncated,
    }
}

fn rendered_line_bytes(line: &str, first: bool) -> usize {
    usize::from(!first) + line.len()
}

fn render_entries(
    entries: &[ReadLineEntry],
    style: LineEndingStyle,
) -> (Vec<String>, Vec<usize>, bool) {
    let mut lines = Vec::new();
    let mut truncated = Vec::new();
    let mut bytes = 0;
    let mut max_bytes_reached = false;
    for entry in entries {
        let rendered = render_line(entry, style);
        let line_bytes = rendered_line_bytes(&rendered.line, lines.is_empty());
        if !lines.is_empty() && bytes + line_bytes > MAX_BYTES {
            max_bytes_reached = true;
            break;
        }
        if rendered.was_truncated {
            truncated.push(entry.line_no);
        }
        bytes += line_bytes;
        lines.push(rendered.line);
        if bytes >= MAX_BYTES {
            max_bytes_reached = true;
            break;
        }
    }
    (lines, truncated, max_bytes_reached)
}

fn finish_tail_entries(
    entries: Vec<ReadLineEntry>,
    flags: LineEndingFlags,
    effective_limit: usize,
    total_lines: usize,
    requested_lines: usize,
) -> ExecutableToolResult {
    let style = line_ending_style_from_flags(flags);
    let mut candidates = entries
        .into_iter()
        .take(effective_limit)
        .map(|entry| {
            let rendered = render_line(&entry, style);
            (entry, rendered)
        })
        .collect::<Vec<_>>();
    let total_bytes = candidates
        .iter()
        .enumerate()
        .map(|(index, (_, rendered))| rendered_line_bytes(&rendered.line, index == 0))
        .sum::<usize>();
    let mut max_bytes_reached = false;
    if total_bytes > MAX_BYTES {
        max_bytes_reached = true;
        let mut kept = VecDeque::new();
        let mut bytes = 0;
        for candidate in candidates.into_iter().rev() {
            let line_bytes = rendered_line_bytes(&candidate.1.line, kept.is_empty());
            if bytes + line_bytes > MAX_BYTES {
                break;
            }
            bytes += line_bytes;
            kept.push_front(candidate);
        }
        candidates = kept.into_iter().collect();
    }
    let start_line = candidates.first().map_or(0, |(entry, _)| entry.line_no);
    let mut rendered_lines = Vec::new();
    let mut truncated_line_numbers = Vec::new();
    for (entry, rendered) in candidates {
        rendered_lines.push(rendered.line);
        if rendered.was_truncated {
            truncated_line_numbers.push(entry.line_no);
        }
    }
    finish_read_result(FinishReadResultInput {
        rendered_lines,
        truncated_line_numbers,
        max_lines_reached: false,
        max_bytes_reached,
        line_ending_style: style,
        start_line,
        total_lines,
        requested_lines,
    })
}

fn finish_read_result(input: FinishReadResultInput) -> ExecutableToolResult {
    let note = format!("<system>{}</system>", finish_message(&input));
    let mut result = ExecutableToolResult::success(input.rendered_lines.join("\n"));
    result.note = Some(note);
    result
}

fn finish_message(input: &FinishReadResultInput) -> String {
    let count = input.rendered_lines.len();
    let mut parts = if count > 0 {
        vec![format!(
            "{count} {} read from file starting from line {}.",
            if count == 1 { "line" } else { "lines" },
            input.start_line
        )]
    } else {
        vec!["No lines read from file.".into()]
    };
    parts.push(format!("Total lines in file: {}.", input.total_lines));
    if input.max_lines_reached {
        parts.push(format!("Max {MAX_LINES} lines reached."));
    } else if input.max_bytes_reached {
        parts.push(format!("Max {MAX_BYTES} bytes reached."));
    } else if count < input.requested_lines {
        parts.push("End of file reached.".into());
    }
    if !input.truncated_line_numbers.is_empty() {
        parts.push(format!(
            "Lines [{}] were truncated.",
            input
                .truncated_line_numbers
                .iter()
                .map(usize::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if input.line_ending_style == LineEndingStyle::Mixed {
        parts.push(
            "Mixed or lone carriage-return line endings are shown as \\r. Use exact \\r\\n or \\r escapes in Edit.old_string for those lines."
                .into(),
        );
    }
    parts.join(" ")
}

fn is_file_not_found_error(error: &HostFsError) -> bool {
    matches!(error.code(), OS_FS_NOT_FOUND | OS_FS_NOT_DIRECTORY)
}

fn is_text_decode_error(error: &HostFsError) -> bool {
    let mut current: Option<&(dyn Error + 'static)> = Some(error);
    while let Some(candidate) = current {
        if candidate.downcast_ref::<TextDecodeError>().is_some() {
            return true;
        }
        current = candidate.source();
    }
    false
}

fn not_readable_file_output(path: &str) -> String {
    format!(
        "\"{path}\" is not readable as UTF-8 text. If it is an image or video, use ReadMediaFile. For other binary formats, use Bash or an MCP tool if available."
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::_base::{
        errors::errors::{Error2Options, ErrorCause},
        exec_env::decode_text::{TextDecodeErrors, TextEncoding, decode_text_with_errors},
    };
    use crate::os::interface::host_fs_errors::OS_FS_UNKNOWN;

    #[test]
    fn rendering_preserves_source_limits_and_line_endings() {
        let mut flags = LineEndingFlags::default();
        update_line_ending_flags(&mut flags, "a\r\nb\r");
        assert_eq!(line_ending_style_from_flags(flags), LineEndingStyle::Mixed);
        assert_eq!(
            truncate_line(&"x".repeat(MAX_LINE_LENGTH + 1), MAX_LINE_LENGTH).len(),
            MAX_LINE_LENGTH
        );
        assert_eq!(
            render_line(
                &ReadLineEntry {
                    line_no: 7,
                    raw_content: "hello\r".into()
                },
                LineEndingStyle::CrLf
            )
            .line,
            "7\thello"
        );
    }

    #[test]
    fn detects_text_decode_failures_by_type_not_message() {
        let decode_error = decode_text_with_errors(
            b"\xff\xfe\x00",
            TextEncoding::Utf8,
            TextDecodeErrors::Strict,
            false,
        )
        .unwrap_err();
        let error = HostFsError::with_options(
            OS_FS_UNKNOWN,
            "read failed",
            Error2Options {
                cause: Some(ErrorCause::Error(Arc::new(decode_error))),
                ..Error2Options::default()
            },
        );
        assert!(is_text_decode_error(&error));

        let io_error = HostFsError::with_options(
            OS_FS_UNKNOWN,
            "read failed: encoded data was not valid utf-8, but the wording alone must not classify",
            Error2Options::default(),
        );
        assert!(!is_text_decode_error(&io_error));
    }
}
