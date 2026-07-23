use std::{collections::HashSet, path::PathBuf, sync::LazyLock};

use regex::Regex;
use serde::Deserialize;
use serde_json::{Map, Value};

use crate::persistence::interface::{
    atomic_document_store::AtomicDocumentStoreHandle,
    storage::{FileSystemStorageServiceHandle, StorageAppendOptions, StorageError},
};

use super::types::{AgentTaskInfo, AgentTaskInfoBase, AgentTaskStatus};

const TASKS_SCOPE: &str = "tasks";
const OUTPUT_LOG_KEY: &str = "output.log";
const JSON_SUFFIX: &str = ".json";

static VALID_TASK_ID: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^[a-z0-9]+(?:-[a-z0-9]+)*-[0-9a-z]{8}$")
        .expect("task id regex is static and valid")
});

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentTaskPersistenceRoot {
    pub dir: PathBuf,
    pub scope: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentTaskStoredOutputSnapshot {
    pub output_path: PathBuf,
    pub output_size_bytes: usize,
    pub preview_bytes: usize,
    pub truncated: bool,
    pub preview: String,
}

#[derive(Debug, thiserror::Error)]
pub enum TaskPersistenceError {
    #[error("Invalid task id: \"{0}\"")]
    InvalidTaskId(String),
    #[error(transparent)]
    Storage(#[from] StorageError),
}

struct ListedTask {
    key_id: String,
    task: AgentTaskInfo,
}

struct TaskOutputData {
    root: AgentTaskPersistenceRoot,
    data: Vec<u8>,
}

pub struct AgentTaskPersistence {
    agent_dir: PathBuf,
    agent_scope: String,
    docs: AtomicDocumentStoreHandle,
    bytes: FileSystemStorageServiceHandle,
    fallback_root: Option<AgentTaskPersistenceRoot>,
}

impl AgentTaskPersistence {
    // Original: task/persist.ts, AgentTaskPersistence.constructor().
    pub fn new(
        agent_dir: impl Into<PathBuf>,
        agent_scope: impl Into<String>,
        docs: AtomicDocumentStoreHandle,
        bytes: FileSystemStorageServiceHandle,
        fallback_root: Option<AgentTaskPersistenceRoot>,
    ) -> Self {
        Self {
            agent_dir: agent_dir.into(),
            agent_scope: agent_scope.into(),
            docs,
            bytes,
            fallback_root,
        }
    }

    fn primary_root(&self) -> AgentTaskPersistenceRoot {
        AgentTaskPersistenceRoot {
            dir: self.agent_dir.clone(),
            scope: self.agent_scope.clone(),
        }
    }

    fn tasks_scope(&self, root: Option<&AgentTaskPersistenceRoot>) -> String {
        let scope = root.map_or(self.agent_scope.as_str(), |root| root.scope.as_str());
        format!("{scope}/{TASKS_SCOPE}")
    }

    fn task_output_scope(
        &self,
        task_id: &str,
        root: Option<&AgentTaskPersistenceRoot>,
    ) -> Result<String, TaskPersistenceError> {
        validate_task_id(task_id)?;
        let scope = root.map_or(self.agent_scope.as_str(), |root| root.scope.as_str());
        Ok(format!("{scope}/{TASKS_SCOPE}/{task_id}"))
    }

    fn task_output_file_at(
        &self,
        task_id: &str,
        root: &AgentTaskPersistenceRoot,
    ) -> Result<PathBuf, TaskPersistenceError> {
        validate_task_id(task_id)?;
        Ok(root
            .dir
            .join(TASKS_SCOPE)
            .join(task_id)
            .join(OUTPUT_LOG_KEY))
    }

    // Original: AgentTaskPersistence.taskOutputFile().
    pub fn task_output_file(&self, task_id: &str) -> Result<PathBuf, TaskPersistenceError> {
        self.task_output_file_at(task_id, &self.primary_root())
    }

    // Original: AgentTaskPersistence.writeTask().
    pub async fn write_task(&self, task: &AgentTaskInfo) -> Result<(), TaskPersistenceError> {
        validate_task_id(&task.base.task_id)?;
        self.docs
            .set(
                &self.tasks_scope(None),
                &format!("{}{JSON_SUFFIX}", task.base.task_id),
                task,
            )
            .await?;
        Ok(())
    }

    // Original: AgentTaskPersistence.readTask(). A present but unrecognized
    // primary document is authoritative and therefore suppresses fallback.
    pub async fn read_task(
        &self,
        task_id: &str,
    ) -> Result<Option<AgentTaskInfo>, TaskPersistenceError> {
        validate_task_id(task_id)?;
        let key = format!("{task_id}{JSON_SUFFIX}");
        if let Some(value) = self.docs.0.get_value(&self.tasks_scope(None), &key).await? {
            return Ok(normalize_persisted_task(&value));
        }
        let Some(fallback) = &self.fallback_root else {
            return Ok(None);
        };
        Ok(self
            .docs
            .0
            .get_value(&self.tasks_scope(Some(fallback)), &key)
            .await?
            .as_ref()
            .and_then(normalize_persisted_task))
    }

    // Original: AgentTaskPersistence.appendTaskOutput().
    pub async fn append_task_output(
        &self,
        task_id: &str,
        chunk: &str,
    ) -> Result<(), TaskPersistenceError> {
        if chunk.is_empty() {
            return Ok(());
        }
        self.bytes
            .0
            .append(
                &self.task_output_scope(task_id, None)?,
                OUTPUT_LOG_KEY,
                chunk.as_bytes(),
                StorageAppendOptions::default(),
            )
            .await?;
        Ok(())
    }

    pub async fn task_output_size_bytes(
        &self,
        task_id: &str,
    ) -> Result<usize, TaskPersistenceError> {
        Ok(self
            .read_task_output_data(task_id)
            .await?
            .map_or(0, |output| output.data.len()))
    }

    pub async fn task_output_exists(&self, task_id: &str) -> Result<bool, TaskPersistenceError> {
        Ok(self.read_task_output_data(task_id).await?.is_some())
    }

    // Original: AgentTaskPersistence.readTaskOutputBytes(). Byte windows may
    // split UTF-8; TextDecoder's replacement behavior maps to from_utf8_lossy.
    pub async fn read_task_output_bytes(
        &self,
        task_id: &str,
        offset: f64,
        max_bytes: f64,
    ) -> Result<String, TaskPersistenceError> {
        let start = nonnegative_truncating_index(offset);
        let limit = nonnegative_truncating_index(max_bytes);
        if limit == 0 {
            return Ok(String::new());
        }
        let Some(output) = self.read_task_output_data(task_id).await? else {
            return Ok(String::new());
        };
        if start >= output.data.len() {
            return Ok(String::new());
        }
        let end = start.saturating_add(limit).min(output.data.len());
        Ok(String::from_utf8_lossy(&output.data[start..end]).into_owned())
    }

    pub async fn read_task_output_snapshot(
        &self,
        task_id: &str,
        max_preview_bytes: f64,
    ) -> Result<Option<AgentTaskStoredOutputSnapshot>, TaskPersistenceError> {
        let Some(output) = self.read_task_output_data(task_id).await? else {
            return Ok(None);
        };
        let preview_bytes = nonnegative_truncating_index(max_preview_bytes).min(output.data.len());
        let preview_offset = output.data.len() - preview_bytes;
        Ok(Some(AgentTaskStoredOutputSnapshot {
            output_path: self.task_output_file_at(task_id, &output.root)?,
            output_size_bytes: output.data.len(),
            preview_bytes,
            truncated: preview_offset > 0,
            preview: String::from_utf8_lossy(&output.data[preview_offset..]).into_owned(),
        }))
    }

    // Original: AgentTaskPersistence.listTasks(). Primary key ids reserve the
    // corresponding fallback id even when their documents are unreadable.
    pub async fn list_tasks(&self) -> Result<Vec<AgentTaskInfo>, TaskPersistenceError> {
        let primary = self.list_tasks_at(&self.primary_root()).await?;
        let mut tasks = primary.1;
        if let Some(fallback) = &self.fallback_root {
            let (_, fallback_tasks) = self.list_tasks_at(fallback).await?;
            tasks.extend(
                fallback_tasks
                    .into_iter()
                    .filter(|entry| !primary.0.contains(&entry.key_id)),
            );
        }
        let mut tasks = tasks
            .into_iter()
            .map(|entry| entry.task)
            .collect::<Vec<_>>();
        tasks.sort_by(|left, right| left.base.task_id.cmp(&right.base.task_id));
        Ok(tasks)
    }

    async fn list_tasks_at(
        &self,
        root: &AgentTaskPersistenceRoot,
    ) -> Result<(HashSet<String>, Vec<ListedTask>), TaskPersistenceError> {
        let scope = self.tasks_scope(Some(root));
        let mut keys = self.docs.list(&scope, None).await?;
        keys.sort();
        let mut reserved = HashSet::new();
        let mut tasks = Vec::new();
        for key in keys {
            let Some(id) = key.strip_suffix(JSON_SUFFIX) else {
                continue;
            };
            if !VALID_TASK_ID.is_match(id) {
                continue;
            }
            reserved.insert(id.to_owned());
            let value = match self.docs.0.get_value(&scope, &key).await {
                Ok(Some(value)) => value,
                Ok(None) | Err(_) => continue,
            };
            if let Some(task) = normalize_persisted_task(&value) {
                tasks.push(ListedTask {
                    key_id: id.into(),
                    task,
                });
            }
        }
        Ok((reserved, tasks))
    }

    async fn read_task_output_data(
        &self,
        task_id: &str,
    ) -> Result<Option<TaskOutputData>, TaskPersistenceError> {
        let primary = self.primary_root();
        if let Some(data) = self
            .bytes
            .0
            .read(
                &self.task_output_scope(task_id, Some(&primary))?,
                OUTPUT_LOG_KEY,
            )
            .await?
        {
            return Ok(Some(TaskOutputData {
                root: primary,
                data,
            }));
        }
        let Some(fallback) = &self.fallback_root else {
            return Ok(None);
        };
        Ok(self
            .bytes
            .0
            .read(
                &self.task_output_scope(task_id, Some(fallback))?,
                OUTPUT_LOG_KEY,
            )
            .await?
            .map(|data| TaskOutputData {
                root: fallback.clone(),
                data,
            }))
    }
}

fn validate_task_id(task_id: &str) -> Result<(), TaskPersistenceError> {
    if VALID_TASK_ID.is_match(task_id) {
        Ok(())
    } else {
        Err(TaskPersistenceError::InvalidTaskId(task_id.into()))
    }
}

fn nonnegative_truncating_index(value: f64) -> usize {
    if value.is_nan() || value <= 0.0 {
        0
    } else if value.is_infinite() || value >= usize::MAX as f64 {
        usize::MAX
    } else {
        value.trunc() as usize
    }
}

fn normalize_persisted_task(value: &Value) -> Option<AgentTaskInfo> {
    let object = value.as_object()?;
    if object.get("task_id").is_some_and(Value::is_string) {
        return serde_json::from_value::<LegacyPersistedTask>(value.clone())
            .ok()
            .map(legacy_persisted_task_to_info);
    }
    if !object.get("taskId").is_some_and(Value::is_string) {
        return None;
    }
    let mut task = serde_json::from_value::<AgentTaskInfo>(value.clone()).ok()?;
    if task.base.detached.is_none() {
        task.base.detached = Some(true);
    }
    Some(task)
}

#[derive(Deserialize)]
struct LegacyPersistedTask {
    task_id: String,
    command: String,
    description: String,
    pid: i64,
    started_at: i64,
    ended_at: Option<i64>,
    exit_code: Option<i64>,
    status: LegacyAgentTaskStatus,
    #[serde(default)]
    timed_out: bool,
    stop_reason: Option<String>,
    timeout_ms: Option<u64>,
    agent_id: Option<String>,
    subagent_type: Option<String>,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
enum LegacyAgentTaskStatus {
    Running,
    AwaitingApproval,
    Completed,
    Failed,
    Killed,
    Lost,
}

fn legacy_persisted_task_to_info(task: LegacyPersistedTask) -> AgentTaskInfo {
    let status = match (task.status, task.timed_out) {
        (LegacyAgentTaskStatus::AwaitingApproval, _) => AgentTaskStatus::Running,
        (LegacyAgentTaskStatus::Failed, true) => AgentTaskStatus::TimedOut,
        (LegacyAgentTaskStatus::Running, _) => AgentTaskStatus::Running,
        (LegacyAgentTaskStatus::Completed, _) => AgentTaskStatus::Completed,
        (LegacyAgentTaskStatus::Failed, _) => AgentTaskStatus::Failed,
        (LegacyAgentTaskStatus::Killed, _) => AgentTaskStatus::Killed,
        (LegacyAgentTaskStatus::Lost, _) => AgentTaskStatus::Lost,
    };
    let base = AgentTaskInfoBase {
        task_id: task.task_id.clone(),
        description: task.description,
        status,
        detached: Some(true),
        started_at: task.started_at,
        ended_at: task.ended_at,
        stop_reason: optional_nonempty_string(task.stop_reason),
        terminal_notification_suppressed: None,
        timeout_ms: task.timeout_ms,
    };
    let (kind, details) = if task.task_id.starts_with("agent-") {
        let mut details = Map::new();
        if let Some(agent_id) = optional_nonempty_string(task.agent_id) {
            details.insert("agentId".into(), Value::String(agent_id));
        }
        if let Some(subagent_type) = optional_nonempty_string(task.subagent_type) {
            details.insert("subagentType".into(), Value::String(subagent_type));
        }
        ("agent".into(), details)
    } else {
        (
            "process".into(),
            Map::from_iter([
                ("command".into(), Value::String(task.command)),
                ("pid".into(), Value::from(task.pid)),
                (
                    "exitCode".into(),
                    task.exit_code.map_or(Value::Null, Value::from),
                ),
            ]),
        )
    };
    AgentTaskInfo {
        base,
        kind,
        details,
    }
}

fn optional_nonempty_string(value: Option<String>) -> Option<String> {
    value.and_then(|value| {
        let trimmed = value.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_owned())
    })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::persistence::{
        backends::{
            memory::in_memory_storage_service::InMemoryStorageService,
            node_fs::atomic_document_store::JsonAtomicDocumentStore,
        },
        interface::{
            atomic_document_store::{AtomicDocumentStoreHandle, AtomicDocumentStoreService},
            storage::{FileSystemStorageService, StorageWriteOptions},
        },
    };

    fn sample(task_id: &str, description: &str) -> AgentTaskInfo {
        AgentTaskInfo {
            base: AgentTaskInfoBase {
                task_id: task_id.into(),
                description: description.into(),
                status: AgentTaskStatus::Running,
                detached: Some(true),
                started_at: 1_700_000_000,
                ended_at: None,
                stop_reason: None,
                terminal_notification_suppressed: None,
                timeout_ms: None,
            },
            kind: "process".into(),
            details: Map::from_iter([
                ("command".into(), Value::String("echo ok".into())),
                ("pid".into(), Value::from(123)),
                ("exitCode".into(), Value::Null),
            ]),
        }
    }

    fn stores() -> (
        Arc<InMemoryStorageService>,
        AtomicDocumentStoreHandle,
        FileSystemStorageServiceHandle,
    ) {
        let storage = Arc::new(InMemoryStorageService::default());
        let fs: Arc<dyn FileSystemStorageService> = storage.clone();
        let docs: Arc<dyn AtomicDocumentStoreService> =
            Arc::new(JsonAtomicDocumentStore::new(fs.clone()));
        (
            storage,
            AtomicDocumentStoreHandle(docs),
            FileSystemStorageServiceHandle(fs),
        )
    }

    fn persistence(
        dir: &str,
        scope: &str,
        docs: &AtomicDocumentStoreHandle,
        bytes: &FileSystemStorageServiceHandle,
        fallback: Option<AgentTaskPersistenceRoot>,
    ) -> AgentTaskPersistence {
        AgentTaskPersistence::new(dir, scope, docs.clone(), bytes.clone(), fallback)
    }

    #[tokio::test]
    async fn writes_reads_overwrites_and_lists_tasks_in_id_order() {
        let (_, docs, bytes) = stores();
        let service = persistence("/home/session", "session", &docs, &bytes, None);
        let second = sample("bash-22222222", "second");
        let mut first = sample("bash-11111111", "first");
        service.write_task(&second).await.unwrap();
        service.write_task(&first).await.unwrap();
        first.base.status = AgentTaskStatus::Completed;
        first.base.ended_at = Some(1_700_000_100);
        first.details.insert("exitCode".into(), Value::from(0));
        service.write_task(&first).await.unwrap();

        assert_eq!(
            service.read_task("bash-11111111").await.unwrap(),
            Some(first)
        );
        assert_eq!(service.read_task("bash-missing0").await.unwrap(), None);
        assert_eq!(
            service
                .list_tasks()
                .await
                .unwrap()
                .iter()
                .map(|task| task.base.task_id.as_str())
                .collect::<Vec<_>>(),
            ["bash-11111111", "bash-22222222"]
        );
    }

    #[tokio::test]
    async fn rejects_every_path_operation_for_invalid_task_ids() {
        let (_, docs, bytes) = stores();
        let service = persistence("/home/session", "session", &docs, &bytes, None);
        let invalid = "../../etc/passwd";
        assert!(matches!(
            service.write_task(&sample(invalid, "bad")).await,
            Err(TaskPersistenceError::InvalidTaskId(_))
        ));
        assert!(service.read_task(invalid).await.is_err());
        assert!(service.task_output_file(invalid).is_err());
        assert!(service.append_task_output(invalid, "x").await.is_err());
    }

    #[tokio::test]
    async fn reads_exact_byte_windows_and_tail_snapshots_with_lossy_utf8() {
        let (_, docs, bytes) = stores();
        let service = persistence("/home/session", "session", &docs, &bytes, None);
        service
            .append_task_output("bash-page0000", "abcdefghijklmnopqrstuvwxyz")
            .await
            .unwrap();
        assert_eq!(
            service
                .task_output_size_bytes("bash-page0000")
                .await
                .unwrap(),
            26
        );
        assert!(service.task_output_exists("bash-page0000").await.unwrap());
        assert_eq!(
            service
                .read_task_output_bytes("bash-page0000", 5.9, 10.8)
                .await
                .unwrap(),
            "fghijklmno"
        );
        assert_eq!(
            service
                .read_task_output_bytes("bash-page0000", f64::NAN, 3.0)
                .await
                .unwrap(),
            "abc"
        );
        assert_eq!(
            service
                .read_task_output_snapshot("bash-page0000", 6.9)
                .await
                .unwrap(),
            Some(AgentTaskStoredOutputSnapshot {
                output_path: PathBuf::from("/home/session/tasks/bash-page0000/output.log"),
                output_size_bytes: 26,
                preview_bytes: 6,
                truncated: true,
                preview: "uvwxyz".into(),
            })
        );
        assert_eq!(
            service
                .read_task_output_bytes("bash-none0001", 0.0, 100.0)
                .await
                .unwrap(),
            ""
        );
    }

    #[tokio::test]
    async fn primary_documents_and_empty_output_are_authoritative_over_fallback() {
        let (storage, docs, bytes) = stores();
        let legacy = persistence("/home/session", "session", &docs, &bytes, None);
        let fallback = AgentTaskPersistenceRoot {
            dir: "/home/session".into(),
            scope: "session".into(),
        };
        let primary = persistence(
            "/home/session/agents/main",
            "session/agents/main",
            &docs,
            &bytes,
            Some(fallback),
        );
        let task_id = "bash-shared01";
        legacy.write_task(&sample(task_id, "legacy")).await.unwrap();
        legacy
            .append_task_output(task_id, "legacy output")
            .await
            .unwrap();

        docs.0
            .set_value(
                "session/agents/main/tasks",
                &format!("{task_id}.json"),
                serde_json::json!({"unexpected": true}),
            )
            .await
            .unwrap();
        storage
            .write(
                &format!("session/agents/main/tasks/{task_id}"),
                OUTPUT_LOG_KEY,
                b"",
                StorageWriteOptions::default(),
            )
            .await
            .unwrap();

        assert_eq!(primary.read_task(task_id).await.unwrap(), None);
        assert!(primary.list_tasks().await.unwrap().is_empty());
        assert_eq!(
            primary
                .read_task_output_snapshot(task_id, 100.0)
                .await
                .unwrap(),
            Some(AgentTaskStoredOutputSnapshot {
                output_path: PathBuf::from(
                    "/home/session/agents/main/tasks/bash-shared01/output.log"
                ),
                output_size_bytes: 0,
                preview_bytes: 0,
                truncated: false,
                preview: String::new(),
            })
        );
    }

    #[tokio::test]
    async fn fallback_reads_legacy_snake_case_process_and_agent_records() {
        let (_, docs, bytes) = stores();
        let fallback = AgentTaskPersistenceRoot {
            dir: "/home/session".into(),
            scope: "session".into(),
        };
        let primary = persistence(
            "/home/session/agents/main",
            "session/agents/main",
            &docs,
            &bytes,
            Some(fallback),
        );
        for (id, value) in [
            (
                "bash-legacy01",
                serde_json::json!({
                    "task_id": "bash-legacy01", "command": "echo old",
                    "description": "legacy process", "pid": 42,
                    "started_at": 10, "ended_at": 20, "exit_code": 1,
                    "status": "failed", "stop_reason": "  stopped  "
                }),
            ),
            (
                "agent-legacy02",
                serde_json::json!({
                    "task_id": "agent-legacy02", "command": "",
                    "description": "legacy agent", "pid": 0,
                    "started_at": 10, "ended_at": 20, "exit_code": null,
                    "status": "failed", "timed_out": true,
                    "agent_id": " agent-0 ", "subagent_type": " explore "
                }),
            ),
        ] {
            docs.0
                .set_value("session/tasks", &format!("{id}.json"), value)
                .await
                .unwrap();
        }

        let process = primary.read_task("bash-legacy01").await.unwrap().unwrap();
        assert_eq!(process.kind, "process");
        assert_eq!(process.base.stop_reason.as_deref(), Some("stopped"));
        assert_eq!(process.details["exitCode"], 1);
        let agent = primary.read_task("agent-legacy02").await.unwrap().unwrap();
        assert_eq!(agent.kind, "agent");
        assert_eq!(agent.base.status, AgentTaskStatus::TimedOut);
        assert_eq!(agent.details["agentId"], "agent-0");
        assert_eq!(agent.details["subagentType"], "explore");
    }
}
