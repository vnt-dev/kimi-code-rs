//! Session-shared todo service contract.
//!
//! Original: `packages/agent-core-v2/src/session/todo/sessionTodo.ts`.

use std::{error::Error, ops::Deref, sync::Arc};

use crate::_base::{
    di::{
        instantiation::ServiceIdentifier,
        lifecycle::{Disposable, DisposeResult},
    },
    event::Event,
};

use super::TodoItem;

pub type SessionTodoError = Box<dyn Error + Send + Sync>;

pub trait SessionTodoServiceContract: Disposable + Send + Sync {
    fn get_todos(&self) -> Vec<TodoItem>;
    fn set_todos(&self, todos: &[TodoItem]) -> Result<(), SessionTodoError>;
    fn clear(&self) -> Result<(), SessionTodoError> {
        self.set_todos(&[])
    }
    fn on_did_change(&self) -> Event<Vec<TodoItem>>;
}

#[derive(Clone)]
pub struct SessionTodoServiceHandle(pub Arc<dyn SessionTodoServiceContract>);

impl Deref for SessionTodoServiceHandle {
    type Target = dyn SessionTodoServiceContract;

    fn deref(&self) -> &Self::Target {
        self.0.as_ref()
    }
}

impl Disposable for SessionTodoServiceHandle {
    fn dispose(&self) -> DisposeResult {
        self.0.dispose()
    }
}

pub const SESSION_TODO_SERVICE_ID: ServiceIdentifier<SessionTodoServiceHandle> =
    ServiceIdentifier::new("sessionTodoService");
