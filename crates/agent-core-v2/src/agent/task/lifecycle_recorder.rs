//! Wire and telemetry effects for agent task lifecycle transitions.
//!
//! Original: `packages/agent-core-v2/src/agent/task/taskService.ts`,
//! `recordTaskStarted()` and `recordTaskTerminated()`.

use serde_json::Value;

use crate::{
    app::telemetry::{TelemetryProperties, TelemetryServiceHandle},
    wire::{contract::WireServiceHandle, wire_service::WireServiceError},
};

use super::{AgentTaskInfo, agent_task_status_text, task_started, task_terminated};

#[derive(Clone)]
pub struct AgentTaskLifecycleRecorder {
    wire: WireServiceHandle,
    telemetry: TelemetryServiceHandle,
}

#[derive(Debug, thiserror::Error)]
pub enum AgentTaskLifecycleRecordError {
    #[error(transparent)]
    Serialize(#[from] serde_json::Error),
    #[error(transparent)]
    Wire(#[from] WireServiceError),
}

impl AgentTaskLifecycleRecorder {
    pub fn new(wire: WireServiceHandle, telemetry: TelemetryServiceHandle) -> Self {
        Self { wire, telemetry }
    }

    // Original: AgentTaskService.recordTaskStarted(). Wire dispatch precedes
    // telemetry, including when the task kind is projected as `process`.
    pub fn record_task_started(
        &self,
        info: &AgentTaskInfo,
    ) -> Result<(), AgentTaskLifecycleRecordError> {
        self.wire.dispatch([task_started(info.clone())?])?;
        let kind = if info.kind == "process" {
            "bash"
        } else {
            &info.kind
        };
        self.telemetry.track(
            "background_task_created",
            Some(&TelemetryProperties::from([
                (
                    "task_id".into(),
                    Some(Value::String(info.base.task_id.clone())),
                ),
                ("kind".into(), Some(Value::String(kind.into()))),
            ])),
        );
        Ok(())
    }

    // Original: AgentTaskService.recordTaskTerminated(). A missing endedAt is
    // emitted as explicit null rather than omitted from telemetry.
    pub fn record_task_terminated(
        &self,
        info: &AgentTaskInfo,
    ) -> Result<(), AgentTaskLifecycleRecordError> {
        self.wire.dispatch([task_terminated(info.clone())?])?;
        let duration = info.base.ended_at.map_or(Value::Null, |ended_at| {
            Value::from(ended_at - info.base.started_at)
        });
        self.telemetry.track(
            "background_task_completed",
            Some(&TelemetryProperties::from([
                (
                    "task_id".into(),
                    Some(Value::String(info.base.task_id.clone())),
                ),
                ("kind".into(), Some(Value::String(info.kind.clone()))),
                ("duration_ms".into(), Some(duration)),
                (
                    "status".into(),
                    Some(Value::String(
                        agent_task_status_text(info.base.status).into(),
                    )),
                ),
            ])),
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;
    use futures_util::stream;
    use serde_json::Map;

    use super::*;
    use crate::{
        _base::di::lifecycle::{DisposableHandle, disposable_none},
        agent::task::{AgentTaskInfoBase, AgentTaskStatus, TASK_MODEL},
        app::telemetry::{
            TelemetryAppender, TelemetryContextPatch, TelemetryServiceContract,
            TelemetryServiceHandle,
        },
        persistence::interface::append_log_store::{
            AppendLogError, AppendLogOptions, AppendLogStoreHandle, AppendLogStoreService,
            AppendLogValueStream,
        },
        wire::{
            contract::WireServiceHandle,
            wire_service::{DomainEventPublisher, WireBlobService, WireService},
        },
    };

    #[derive(Default)]
    struct MemoryLog;

    #[async_trait]
    impl AppendLogStoreService for MemoryLog {
        fn append_value(&self, _: &str, _: &str, _: Value, _: AppendLogOptions) {}

        fn read_values(&self, _: &str, _: &str) -> AppendLogValueStream {
            Box::pin(stream::empty())
        }

        async fn rewrite_values(
            &self,
            _: &str,
            _: &str,
            _: Vec<Value>,
        ) -> Result<(), AppendLogError> {
            Ok(())
        }

        async fn flush(&self) -> Result<(), AppendLogError> {
            Ok(())
        }

        async fn close(&self) -> Result<(), AppendLogError> {
            Ok(())
        }

        fn acquire(&self, _: &str, _: &str) -> DisposableHandle {
            disposable_none()
        }
    }

    struct IdentityBlobs;

    #[async_trait]
    impl WireBlobService for IdentityBlobs {
        async fn offload_parts(&self, parts: Vec<Value>) -> Result<Vec<Value>, String> {
            Ok(parts)
        }

        async fn load_parts(&self, parts: Vec<Value>) -> Result<Vec<Value>, String> {
            Ok(parts)
        }
    }

    struct NoopEvents;

    impl DomainEventPublisher for NoopEvents {
        fn publish(&self, _: Value) {}
    }

    #[derive(Default)]
    struct RecordingTelemetry(Mutex<Vec<(String, TelemetryProperties)>>);

    #[async_trait]
    impl TelemetryServiceContract for RecordingTelemetry {
        fn track(&self, event: &str, properties: Option<&TelemetryProperties>) {
            self.0
                .lock()
                .unwrap()
                .push((event.into(), properties.cloned().unwrap_or_default()));
        }

        fn with_context(&self, _: &TelemetryContextPatch) -> TelemetryServiceHandle {
            TelemetryServiceHandle(Arc::new(Self::default()))
        }

        fn set_context(&self, _: &TelemetryContextPatch) {}
        fn add_appender(&self, _: Arc<dyn TelemetryAppender>) -> DisposableHandle {
            disposable_none()
        }
        fn remove_appender(&self, _: &Arc<dyn TelemetryAppender>) {}
        fn set_appender(&self, _: Arc<dyn TelemetryAppender>) {}
        fn set_enabled(&self, _: bool) {}
        async fn flush(&self) {}
        async fn shutdown(&self) {}
    }

    fn info(status: AgentTaskStatus, ended_at: Option<i64>) -> AgentTaskInfo {
        AgentTaskInfo {
            base: AgentTaskInfoBase {
                task_id: "bash-12345678".into(),
                description: "command".into(),
                status,
                detached: Some(true),
                started_at: 10,
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
    fn records_wire_state_before_matching_created_and_completed_telemetry() {
        let wire = WireServiceHandle(Arc::new(WireService::new(
            "agent",
            AppendLogStoreHandle(Arc::new(MemoryLog)),
            Arc::new(IdentityBlobs),
            Arc::new(NoopEvents),
        )));
        let telemetry = Arc::new(RecordingTelemetry::default());
        let recorder = AgentTaskLifecycleRecorder::new(
            wire.clone(),
            TelemetryServiceHandle(telemetry.clone()),
        );

        let running = info(AgentTaskStatus::Running, None);
        recorder.record_task_started(&running).unwrap();
        assert_eq!(wire.get_model(&TASK_MODEL)["bash-12345678"], running);

        let completed = info(AgentTaskStatus::Completed, Some(25));
        recorder.record_task_terminated(&completed).unwrap();
        assert_eq!(wire.get_model(&TASK_MODEL)["bash-12345678"], completed);

        let events = telemetry.0.lock().unwrap();
        assert_eq!(events[0].0, "background_task_created");
        assert_eq!(events[0].1["kind"], Some(Value::String("bash".into())));
        assert_eq!(events[1].0, "background_task_completed");
        assert_eq!(events[1].1["kind"], Some(Value::String("process".into())));
        assert_eq!(events[1].1["duration_ms"], Some(Value::from(15)));
        assert_eq!(
            events[1].1["status"],
            Some(Value::String("completed".into()))
        );
    }

    #[test]
    fn terminated_telemetry_keeps_missing_end_time_as_explicit_null() {
        let wire = WireServiceHandle(Arc::new(WireService::new(
            "agent",
            AppendLogStoreHandle(Arc::new(MemoryLog)),
            Arc::new(IdentityBlobs),
            Arc::new(NoopEvents),
        )));
        let telemetry = Arc::new(RecordingTelemetry::default());
        let recorder =
            AgentTaskLifecycleRecorder::new(wire, TelemetryServiceHandle(telemetry.clone()));
        recorder
            .record_task_terminated(&info(AgentTaskStatus::Lost, None))
            .unwrap();
        assert_eq!(
            telemetry.0.lock().unwrap()[0].1["duration_ms"],
            Some(Value::Null)
        );
    }
}
