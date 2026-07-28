//! The model-facing exact text replacement tool.
//!
//! Original: `packages/agent-core-v2/src/app/edit/tools/edit.ts`.

use std::sync::Arc;

use async_trait::async_trait;
use futures_util::future::BoxFuture;
use serde::{Deserialize, Deserializer};
use serde_json::{Map, Value, json};

use crate::{
    _base::di::instantiation::ServicesAccessorExt,
    agent::tool_registry::{ToolContributionOptions, register_tool},
    app::edit::{FILE_EDIT_SERVICE_ID, FileEditInput, FileEditResult, FileEditServiceHandle},
    kosong::contract::tool::Tool,
    os::interface::host_environment::{HOST_ENVIRONMENT_SERVICE_ID, HostEnvironmentHandle},
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

const EDIT_DESCRIPTION: &str = include_str!("edit.md");
const NO_CHANGES_MESSAGE: &str =
    "No changes to make: old_string and new_string are exactly the same.";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EditInput {
    pub path: String,
    pub old_string: String,
    pub new_string: String,
    pub replace_all: Option<bool>,
}

impl<'de> Deserialize<'de> for EditInput {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = Value::deserialize(deserializer)?;
        parse_edit_input(&value).map_err(serde::de::Error::custom)
    }
}

pub fn parse_edit_input(value: &Value) -> Result<EditInput, String> {
    let object = value
        .as_object()
        .ok_or_else(|| "Edit input must be an object".to_owned())?;
    let string = |name: &str| {
        object
            .get(name)
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| format!("{name} must be a string"))
    };
    let old_string = string("old_string")?;
    if old_string.is_empty() {
        return Err("old_string must not be empty".into());
    }
    let replace_all = match object.get("replace_all") {
        None => None,
        Some(Value::Bool(value)) => Some(*value),
        Some(_) => return Err("replace_all must be a boolean".into()),
    };
    Ok(EditInput {
        path: string("path")?,
        old_string,
        new_string: string("new_string")?,
        replace_all,
    })
}

pub fn edit_parameters() -> Map<String, Value> {
    to_input_json_schema(
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path to the text file to edit. Relative paths resolve against the working directory; a path outside the working directory must be absolute."
                },
                "old_string": {
                    "type": "string",
                    "minLength": 1,
                    "description": "Exact content to replace from the Read output view, without the line-number prefix. Use LF for pure CRLF files; use actual \\r escapes where Read shows \\r."
                },
                "new_string": {
                    "type": "string",
                    "description": "Replacement text in the same Read output view. LF is written back as CRLF only for pure CRLF files."
                },
                "replace_all": {
                    "type": "boolean",
                    "description": "Set true only when every occurrence of old_string should be replaced."
                }
            },
            "required": ["path", "old_string", "new_string"]
        })
        .as_object()
        .cloned()
        .expect("Edit schema is an object"),
    )
}

#[derive(Clone)]
pub struct EditTool {
    editor: FileEditServiceHandle,
    environment: HostEnvironmentHandle,
    workspace_context: SessionWorkspaceContextHandle,
    skill_catalog: Option<SessionSkillCatalogHandle>,
    definition: Tool,
}

impl EditTool {
    pub fn new(
        editor: FileEditServiceHandle,
        environment: HostEnvironmentHandle,
        workspace_context: SessionWorkspaceContextHandle,
        skill_catalog: Option<SessionSkillCatalogHandle>,
    ) -> Self {
        Self {
            editor,
            environment,
            workspace_context,
            skill_catalog,
            definition: Tool {
                name: "Edit".into(),
                description: EDIT_DESCRIPTION.into(),
                parameters: edit_parameters(),
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

    async fn execute(&self, args: EditInput, safe_path: String) -> ExecutableToolResult {
        if args.old_string == args.new_string {
            return ExecutableToolResult::error(NO_CHANGES_MESSAGE);
        }
        match self
            .editor
            .edit(FileEditInput {
                path: safe_path,
                display_path: args.path.clone(),
                old_string: args.old_string,
                new_string: args.new_string,
                replace_all: args.replace_all.unwrap_or(false),
            })
            .await
        {
            FileEditResult::Ok { count } => {
                let word = if count == 1 {
                    "occurrence"
                } else {
                    "occurrences"
                };
                ExecutableToolResult::success(format!("Replaced {count} {word} in {}", args.path))
            }
            FileEditResult::Err { error } => ExecutableToolResult::error(error),
        }
    }
}

#[async_trait]
impl ExecutableTool for EditTool {
    type Input = EditInput;

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

        let approval_rule = literal_rule_pattern("Edit", &safe_path);
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
        execution.accesses = Some(ToolAccess::read_write_file(safe_path.clone()));
        execution.description = Some(format!("Editing {}", args.path));
        execution.display = Some(ToolInputDisplay::FileIo {
            operation: FileIoOperation::Edit,
            path: safe_path,
            detail: None,
            content: None,
            before: Some(args.old_string),
            after: Some(args.new_string),
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

pub fn register_edit_tool() {
    register_tool(
        Arc::new(|accessor| {
            let skill_catalog = accessor
                .get(SESSION_SKILL_CATALOG_ID)
                .ok()
                .map(|catalog| (*catalog).clone());
            Ok(Arc::new(EditTool::new(
                (*accessor.get(FILE_EDIT_SERVICE_ID)?).clone(),
                (*accessor.get(HOST_ENVIRONMENT_SERVICE_ID)?).clone(),
                (*accessor.get(SESSION_WORKSPACE_CONTEXT_ID)?).clone(),
                skill_catalog,
            )) as Arc<dyn crate::tool::ErasedExecutableTool>)
        }),
        ToolContributionOptions::default(),
    );
}

#[cfg(test)]
mod tests {
    use std::{
        path::PathBuf,
        sync::{Arc, Mutex},
    };

    use async_trait::async_trait;

    use super::*;
    use crate::{
        _base::{
            errors::errors::BugIndicatingError,
            exec_env::environment_probe::{
                HostEnvironmentInfo, HostEnvironmentProbeError, PathClass, ShellName,
            },
            utils::abort::AbortController,
        },
        app::edit::FileEditServiceContract,
        os::interface::host_environment::HostEnvironment,
        session::workspace_context::{
            PathAccessError as WorkspacePathAccessError,
            PathAccessOperation as WorkspacePathAccessOperation, SessionWorkspaceContextContract,
        },
        tool::{ExecutableToolOutput, ToolResourceAccess},
    };

    struct TestEditor {
        result: Mutex<FileEditResult>,
        inputs: Mutex<Vec<FileEditInput>>,
    }

    impl TestEditor {
        fn new(result: FileEditResult) -> Self {
            Self {
                result: Mutex::new(result),
                inputs: Mutex::new(Vec::new()),
            }
        }

        fn inputs(&self) -> Vec<FileEditInput> {
            self.inputs.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl FileEditServiceContract for TestEditor {
        async fn edit(&self, input: FileEditInput) -> FileEditResult {
            self.inputs.lock().unwrap().push(input);
            self.result.lock().unwrap().clone()
        }
    }

    struct TestEnvironment {
        home_dir: String,
    }

    #[async_trait]
    impl HostEnvironment for TestEnvironment {
        async fn ready(&self) -> Result<(), HostEnvironmentProbeError> {
            Ok(())
        }

        fn info(&self) -> Result<HostEnvironmentInfo, BugIndicatingError> {
            Ok(HostEnvironmentInfo {
                os_kind: "Linux".into(),
                os_arch: "x86_64".into(),
                os_version: "test".into(),
                shell_name: ShellName::Bash,
                shell_path: "/bin/bash".into(),
                path_class: PathClass::Posix,
                home_dir: self.home_dir.clone(),
            })
        }
    }

    struct TestWorkspace {
        work_dir: PathBuf,
        additional_dirs: Vec<PathBuf>,
    }

    impl SessionWorkspaceContextContract for TestWorkspace {
        fn work_dir(&self) -> PathBuf {
            self.work_dir.clone()
        }

        fn additional_dirs(&self) -> Vec<PathBuf> {
            self.additional_dirs.clone()
        }

        fn set_work_dir(&self, _work_dir: &str) -> std::io::Result<()> {
            Ok(())
        }

        fn set_additional_dirs(&self, _dirs: &[String]) -> std::io::Result<()> {
            Ok(())
        }

        fn resolve(&self, relative: &str) -> PathBuf {
            self.work_dir.join(relative)
        }

        fn is_within(&self, _absolute_path: &str) -> bool {
            true
        }

        fn assert_allowed(
            &self,
            absolute_path: &str,
            _operation: WorkspacePathAccessOperation,
        ) -> Result<PathBuf, WorkspacePathAccessError> {
            Ok(PathBuf::from(absolute_path))
        }

        fn add_additional_dir(&self, _dir: &str) -> std::io::Result<()> {
            Ok(())
        }

        fn remove_additional_dir(&self, _dir: &str) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn tool(
        editor: Arc<TestEditor>,
        work_dir: &str,
        home_dir: &str,
    ) -> (EditTool, Arc<TestEditor>) {
        let editor_handle: Arc<dyn FileEditServiceContract> = editor.clone();
        let environment: Arc<dyn HostEnvironment> = Arc::new(TestEnvironment {
            home_dir: home_dir.into(),
        });
        let workspace: Arc<dyn SessionWorkspaceContextContract> = Arc::new(TestWorkspace {
            work_dir: PathBuf::from(work_dir),
            additional_dirs: Vec::new(),
        });
        (
            EditTool::new(
                FileEditServiceHandle(editor_handle),
                HostEnvironmentHandle(environment),
                SessionWorkspaceContextHandle(workspace),
                None,
            ),
            editor,
        )
    }

    fn input(path: &str, old_string: &str, new_string: &str) -> EditInput {
        EditInput {
            path: path.into(),
            old_string: old_string.into(),
            new_string: new_string.into(),
            replace_all: None,
        }
    }

    fn context() -> ExecutableToolContext {
        ExecutableToolContext {
            turn_id: 0,
            tool_call_id: "call_edit".into(),
            trace: None,
            metadata: None,
            signal: AbortController::new().signal(),
            on_update: None,
            on_foreground_task_start: None,
        }
    }

    fn text_output(result: &ExecutableToolResult) -> &str {
        match &result.output {
            ExecutableToolOutput::Text(output) => output,
            ExecutableToolOutput::Content(_) => panic!("expected text output"),
        }
    }

    #[test]
    fn schema_and_input_validation_match_the_source_contract() {
        let parameters = edit_parameters();
        assert_eq!(
            parameters["required"],
            json!(["path", "old_string", "new_string"])
        );
        assert_eq!(parameters["additionalProperties"], false);
        assert_eq!(parameters["properties"]["old_string"]["minLength"], 1);
        assert!(EDIT_DESCRIPTION.contains("Read the target file before every Edit"));
        assert!(EDIT_DESCRIPTION.contains("DO NOT issue consecutive Edit calls"));

        let parsed: EditInput = serde_json::from_value(json!({
            "path": "/tmp/a.txt",
            "old_string": "old",
            "new_string": "new",
            "unknown": "stripped"
        }))
        .unwrap();
        assert_eq!(parsed, input("/tmp/a.txt", "old", "new"));
        for invalid in [
            json!(null),
            json!({"path": "/tmp/a.txt", "old_string": "", "new_string": "new"}),
            json!({"path": "/tmp/a.txt", "old_string": "old"}),
            json!({"path": "/tmp/a.txt", "old_string": "old", "new_string": "new", "replace_all": null}),
        ] {
            assert!(
                serde_json::from_value::<EditInput>(invalid.clone()).is_err(),
                "{invalid}"
            );
        }
    }

    #[tokio::test]
    async fn resolution_declares_diff_access_and_literal_approval_rule() {
        let editor = Arc::new(TestEditor::new(FileEditResult::Ok { count: 1 }));
        let (tool, _) = tool(editor, "/", "/home/test");
        let ToolExecution::Runnable(execution) = tool
            .resolve_execution(input("/tmp/foo.ts", "a\nb\nc", "a\nB\nc"))
            .await
        else {
            panic!("expected runnable execution");
        };

        assert_eq!(
            execution.accesses,
            Some(ToolAccess::read_write_file("/tmp/foo.ts"))
        );
        assert_eq!(execution.approval_rule, "Edit(/tmp/foo.ts)");
        assert_eq!(
            execution.display,
            Some(ToolInputDisplay::FileIo {
                operation: FileIoOperation::Edit,
                path: "/tmp/foo.ts".into(),
                detail: None,
                content: None,
                before: Some("a\nb\nc".into()),
                after: Some("a\nB\nc".into()),
            })
        );
        assert_eq!(
            execution.description.as_deref(),
            Some("Editing /tmp/foo.ts")
        );
        assert!(execution.matches_rule.as_ref().unwrap()("/tmp/foo.ts"));
        assert!(matches!(
            execution.accesses.as_deref(),
            Some([ToolResourceAccess::File(access)])
                if access.path == "/tmp/foo.ts"
        ));
    }

    #[tokio::test]
    async fn execution_maps_paths_defaults_and_result_wording() {
        let editor = Arc::new(TestEditor::new(FileEditResult::Ok { count: 2 }));
        let (tool, editor) = tool(editor, "/workspace", "/home/test");
        let ToolExecution::Runnable(execution) = tool
            .resolve_execution(EditInput {
                replace_all: Some(true),
                ..input("~/notes/today.txt", "old", "new")
            })
            .await
        else {
            panic!("expected runnable execution");
        };
        let result = execution.execute(context()).await;

        assert!(!result.is_error);
        assert_eq!(
            text_output(&result),
            "Replaced 2 occurrences in ~/notes/today.txt"
        );
        assert_eq!(
            editor.inputs(),
            vec![FileEditInput {
                path: "/home/test/notes/today.txt".into(),
                display_path: "~/notes/today.txt".into(),
                old_string: "old".into(),
                new_string: "new".into(),
                replace_all: true,
            }]
        );
    }

    #[tokio::test]
    async fn no_op_is_rejected_before_the_edit_service() {
        let editor = Arc::new(TestEditor::new(FileEditResult::Ok { count: 1 }));
        let (tool, editor) = tool(editor, "/", "/home/test");
        let ToolExecution::Runnable(execution) = tool
            .resolve_execution(input("/tmp/a.txt", "same", "same"))
            .await
        else {
            panic!("expected runnable execution");
        };
        let result = execution.execute(context()).await;

        assert!(result.is_error);
        assert_eq!(text_output(&result), NO_CHANGES_MESSAGE);
        assert!(editor.inputs().is_empty());
    }

    #[tokio::test]
    async fn service_errors_and_relative_traversal_are_model_errors() {
        let editor = Arc::new(TestEditor::new(FileEditResult::Err {
            error: "old_string not found".into(),
        }));
        let (tool, editor) = tool(editor, "/workspace/project", "/home/test");
        let ToolExecution::Runnable(execution) = tool
            .resolve_execution(input("src/a.txt", "old", "new"))
            .await
        else {
            panic!("expected runnable execution");
        };
        let result = execution.execute(context()).await;
        assert!(result.is_error);
        assert_eq!(text_output(&result), "old_string not found");

        let traversal = tool
            .resolve_execution(input("../outside.txt", "old", "new"))
            .await;
        let ToolExecution::Error(result) = traversal else {
            panic!("expected path resolution failure");
        };
        assert!(result.is_error);
        assert!(text_output(&result).contains("absolute path"));
        assert_eq!(editor.inputs().len(), 1);
    }
}
