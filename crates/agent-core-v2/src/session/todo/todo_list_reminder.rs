//! Pure stale-todo reminder calculation.
//!
//! Original: `session/todo/todoListReminder.ts`.

use crate::{
    agent::context_memory::{ContextMessage, PromptOrigin},
    kosong::contract::message::Role,
};

use super::todo_item::{TODO_LIST_TOOL_NAME, TodoItem};

pub const TODO_LIST_REMINDER_VARIANT: &str = "todo_list_reminder";
const TODO_LIST_REMINDER_TURNS_SINCE_WRITE: u64 = 10;
const TODO_LIST_REMINDER_TURNS_BETWEEN_REMINDERS: u64 = 10;

pub struct TodoListReminderInput<'a> {
    pub active: bool,
    pub history: &'a [ContextMessage],
    pub todos: &'a [TodoItem],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct TodoListReminderTurnCounts {
    turns_since_last_write: u64,
    turns_since_last_reminder: u64,
}

// Original: todoListStaleReminder().
pub fn todo_list_stale_reminder(input: TodoListReminderInput<'_>) -> Option<String> {
    if !input.active {
        return None;
    }
    let counts = todo_list_reminder_turn_counts(input.history);
    if counts.turns_since_last_write < TODO_LIST_REMINDER_TURNS_SINCE_WRITE
        || counts.turns_since_last_reminder < TODO_LIST_REMINDER_TURNS_BETWEEN_REMINDERS
    {
        return None;
    }
    Some(render_todo_list_reminder(input.todos))
}

// Original: getTodoListReminderTurnCounts(). Only assistant messages count as
// turns; injections are searched independently while walking backwards.
fn todo_list_reminder_turn_counts(history: &[ContextMessage]) -> TodoListReminderTurnCounts {
    let mut found_write = false;
    let mut found_reminder = false;
    let mut turns_since_last_write = 0;
    let mut turns_since_last_reminder = 0;
    for message in history.iter().rev() {
        if message.message.role == Role::Assistant {
            if !found_write && has_todo_list_write(message) {
                found_write = true;
            }
            if !found_write {
                turns_since_last_write += 1;
            }
            if !found_reminder {
                turns_since_last_reminder += 1;
            }
            continue;
        }
        if !found_reminder && is_todo_list_reminder(message) {
            found_reminder = true;
        }
        if found_write && found_reminder {
            break;
        }
    }
    TodoListReminderTurnCounts {
        turns_since_last_write,
        turns_since_last_reminder,
    }
}

fn has_todo_list_write(message: &ContextMessage) -> bool {
    message.message.tool_calls.iter().any(|tool_call| {
        tool_call.name == TODO_LIST_TOOL_NAME
            && tool_call.arguments.as_deref().is_some_and(|arguments| {
                serde_json::from_str::<serde_json::Value>(arguments)
                    .ok()
                    .and_then(|value| value.get("todos").cloned())
                    .is_some_and(|todos| todos.is_array())
            })
    })
}

fn is_todo_list_reminder(message: &ContextMessage) -> bool {
    matches!(
        message.origin,
        Some(PromptOrigin::Injection { ref variant }) if variant == TODO_LIST_REMINDER_VARIANT
    )
}

fn render_todo_list_reminder(todos: &[TodoItem]) -> String {
    let mut message = "The TodoList tool has not been updated recently. If you are working on tasks that benefit from progress tracking, consider using TodoList to update task status. Also consider clearing or rewriting the todo list if it has become stale and no longer matches the current work. Only use it if relevant. This is a gentle reminder; ignore it if not applicable. Make sure that you NEVER mention this reminder to the user.".to_owned();
    let items = todos
        .iter()
        .enumerate()
        .map(|(index, todo)| {
            format!(
                "{}. [{}] {}",
                index + 1,
                todo_status_name(todo.status),
                todo.title
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    if !items.is_empty() {
        message.push_str("\n\nCurrent todo list:\n");
        message.push_str(&items);
    }
    message
}

fn todo_status_name(status: super::todo_item::TodoStatus) -> &'static str {
    match status {
        super::todo_item::TodoStatus::Pending => "pending",
        super::todo_item::TodoStatus::InProgress => "in_progress",
        super::todo_item::TodoStatus::Done => "done",
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        agent::context_memory::{ContextMessage, PromptOrigin},
        kosong::contract::message::{Message, Role, ToolCall, ToolCallType},
        session::todo::{TodoItem, TodoStatus},
    };

    use super::*;

    fn message(
        role: Role,
        tool_calls: Vec<ToolCall>,
        origin: Option<PromptOrigin>,
    ) -> ContextMessage {
        ContextMessage {
            message: Message::new(role, vec![], tool_calls),
            id: None,
            provider_message_id: None,
            origin,
            is_error: None,
            note: None,
        }
    }

    #[test]
    fn waits_ten_assistant_turns_after_todo_write_and_last_reminder() {
        let write = ToolCall {
            call_type: ToolCallType::Function,
            id: "todo".into(),
            name: TODO_LIST_TOOL_NAME.into(),
            arguments: Some("{\"todos\":[]}".into()),
            extras: None,
            stream_index: None,
        };
        let mut history = vec![message(Role::Assistant, vec![write], None)];
        history.extend((0..10).map(|_| message(Role::Assistant, vec![], None)));
        assert!(
            todo_list_stale_reminder(TodoListReminderInput {
                active: true,
                history: &history,
                todos: &[]
            })
            .is_some()
        );
        history.push(message(
            Role::User,
            vec![],
            Some(PromptOrigin::Injection {
                variant: TODO_LIST_REMINDER_VARIANT.into(),
            }),
        ));
        assert!(
            todo_list_stale_reminder(TodoListReminderInput {
                active: true,
                history: &history,
                todos: &[]
            })
            .is_none()
        );
    }

    #[test]
    fn inactive_tool_never_injects_and_rendering_keeps_persisted_status_names() {
        let todos = [TodoItem {
            title: "work".into(),
            status: TodoStatus::InProgress,
        }];
        assert!(
            todo_list_stale_reminder(TodoListReminderInput {
                active: false,
                history: &[],
                todos: &todos
            })
            .is_none()
        );
        let history = (0..10)
            .map(|_| message(Role::Assistant, vec![], None))
            .collect::<Vec<_>>();
        let reminder = todo_list_stale_reminder(TodoListReminderInput {
            active: true,
            history: &history,
            todos: &todos,
        })
        .unwrap();
        assert!(reminder.ends_with("Current todo list:\n1. [in_progress] work"));
    }
}
