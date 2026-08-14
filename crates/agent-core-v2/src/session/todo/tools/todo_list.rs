//! Structured, session-shared todo-list tool.
//!
//! Original: `packages/agent-core-v2/src/session/todo/tools/todo-list.ts`.

use std::sync::{Arc, LazyLock};

use async_trait::async_trait;
use futures_util::future::BoxFuture;
use kimi_code_protocol::display::{TodoDisplayItem, ToolInputDisplay};
use serde::{Deserialize, Deserializer};
use serde_json::{Map, Value, json};

use crate::{
    _base::di::instantiation::ServicesAccessorExt,
    agent::tool_registry::{ToolContributionOptions, register_tool},
    kosong::contract::tool::Tool,
    tool::{
        ExecutableTool, ExecutableToolContext, ExecutableToolResult, RunnableToolExecution,
        ToolExecution, input_schema::to_input_json_schema,
    },
};

use super::super::{
    SESSION_TODO_SERVICE_ID, SessionTodoError, SessionTodoServiceHandle, TODO_LIST_TOOL_NAME,
    TodoItem, TodoStatus, render_todo_list,
};

const TODO_LIST_DESCRIPTION: &str = include_str!("todo-list.md");
const TODO_LIST_WRITE_REMINDER: &str = include_str!("todo-list-write-reminder.md");

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TodoListInput {
    pub todos: Option<Vec<TodoItem>>,
}

impl<'de> Deserialize<'de> for TodoListInput {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = Value::deserialize(deserializer)?;
        parse_todo_list_input(&value).map_err(serde::de::Error::custom)
    }
}

pub fn parse_todo_list_input(value: &Value) -> Result<TodoListInput, String> {
    let object = value
        .as_object()
        .ok_or_else(|| "TodoList input must be an object".to_owned())?;
    if object.keys().any(|key| key != "todos") {
        return Err("TodoList input may only contain todos".into());
    }
    let Some(raw_todos) = object.get("todos") else {
        return Ok(TodoListInput::default());
    };
    let raw_todos = raw_todos
        .as_array()
        .ok_or_else(|| "todos must be an array".to_owned())?;
    let todos = raw_todos
        .iter()
        .map(parse_todo_item)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(TodoListInput { todos: Some(todos) })
}

fn parse_todo_item(value: &Value) -> Result<TodoItem, String> {
    let object = value
        .as_object()
        .ok_or_else(|| "each todo must be an object".to_owned())?;
    if object.len() != 2 || !object.contains_key("title") || !object.contains_key("status") {
        return Err("each todo must contain only title and status".into());
    }
    let title = object
        .get("title")
        .and_then(Value::as_str)
        .filter(|title| !title.is_empty())
        .ok_or_else(|| "todo title must be a non-empty string".to_owned())?
        .to_owned();
    let status = match object.get("status").and_then(Value::as_str) {
        Some("pending") => TodoStatus::Pending,
        Some("in_progress") => TodoStatus::InProgress,
        Some("done") => TodoStatus::Done,
        _ => return Err("todo status must be pending, in_progress, or done".into()),
    };
    Ok(TodoItem { title, status })
}

pub static TODO_LIST_PARAMETERS: LazyLock<Map<String, Value>> = LazyLock::new(|| {
    to_input_json_schema(
        json!({
            "type": "object",
            "properties": {
                "todos": {
                    "type": "array",
                    "description": "The updated todo list. Omit to read the current todo list without making changes. Pass an empty array to clear the list.",
                    "items": {
                        "type": "object",
                        "properties": {
                            "title": {
                                "type": "string",
                                "minLength": 1,
                                "description": "Short, actionable title for the todo."
                            },
                            "status": {
                                "type": "string",
                                "enum": ["pending", "in_progress", "done"],
                                "description": "Current status of the todo."
                            }
                        },
                        "required": ["title", "status"]
                    }
                }
            }
        })
        .as_object()
        .cloned()
        .expect("TodoList schema is an object"),
    )
});

pub trait TodoListProvider: Send + Sync {
    fn get_todos(&self) -> Vec<TodoItem>;
    fn set_todos(&self, todos: &[TodoItem]) -> Result<(), SessionTodoError>;
}

impl TodoListProvider for SessionTodoServiceHandle {
    fn get_todos(&self) -> Vec<TodoItem> {
        (**self).get_todos()
    }

    fn set_todos(&self, todos: &[TodoItem]) -> Result<(), SessionTodoError> {
        (**self).set_todos(todos)
    }
}

pub struct TodoListTool {
    todo: Arc<dyn TodoListProvider>,
    definition: Tool,
}

impl TodoListTool {
    pub fn new(todo: Arc<dyn TodoListProvider>) -> Self {
        Self {
            todo,
            definition: Tool {
                name: TODO_LIST_TOOL_NAME.into(),
                description: TODO_LIST_DESCRIPTION.into(),
                parameters: TODO_LIST_PARAMETERS.clone(),
                deferred: None,
            },
        }
    }

    pub fn from_service(todo: SessionTodoServiceHandle) -> Self {
        Self::new(Arc::new(todo))
    }
}

#[async_trait]
impl ExecutableTool for TodoListTool {
    type Input = TodoListInput;

    fn tool(&self) -> &Tool {
        &self.definition
    }

    async fn resolve_execution(&self, args: TodoListInput) -> ToolExecution {
        let description = match args.todos.as_ref() {
            None => "Reading todo list",
            Some(todos) if todos.is_empty() => "Clearing todo list",
            Some(_) => "Updating todo list",
        };
        let displayed_todos = args.todos.clone().unwrap_or_else(|| self.todo.get_todos());
        let todo = Arc::clone(&self.todo);
        let todos = args.todos;
        let execute = Arc::new(move |_context: ExecutableToolContext| {
            let todo = Arc::clone(&todo);
            let todos = todos.clone();
            Box::pin(async move {
                let Some(todos) = todos else {
                    return ExecutableToolResult::success(render_todo_list(
                        &todo.get_todos(),
                        None,
                    ));
                };
                if let Err(error) = todo.set_todos(&todos) {
                    return ExecutableToolResult::error(error.to_string());
                }
                let stored = todo.get_todos();
                if stored.is_empty() {
                    ExecutableToolResult::success("Todo list cleared.")
                } else {
                    ExecutableToolResult::success(format!(
                        "Todo list updated.\n{}\n\n{}",
                        render_todo_list(&stored, None),
                        TODO_LIST_WRITE_REMINDER.trim()
                    ))
                }
            }) as BoxFuture<'static, ExecutableToolResult>
        });
        let mut execution = RunnableToolExecution::new(TODO_LIST_TOOL_NAME, execute);
        execution.description = Some(description.into());
        execution.display = Some(ToolInputDisplay::TodoList {
            items: displayed_todos
                .into_iter()
                .map(|todo| TodoDisplayItem {
                    title: todo.title,
                    status: match todo.status {
                        TodoStatus::Pending => "pending",
                        TodoStatus::InProgress => "in_progress",
                        TodoStatus::Done => "done",
                    }
                    .into(),
                })
                .collect(),
        });
        ToolExecution::Runnable(execution)
    }
}

// Original: todo-list.ts, registerTool(TodoListTool).
pub fn register_todo_list_tool() {
    register_tool(
        Arc::new(|accessor| {
            let todo = accessor.get(SESSION_TODO_SERVICE_ID)?;
            Ok(Arc::new(TodoListTool::from_service((*todo).clone())))
        }),
        ToolContributionOptions::default(),
    );
}

#[cfg(test)]
mod tests {
    use parking_lot::Mutex;

    use super::*;
    use crate::{
        _base::utils::abort::AbortController,
        tool::{ExecutableToolOutput, ToolExecution},
    };

    #[derive(Default)]
    struct StubTodo {
        todos: Mutex<Vec<TodoItem>>,
    }

    impl StubTodo {
        fn with_todos(todos: Vec<TodoItem>) -> Self {
            Self {
                todos: Mutex::new(todos),
            }
        }
    }

    impl TodoListProvider for StubTodo {
        fn get_todos(&self) -> Vec<TodoItem> {
            self.todos.lock().clone()
        }

        fn set_todos(&self, todos: &[TodoItem]) -> Result<(), SessionTodoError> {
            *self.todos.lock() = todos.to_vec();
            Ok(())
        }
    }

    fn item(title: &str, status: TodoStatus) -> TodoItem {
        TodoItem {
            title: title.into(),
            status,
        }
    }

    fn execution_context() -> ExecutableToolContext {
        ExecutableToolContext {
            turn_id: crate::agent::TurnId::new(1),
            tool_call_id: "call_1".into(),
            trace: None,
            metadata: None,
            signal: AbortController::new().signal(),
            on_update: None,
            on_foreground_task_start: None,
        }
    }

    async fn execute(
        tool: &TodoListTool,
        input: TodoListInput,
    ) -> (RunnableToolExecution, ExecutableToolResult) {
        let ToolExecution::Runnable(execution) = tool.resolve_execution(input).await else {
            panic!("TodoList must be runnable");
        };
        let result = execution.execute(execution_context()).await;
        (execution, result)
    }

    #[test]
    fn schema_and_parser_preserve_source_query_and_validation_semantics() {
        assert_eq!(TODO_LIST_TOOL_NAME, "TodoList");
        assert_eq!(
            parse_todo_list_input(&json!({})).unwrap(),
            TodoListInput { todos: None }
        );
        assert_eq!(
            parse_todo_list_input(&json!({
                "todos": [{"title": "x", "status": "in_progress"}]
            }))
            .unwrap(),
            TodoListInput {
                todos: Some(vec![item("x", TodoStatus::InProgress)])
            }
        );
        for invalid in [
            json!(null),
            json!({"todos": null}),
            json!({"todos": [{"title": "", "status": "pending"}]}),
            json!({"todos": [{"title": "x", "status": "wip"}]}),
            json!({"todos": [{"title": "x", "status": "pending", "extra": true}]}),
            json!({"extra": true}),
        ] {
            assert!(parse_todo_list_input(&invalid).is_err(), "{invalid}");
        }
        assert_eq!(TODO_LIST_PARAMETERS["additionalProperties"], false);
        assert_eq!(
            TODO_LIST_PARAMETERS["properties"]["todos"]["items"]["additionalProperties"],
            false
        );
    }

    #[test]
    fn definition_contains_current_description_and_schema() {
        let tool = TodoListTool::new(Arc::new(StubTodo::default()));
        assert_eq!(tool.tool().name, TODO_LIST_TOOL_NAME);
        assert_eq!(tool.tool().description, TODO_LIST_DESCRIPTION);
        assert!(tool.tool().description.contains("**Avoid churn:**"));
        assert!(tool.tool().description.contains("proactively and often"));
        assert!(tool.tool().description.contains("tests are failing"));
        assert_eq!(tool.tool().parameters, *TODO_LIST_PARAMETERS);
    }

    #[tokio::test]
    async fn query_renders_without_mutating_and_uses_read_description() {
        let todo = Arc::new(StubTodo::with_todos(vec![item(
            "existing",
            TodoStatus::InProgress,
        )]));
        let tool = TodoListTool::new(todo.clone());
        let (execution, result) = execute(&tool, TodoListInput::default()).await;
        assert_eq!(execution.description.as_deref(), Some("Reading todo list"));
        assert_eq!(execution.approval_rule, TODO_LIST_TOOL_NAME);
        assert_eq!(
            execution.display,
            Some(ToolInputDisplay::TodoList {
                items: vec![TodoDisplayItem {
                    title: "existing".into(),
                    status: "in_progress".into(),
                }],
            })
        );
        assert_eq!(
            result.output,
            ExecutableToolOutput::Text("Current todo list:\n  [in_progress] existing".into())
        );
        assert!(!result.is_error);
        assert_eq!(
            todo.get_todos(),
            vec![item("existing", TodoStatus::InProgress)]
        );
    }

    #[tokio::test]
    async fn write_replaces_and_renders_list_with_progress_reminder() {
        let todo = Arc::new(StubTodo::default());
        let tool = TodoListTool::new(todo.clone());
        let input = TodoListInput {
            todos: Some(vec![
                item("first", TodoStatus::Pending),
                item("second", TodoStatus::InProgress),
            ]),
        };
        let (execution, result) = execute(&tool, input).await;
        assert_eq!(execution.description.as_deref(), Some("Updating todo list"));
        assert_eq!(
            result.output,
            ExecutableToolOutput::Text(
                "Todo list updated.\nCurrent todo list:\n  [pending] first\n  [in_progress] second\n\nEnsure that you continue to use the todo list to track progress. Mark tasks done immediately after finishing them, and keep exactly one task in_progress when work is underway.".into()
            )
        );
        assert_eq!(
            todo.get_todos(),
            vec![
                item("first", TodoStatus::Pending),
                item("second", TodoStatus::InProgress)
            ]
        );
    }

    #[tokio::test]
    async fn clear_empties_list_without_progress_reminder() {
        let todo = Arc::new(StubTodo::with_todos(vec![item("old", TodoStatus::Pending)]));
        let tool = TodoListTool::new(todo.clone());
        let (execution, result) = execute(
            &tool,
            TodoListInput {
                todos: Some(Vec::new()),
            },
        )
        .await;
        assert_eq!(execution.description.as_deref(), Some("Clearing todo list"));
        assert_eq!(
            result.output,
            ExecutableToolOutput::Text("Todo list cleared.".into())
        );
        assert!(todo.get_todos().is_empty());
    }

    #[tokio::test]
    async fn done_status_renders_its_persisted_enum_value() {
        let tool = TodoListTool::new(Arc::new(StubTodo::with_todos(vec![item(
            "shipped",
            TodoStatus::Done,
        )])));
        let (_, result) = execute(&tool, TodoListInput::default()).await;
        let ExecutableToolOutput::Text(output) = result.output else {
            panic!("TodoList output must be text");
        };
        assert!(output.contains("[done] shipped"));
        assert!(!output.contains("[completed]"));
    }
}
