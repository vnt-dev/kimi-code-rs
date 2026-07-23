//! Restored task records that have no live task object in the current process.
//!
//! Original: `packages/agent-core-v2/src/agent/task/taskService.ts`,
//! `restoreGhostsFromWire()`, `loadFromDisk()`, and the ghost portion of
//! `markLoadedTasksLost()`.

use std::collections::HashSet;

use indexmap::IndexMap;

use super::{AgentTaskInfo, mark_loaded_task_lost, newer_restored_task};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct RestoredTaskRegistry {
    ghosts: IndexMap<String, AgentTaskInfo>,
}

impl RestoredTaskRegistry {
    pub fn get(&self, task_id: &str) -> Option<&AgentTaskInfo> {
        self.ghosts.get(task_id)
    }

    pub fn values(&self) -> impl Iterator<Item = &AgentTaskInfo> {
        self.ghosts.values()
    }

    // Original: taskService.ts, restoreGhostsFromWire(). Existing live task
    // ids are authoritative; other wire entries replace matching ghosts while
    // retaining JavaScript Map insertion order.
    pub fn restore_from_wire(
        &mut self,
        wire_tasks: &IndexMap<String, AgentTaskInfo>,
        live_task_ids: &HashSet<String>,
    ) {
        for (task_id, info) in wire_tasks {
            if live_task_ids.contains(task_id) {
                continue;
            }
            self.ghosts.insert(task_id.clone(), info.clone());
        }
    }

    // Original: taskService.ts, loadFromDisk() after listTasks(). `replace`
    // defaults to true; wire and disk conflicts use newerRestoredTask().
    pub fn merge_loaded(
        &mut self,
        loaded_tasks: impl IntoIterator<Item = AgentTaskInfo>,
        replace: Option<bool>,
        live_task_ids: &HashSet<String>,
    ) {
        if replace != Some(false) {
            self.ghosts.clear();
        }
        for loaded in loaded_tasks {
            let task_id = loaded.base.task_id.clone();
            if live_task_ids.contains(&task_id) {
                continue;
            }
            if let Some(existing) = self.ghosts.get_mut(&task_id) {
                *existing = newer_restored_task(existing.clone(), loaded);
            } else {
                self.ghosts.insert(task_id, loaded);
            }
        }
    }

    // Original: taskService.ts, markLoadedTasksLost() state updates. The
    // caller persists the returned records sequentially before emitting their
    // terminal effects.
    pub fn mark_active_lost(&mut self, now_ms: i64) -> Vec<AgentTaskInfo> {
        let mut lost = Vec::new();
        for info in self.ghosts.values_mut() {
            let Some(updated) = mark_loaded_task_lost(info.clone(), now_ms) else {
                continue;
            };
            *info = updated.clone();
            lost.push(updated);
        }
        lost
    }
}

#[cfg(test)]
mod tests {
    use serde_json::Map;

    use super::*;
    use crate::agent::task::{AgentTaskInfoBase, AgentTaskStatus};

    fn task(task_id: &str, status: AgentTaskStatus, ended_at: Option<i64>) -> AgentTaskInfo {
        AgentTaskInfo {
            base: AgentTaskInfoBase {
                task_id: task_id.into(),
                description: task_id.into(),
                status,
                detached: Some(true),
                started_at: 1,
                ended_at,
                stop_reason: None,
                terminal_notification_suppressed: None,
                timeout_ms: None,
            },
            kind: "process".into(),
            details: Map::new(),
        }
    }

    #[test]
    fn wire_restore_skips_live_ids_and_replaces_without_reordering() {
        let mut registry = RestoredTaskRegistry::default();
        registry.merge_loaded(
            [
                task("task-a", AgentTaskStatus::Running, None),
                task("task-b", AgentTaskStatus::Running, None),
            ],
            None,
            &HashSet::new(),
        );
        let wire = IndexMap::from([
            (
                "task-b".into(),
                task("task-b", AgentTaskStatus::Failed, Some(4)),
            ),
            (
                "task-c".into(),
                task("task-c", AgentTaskStatus::Completed, Some(5)),
            ),
        ]);
        registry.restore_from_wire(&wire, &HashSet::from(["task-c".into()]));
        assert_eq!(
            registry
                .values()
                .map(|info| (info.base.task_id.as_str(), info.base.status))
                .collect::<Vec<_>>(),
            [
                ("task-a", AgentTaskStatus::Running),
                ("task-b", AgentTaskStatus::Failed),
            ]
        );
    }

    #[test]
    fn disk_merge_defaults_to_replace_and_preserves_conflict_rules() {
        let mut registry = RestoredTaskRegistry::default();
        registry.merge_loaded(
            [task("wire", AgentTaskStatus::Failed, Some(9))],
            None,
            &HashSet::new(),
        );
        registry.merge_loaded(
            [task("disk", AgentTaskStatus::Running, None)],
            None,
            &HashSet::new(),
        );
        assert!(registry.get("wire").is_none());
        assert!(registry.get("disk").is_some());

        registry.merge_loaded(
            [
                task("disk", AgentTaskStatus::Completed, Some(10)),
                task("live", AgentTaskStatus::Failed, Some(11)),
            ],
            Some(false),
            &HashSet::from(["live".into()]),
        );
        assert_eq!(
            registry.get("disk").unwrap().base.status,
            AgentTaskStatus::Completed
        );
        assert!(registry.get("live").is_none());
    }

    #[test]
    fn active_ghosts_become_lost_in_map_order() {
        let mut registry = RestoredTaskRegistry::default();
        registry.merge_loaded(
            [
                task("running-a", AgentTaskStatus::Running, None),
                task("done", AgentTaskStatus::Completed, Some(2)),
                task("running-b", AgentTaskStatus::Running, Some(3)),
            ],
            None,
            &HashSet::new(),
        );
        let lost = registry.mark_active_lost(20);
        assert_eq!(
            lost.iter()
                .map(|info| (info.base.task_id.as_str(), info.base.ended_at))
                .collect::<Vec<_>>(),
            [("running-a", Some(20)), ("running-b", Some(3))]
        );
        assert_eq!(
            registry.get("running-a").unwrap().base.status,
            AgentTaskStatus::Lost
        );
        assert_eq!(
            registry.get("done").unwrap().base.status,
            AgentTaskStatus::Completed
        );
    }
}
