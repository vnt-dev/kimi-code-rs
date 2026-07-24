//! Replayable in-memory cron task model and transient operations.
//!
//! Original: `session/cron/cronOps.ts`.

use std::sync::LazyLock;

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

use crate::{
    app::cron::CronTask,
    wire::{
        model::{ModelDef, ModelOptions, define_model},
        op::{DefineOpOptions, DefinedOp, Op},
    },
};

pub type CronModelState = IndexMap<String, CronTask>;

pub static CRON_MODEL: LazyLock<ModelDef<CronModelState>> =
    LazyLock::new(|| define_model("cron", IndexMap::new, ModelOptions::default()));

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct CronAddPayload {
    pub task: CronTask,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CronDeletePayload {
    pub ids: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CronCursorPayload {
    pub id: String,
    pub last_fired_at: f64,
}

pub static CRON_ADD: LazyLock<DefinedOp<CronModelState, CronAddPayload>> = LazyLock::new(|| {
    let mut options = DefineOpOptions::new(apply_cron_add);
    options.persist = Some(false);
    CRON_MODEL
        .define_op("cron.add", options)
        .expect("cron.add must have one global definition")
});

pub static CRON_DELETE: LazyLock<DefinedOp<CronModelState, CronDeletePayload>> =
    LazyLock::new(|| {
        let mut options = DefineOpOptions::new(apply_cron_delete);
        options.persist = Some(false);
        CRON_MODEL
            .define_op("cron.delete", options)
            .expect("cron.delete must have one global definition")
    });

pub static CRON_CURSOR: LazyLock<DefinedOp<CronModelState, CronCursorPayload>> =
    LazyLock::new(|| {
        let mut options = DefineOpOptions::new(apply_cron_cursor);
        options.persist = Some(false);
        CRON_MODEL
            .define_op("cron.cursor", options)
            .expect("cron.cursor must have one global definition")
    });

// Original: cronAdd.apply(). Replacing an equal task still creates a fresh
// map in the source, so this reducer always clones before inserting.
fn apply_cron_add(mut state: CronModelState, payload: &CronAddPayload) -> CronModelState {
    state.insert(payload.task.id.clone(), payload.task.clone());
    state
}

// Original: cronDelete.apply(). An absent-only delete is a state no-op.
fn apply_cron_delete(mut state: CronModelState, payload: &CronDeletePayload) -> CronModelState {
    for id in &payload.ids {
        state.shift_remove(id);
    }
    state
}

// Original: cronCursor.apply(). Unknown tasks are left unchanged; a matching
// task keeps every field except its cursor timestamp.
fn apply_cron_cursor(mut state: CronModelState, payload: &CronCursorPayload) -> CronModelState {
    if let Some(task) = state.get_mut(&payload.id) {
        task.last_fired_at = Some(payload.last_fired_at);
    }
    state
}

pub fn cron_add(task: CronTask) -> Result<Op, serde_json::Error> {
    CRON_ADD.create(CronAddPayload { task })
}

pub fn cron_delete(ids: Vec<String>) -> Result<Op, serde_json::Error> {
    CRON_DELETE.create(CronDeletePayload { ids })
}

pub fn cron_cursor(id: impl Into<String>, last_fired_at: f64) -> Result<Op, serde_json::Error> {
    CRON_CURSOR.create(CronCursorPayload {
        id: id.into(),
        last_fired_at,
    })
}

#[cfg(test)]
mod tests {
    use crate::wire::op::ErasedOpDescriptor;

    use super::*;

    fn task(id: &str) -> CronTask {
        CronTask {
            id: id.into(),
            cron: "* * * * *".into(),
            prompt: "Check status".into(),
            created_at: 10.0,
            recurring: Some(true),
            last_fired_at: None,
            tags: None,
        }
    }

    fn applied<P>(
        op: &DefinedOp<CronModelState, P>,
        state: CronModelState,
        payload: &P,
    ) -> CronModelState
    where
        P: serde::Serialize + Send + Sync + 'static,
    {
        *op.descriptor()
            .apply(Box::new(state), payload)
            .unwrap()
            .downcast::<CronModelState>()
            .unwrap()
    }

    #[test]
    fn transient_ops_preserve_payloads_and_model_names() {
        let task = task("task-1");
        assert_eq!(CRON_MODEL.name(), "cron");
        assert_eq!(CRON_ADD.descriptor().persist_value(), Some(false));
        assert_eq!(CRON_DELETE.descriptor().persist_value(), Some(false));
        assert_eq!(CRON_CURSOR.descriptor().persist_value(), Some(false));
        assert_eq!(
            cron_add(task.clone()).unwrap().payload_value,
            serde_json::json!({"task": task})
        );
        assert_eq!(
            cron_delete(vec!["task-1".into()]).unwrap().payload_value,
            serde_json::json!({"ids": ["task-1"]})
        );
        assert_eq!(
            cron_cursor("task-1", 15.5).unwrap().payload_value,
            serde_json::json!({"id": "task-1", "lastFiredAt": 15.5})
        );
    }

    #[test]
    fn add_delete_and_cursor_update_only_their_source_fields() {
        let first = task("first");
        let second = task("second");
        let state = applied(
            &CRON_ADD,
            CronModelState::new(),
            &CronAddPayload { task: first },
        );
        let state = applied(
            &CRON_ADD,
            state,
            &CronAddPayload {
                task: second.clone(),
            },
        );
        let state = applied(
            &CRON_CURSOR,
            state,
            &CronCursorPayload {
                id: "second".into(),
                last_fired_at: 50.25,
            },
        );
        assert_eq!(state["second"].last_fired_at, Some(50.25));
        let state = applied(
            &CRON_DELETE,
            state,
            &CronDeletePayload {
                ids: vec!["first".into(), "missing".into()],
            },
        );
        assert_eq!(
            state.keys().map(String::as_str).collect::<Vec<_>>(),
            vec!["second"]
        );
        let untouched = applied(
            &CRON_CURSOR,
            state.clone(),
            &CronCursorPayload {
                id: "missing".into(),
                last_fired_at: 99.0,
            },
        );
        assert_eq!(untouched, state);
    }
}
