//! Managed concurrent execution primitive contracts.
//!
//! Original: `packages/agent-core-v2/src/app/task/task.ts`.

use std::{error::Error, fmt, sync::Arc};

use async_trait::async_trait;

use crate::_base::{
    di::{instantiation::ServiceIdentifier, lifecycle::Disposable},
    event::Event,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TaskState {
    Pending,
    Running,
    Completed,
    Failed,
    Cancelled,
}

impl TaskState {
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Cancelled)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TaskCancelledError {
    pub task_id: String,
}

impl fmt::Display for TaskCancelledError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "Task {} was cancelled", self.task_id)
    }
}

impl Error for TaskCancelledError {}

pub type TaskFailure = Arc<dyn Error + Send + Sync>;
pub type TaskResult<T> = Result<Arc<T>, TaskFailure>;

#[async_trait]
pub trait TaskHandle<T>: Disposable + Send + Sync {
    fn id(&self) -> &str;
    fn state(&self) -> TaskState;
    async fn result(&self) -> TaskResult<T>;
    fn on_did_change_state(&self) -> Event<TaskState>;
    fn on_did_output(&self) -> Event<String>;
    fn cancel(&self);
}

pub trait DeferredHandle<T>: TaskHandle<T> {
    fn resolve(&self, value: T);
    fn reject(&self, reason: TaskFailure);
}

pub trait TaskServiceContract: Send + Sync {}

pub const TASK_SERVICE_ID: ServiceIdentifier<dyn TaskServiceContract> =
    ServiceIdentifier::new("taskService");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_and_error_contract_match_source() {
        assert!(!TaskState::Running.is_terminal());
        assert!(TaskState::Cancelled.is_terminal());
        assert_eq!(
            TaskCancelledError {
                task_id: "task-2".into()
            }
            .to_string(),
            "Task task-2 was cancelled"
        );
        assert_eq!(TASK_SERVICE_ID.to_string(), "taskService");
    }
}
