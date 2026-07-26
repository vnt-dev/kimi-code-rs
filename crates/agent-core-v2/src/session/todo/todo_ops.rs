//! Todo wire model and replayable `tools.update_store` operation.
//!
//! Original: `session/todo/todoOps.ts`.

use std::sync::LazyLock;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::wire::{
    model::{ModelDef, ModelOptions, define_model},
    op::{DefineOpOptions, DefinedOp, Op},
};

use super::todo_item::{TodoItem, read_todo_items};

pub type TodoModelState = Vec<TodoItem>;

pub static TODO_MODEL: LazyLock<ModelDef<TodoModelState>> =
    LazyLock::new(|| define_model("todo", Vec::new, ModelOptions::default()));

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct TodoSetPayload {
    pub key: String,
    pub value: Value,
}

pub static TODO_SET: LazyLock<DefinedOp<TodoModelState, TodoSetPayload>> = LazyLock::new(|| {
    TODO_MODEL
        .define_op("tools.update_store", DefineOpOptions::new(apply_todo_set))
        .expect("tools.update_store must have one global definition")
});

// Original: todoSet.apply(). Records for every other v1 store key leave this
// model unchanged, while `todo` values are sanitized before becoming state.
fn apply_todo_set(state: TodoModelState, payload: &TodoSetPayload) -> TodoModelState {
    if payload.key == "todo" {
        read_todo_items(&payload.value)
    } else {
        state
    }
}

// Original: todoSet({ key: 'todo', value }).
pub fn todo_set(todos: &[TodoItem]) -> Result<Op, serde_json::Error> {
    TODO_SET.create(TodoSetPayload {
        key: "todo".into(),
        value: serde_json::to_value(todos)?,
    })
}

/// Forces the v1 store descriptor to exist before an agent wire is restored.
///
/// The TypeScript module registers it while loading; the Rust lazy static must
/// be realized explicitly by the session-scoped service.
pub fn ensure_todo_ops_registered() {
    LazyLock::force(&TODO_SET);
}

#[cfg(test)]
mod tests {
    use crate::wire::op::ErasedOpDescriptor;

    use super::*;
    use crate::session::todo::{TodoItem, TodoStatus};

    #[test]
    fn update_store_payload_and_reducer_preserve_v1_todo_semantics() {
        ensure_todo_ops_registered();
        let todos = vec![TodoItem {
            title: "check".into(),
            status: TodoStatus::Pending,
        }];
        let op = todo_set(&todos).unwrap();
        assert_eq!(TODO_MODEL.name(), "todo");
        assert_eq!(TODO_SET.op_type(), "tools.update_store");
        assert_eq!(
            op.payload_value,
            serde_json::json!({ "key": "todo", "value": todos })
        );
        assert_eq!(
            TODO_SET
                .descriptor()
                .apply(Box::new(TodoModelState::new()), op.payload())
                .unwrap()
                .downcast::<TodoModelState>()
                .unwrap()
                .as_ref(),
            &todos
        );
        let untouched = TODO_SET
            .descriptor()
            .apply(
                Box::new(todos.clone()),
                &TodoSetPayload {
                    key: "other".into(),
                    value: serde_json::json!(null),
                },
            )
            .unwrap()
            .downcast::<TodoModelState>()
            .unwrap();
        assert_eq!(*untouched, todos);
    }
}
