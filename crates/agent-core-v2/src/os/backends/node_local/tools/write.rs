//! The model-facing UTF-8 file writer.
//!
//! Original: `packages/agent-core-v2/src/os/backends/node-local/tools/write.ts`.

use std::{path::Path, sync::Arc};

use async_trait::async_trait;
use futures_util::future::BoxFuture;
use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::{
    _base::di::instantiation::ServicesAccessorExt,
    agent::tool_registry::{ToolContributionOptions, register_tool},
    kosong::contract::tool::Tool,
    os::interface::{
        host_environment::{HOST_ENVIRONMENT_SERVICE_ID, HostEnvironmentHandle},
        host_file_system::{
            HOST_FILE_SYSTEM_SERVICE_ID, HostFileStat, HostFileSystemServiceHandle,
        },
        host_fs_errors::OS_FS_NOT_FOUND,
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

const WRITE_DESCRIPTION: &str = include_str!("write.md");

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum WriteMode {
    #[default]
    Overwrite,
    Append,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub struct WriteInput {
    pub path: String,
    pub content: String,
    #[serde(default)]
    pub mode: Option<WriteMode>,
}

pub fn write_parameters() -> Map<String, Value> {
    to_input_json_schema(
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path to the file to create, append to, or completely overwrite. Relative paths resolve against the working directory; a path outside the working directory must be absolute. Missing parent directories are created automatically."
                },
                "content": {
                    "type": "string",
                    "description": "Raw full file content to write exactly as provided. This does not use the Read/Edit text view."
                },
                "mode": {
                    "type": "string",
                    "enum": ["overwrite", "append"],
                    "description": "Write mode. Defaults to overwrite. append adds content to the end exactly as provided and does not add a newline."
                }
            },
            "required": ["path", "content"]
        })
        .as_object()
        .cloned()
        .expect("Write schema is an object"),
    )
}

#[derive(Clone)]
pub struct WriteTool {
    fs: HostFileSystemServiceHandle,
    environment: HostEnvironmentHandle,
    workspace_context: SessionWorkspaceContextHandle,
    skill_catalog: Option<SessionSkillCatalogHandle>,
    definition: Tool,
}

impl WriteTool {
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
                name: "Write".into(),
                description: WRITE_DESCRIPTION.into(),
                parameters: write_parameters(),
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

    async fn execute(&self, args: WriteInput, safe_path: String) -> ExecutableToolResult {
        if let Some(error) = self.ensure_parent_directory(&safe_path).await {
            return ExecutableToolResult::error(error);
        }

        let mode = args.mode.unwrap_or_default();
        let result = match mode {
            WriteMode::Append => {
                self.fs
                    .append_text(Path::new(&safe_path), &args.content)
                    .await
            }
            WriteMode::Overwrite => {
                self.fs
                    .write_text(Path::new(&safe_path), &args.content)
                    .await
            }
        };
        match result {
            Ok(()) => {
                let verb = if mode == WriteMode::Append {
                    "Appended"
                } else {
                    "Wrote"
                };
                ExecutableToolResult::success(format!(
                    "{verb} {} bytes to {}",
                    args.content.len(),
                    args.path
                ))
            }
            Err(error) if error.code() == OS_FS_NOT_FOUND => ExecutableToolResult::error(format!(
                "Failed to write {}: parent directory does not exist.",
                args.path
            )),
            Err(error) => ExecutableToolResult::error(error.to_string()),
        }
    }

    async fn ensure_parent_directory(&self, safe_path: &str) -> Option<String> {
        let parent = Path::new(safe_path)
            .parent()
            .unwrap_or_else(|| Path::new(""));
        let stat: HostFileStat = match self.fs.stat(parent).await {
            Ok(stat) => stat,
            Err(error) if error.code() == OS_FS_NOT_FOUND => {
                return self
                    .fs
                    .create_dir(parent, true)
                    .await
                    .err()
                    .map(|error| error.to_string());
            }
            Err(_) => return None,
        };
        (!stat.is_directory).then(|| {
            format!(
                "Parent path is not a directory: {}.",
                parent.to_string_lossy()
            )
        })
    }
}

#[async_trait]
impl ExecutableTool for WriteTool {
    type Input = WriteInput;

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
            PathAccessOperation::Write,
            DEFAULT_WORKSPACE_ACCESS_POLICY,
            true,
        ) {
            Ok(path) => path,
            Err(error) => {
                return ToolExecution::Error(ExecutableToolResult::error(error.to_string()));
            }
        };
        let approval_rule = literal_rule_pattern("Write", &safe_path);
        let rule_path = safe_path.clone();
        let rule_cwd = workspace.workspace_dir.clone();
        let rule_home = info.home_dir.clone();
        let path_class = info.path_class;
        let tool = self.clone();
        let execution_args = args.clone();
        let execution_path = safe_path.clone();
        let mut execution = RunnableToolExecution::new(
            approval_rule,
            Arc::new(move |_context: ExecutableToolContext| {
                let tool = tool.clone();
                let args = execution_args.clone();
                let path = execution_path.clone();
                Box::pin(async move { tool.execute(args, path).await })
                    as BoxFuture<'static, ExecutableToolResult>
            }),
        );
        execution.accesses = Some(ToolAccess::write_file(safe_path.clone()));
        execution.description = Some(format!("Writing {}", args.path));
        execution.display = Some(ToolInputDisplay::FileIo {
            operation: FileIoOperation::Write,
            path: safe_path,
            detail: None,
            content: Some(args.content),
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

pub fn register_write_tool() {
    register_tool(
        Arc::new(|accessor| {
            let skill_catalog = accessor
                .get(SESSION_SKILL_CATALOG_ID)
                .ok()
                .map(|catalog| (*catalog).clone());
            Ok(Arc::new(WriteTool::new(
                (*accessor.get(HOST_FILE_SYSTEM_SERVICE_ID)?).clone(),
                (*accessor.get(HOST_ENVIRONMENT_SERVICE_ID)?).clone(),
                (*accessor.get(SESSION_WORKSPACE_CONTEXT_ID)?).clone(),
                skill_catalog,
            )) as Arc<dyn crate::tool::ErasedExecutableTool>)
        }),
        ToolContributionOptions::default(),
    );
}
