//! Replayable task lifecycle model and transient task operations.
//!
//! Original: `packages/agent-core-v2/src/agent/task/taskOps.ts`.

use std::sync::{Arc, LazyLock};

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

use crate::wire::{
    model::{ModelDef, ModelOptions, define_model},
    op::{DefineOpOptions, DefinedOp, Op},
};

use super::types::AgentTaskInfo;

pub type TaskModelState = IndexMap<String, AgentTaskInfo>;

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct TaskInfoPayload {
    pub info: AgentTaskInfo,
}

pub static TASK_MODEL: LazyLock<ModelDef<TaskModelState>> =
    LazyLock::new(|| define_model("task", TaskModelState::new, ModelOptions::default()));

pub static TASK_STARTED: LazyLock<DefinedOp<TaskModelState, TaskInfoPayload>> =
    LazyLock::new(|| define_task_op("task.started"));

pub static TASK_TERMINATED: LazyLock<DefinedOp<TaskModelState, TaskInfoPayload>> =
    LazyLock::new(|| define_task_op("task.terminated"));

fn define_task_op(op_type: &'static str) -> DefinedOp<TaskModelState, TaskInfoPayload> {
    let event_type = op_type;
    let mut options = DefineOpOptions::new(apply_task_info);
    options.persist = Some(false);
    options.to_event = Some(Arc::new(move |payload, _state| {
        Some(serde_json::json!({
            "type": event_type,
            "info": payload.info,
        }))
    }));
    TASK_MODEL
        .define_op(op_type, options)
        .expect("task lifecycle op must have one global definition")
}

// Original: taskOps.ts, taskStarted.apply() and taskTerminated.apply(). Both
// operations copy the model Map and replace the entry selected by info.taskId.
fn apply_task_info(mut state: TaskModelState, payload: &TaskInfoPayload) -> TaskModelState {
    state.insert(payload.info.base.task_id.clone(), payload.info.clone());
    state
}

pub fn task_started(info: AgentTaskInfo) -> Result<Op, serde_json::Error> {
    TASK_STARTED.create(TaskInfoPayload { info })
}

pub fn task_terminated(info: AgentTaskInfo) -> Result<Op, serde_json::Error> {
    TASK_TERMINATED.create(TaskInfoPayload { info })
}

#[cfg(test)]
mod tests {
    use serde_json::{Map, Value};

    use super::*;
    use crate::{
        agent::task::types::{AgentTaskInfoBase, AgentTaskStatus},
        wire::op::ErasedOpDescriptor,
    };

    fn info(task_id: &str, status: AgentTaskStatus) -> AgentTaskInfo {
        AgentTaskInfo {
            base: AgentTaskInfoBase {
                task_id: task_id.into(),
                description: "command".into(),
                status,
                detached: Some(true),
                started_at: 10,
                ended_at: None,
                stop_reason: None,
                terminal_notification_suppressed: None,
                timeout_ms: None,
            },
            kind: "process".into(),
            details: Map::from_iter([
                ("command".into(), Value::String("pwd".into())),
                ("pid".into(), Value::from(42)),
            ]),
        }
    }

    #[test]
    fn model_and_transient_wire_payloads_match_source() {
        assert_eq!(TASK_MODEL.name(), "task");
        assert!(TASK_MODEL.initial().is_empty());
        assert_eq!(TASK_STARTED.op_type(), "task.started");
        assert_eq!(TASK_TERMINATED.op_type(), "task.terminated");
        assert_eq!(TASK_STARTED.descriptor().persist(), Some(false));
        assert_eq!(TASK_TERMINATED.descriptor().persist(), Some(false));

        let op = task_started(info("bash-12345678", AgentTaskStatus::Running)).unwrap();
        assert_eq!(op.payload_value["info"]["taskId"], "bash-12345678");
        assert_eq!(op.payload_value["info"]["status"], "running");
    }

    #[test]
    fn both_reducers_replace_the_same_task_id_and_keep_other_entries() {
        let first = info("bash-11111111", AgentTaskStatus::Running);
        let other = info("bash-22222222", AgentTaskStatus::Running);
        let mut state = IndexMap::from([
            (first.base.task_id.clone(), first),
            (other.base.task_id.clone(), other.clone()),
        ]);
        let terminated = info("bash-11111111", AgentTaskStatus::Completed);
        state = apply_task_info(
            state,
            &TaskInfoPayload {
                info: terminated.clone(),
            },
        );

        assert_eq!(state.len(), 2);
        assert_eq!(state["bash-11111111"], terminated);
        assert_eq!(state["bash-22222222"], other);
    }

    #[test]
    fn lifecycle_ops_project_their_corresponding_events() {
        for (op, descriptor, event_type) in [
            (
                task_started(info("bash-11111111", AgentTaskStatus::Running)).unwrap(),
                &*TASK_STARTED,
                "task.started",
            ),
            (
                task_terminated(info("bash-22222222", AgentTaskStatus::Failed)).unwrap(),
                &*TASK_TERMINATED,
                "task.terminated",
            ),
        ] {
            let event = descriptor
                .descriptor()
                .to_event(op.payload(), &TaskModelState::new())
                .unwrap()
                .unwrap();
            assert_eq!(event["type"], event_type);
            assert_eq!(event["info"], op.payload_value["info"]);
        }
    }
}
