//! Persistent todo-item data and display helpers.
//!
//! Original: `session/todo/todoItem.ts`.

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const TODO_LIST_TOOL_NAME: &str = "TodoList";

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TodoStatus {
    Pending,
    InProgress,
    Done,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TodoItem {
    pub title: String,
    pub status: TodoStatus,
}

// Original: readTodoItems(). Invalid array members are ignored rather than
// rejecting the whole persisted `tools.update_store` record.
pub fn read_todo_items(raw: &Value) -> Vec<TodoItem> {
    raw.as_array()
        .map(|items| {
            items
                .iter()
                .filter(|item| is_todo_item(item))
                .filter_map(|item| serde_json::from_value(item.clone()).ok())
                .collect()
        })
        .unwrap_or_default()
}

// Original: isTodoItem().
pub fn is_todo_item(value: &Value) -> bool {
    serde_json::from_value::<TodoItem>(value.clone()).is_ok()
}

// Original: renderTodoList().
pub fn render_todo_list(todos: &[TodoItem], title: Option<&str>) -> String {
    if todos.is_empty() {
        return "Todo list is empty.".into();
    }
    let title = title.unwrap_or("Current todo list:");
    std::iter::once(title.to_owned())
        .chain(
            todos
                .iter()
                .map(|todo| format!("  {} {}", status_marker(todo.status), todo.title)),
        )
        .collect::<Vec<_>>()
        .join("\n")
}

fn status_marker(status: TodoStatus) -> &'static str {
    match status {
        TodoStatus::Pending => "[pending]",
        TodoStatus::InProgress => "[in_progress]",
        TodoStatus::Done => "[done]",
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn preserves_valid_items_and_ignores_invalid_persisted_members() {
        assert_eq!(
            read_todo_items(&json!([
                { "title": "first", "status": "pending" },
                { "title": "missing status" },
                { "title": 2, "status": "done" },
                { "title": "third", "status": "in_progress" },
            ])),
            vec![
                TodoItem {
                    title: "first".into(),
                    status: TodoStatus::Pending
                },
                TodoItem {
                    title: "third".into(),
                    status: TodoStatus::InProgress
                },
            ]
        );
        assert!(read_todo_items(&json!({})).is_empty());
    }

    #[test]
    fn renders_source_markers_and_empty_list_message() {
        let todos = vec![TodoItem {
            title: "Ship it".into(),
            status: TodoStatus::Done,
        }];
        assert_eq!(
            render_todo_list(&todos, None),
            "Current todo list:\n  [done] Ship it"
        );
        assert_eq!(
            render_todo_list(&[], Some("Ignored")),
            "Todo list is empty."
        );
    }
}
