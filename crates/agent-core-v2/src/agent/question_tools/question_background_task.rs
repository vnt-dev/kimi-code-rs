//! Detached task implementation for a background ask-user request.
//!
//! Original: `agent/questionTools/tools/question-background-task.ts`,
//! `QuestionBackgroundTask`.

use std::sync::Arc;

use async_trait::async_trait;
use futures_util::future::BoxFuture;
use serde_json::{Map, Value};

use crate::{
    _base::utils::abort::{AbortSignal, is_abort_error},
    agent::task::{
        AgentTask, AgentTaskError, AgentTaskInfo, AgentTaskInfoBase, AgentTaskSettlement,
        AgentTaskSettlementStatus, AgentTaskSink,
    },
    tool::{ExecutableToolOutput, ExecutableToolResult},
};

pub type QuestionTaskRun = Arc<
    dyn Fn(AbortSignal) -> BoxFuture<'static, Result<ExecutableToolResult, AgentTaskError>>
        + Send
        + Sync,
>;

pub struct QuestionBackgroundTask {
    run: QuestionTaskRun,
    description: String,
    question_count: u64,
    tool_call_id: Option<String>,
}

impl QuestionBackgroundTask {
    pub fn new(
        run: QuestionTaskRun,
        description: impl Into<String>,
        question_count: u64,
        tool_call_id: Option<String>,
    ) -> Self {
        Self {
            run,
            description: description.into(),
            question_count,
            tool_call_id,
        }
    }
}

#[async_trait]
impl AgentTask for QuestionBackgroundTask {
    fn id_prefix(&self) -> &str {
        "question"
    }

    fn kind(&self) -> &str {
        "question"
    }

    fn description(&self) -> &str {
        &self.description
    }

    // Original: QuestionBackgroundTask.start().
    async fn start(&self, sink: &dyn AgentTaskSink) -> Result<(), AgentTaskError> {
        match (self.run)(sink.signal()).await {
            Ok(result) => {
                sink.append_output(&tool_output_text(&result.output)?);
                sink.settle(AgentTaskSettlement {
                    status: AgentTaskSettlementStatus::Completed,
                    stop_reason: None,
                })
                .await?;
            }
            Err(error) if sink.signal().aborted() && is_abort_error(error.as_ref()) => {
                sink.settle(AgentTaskSettlement {
                    status: AgentTaskSettlementStatus::Killed,
                    stop_reason: None,
                })
                .await?;
            }
            Err(error) => {
                sink.settle(AgentTaskSettlement {
                    status: AgentTaskSettlementStatus::Failed,
                    stop_reason: Some(error.to_string()),
                })
                .await?;
            }
        }
        Ok(())
    }

    // Original: QuestionBackgroundTask.toInfo().
    fn to_info(&self, base: AgentTaskInfoBase) -> AgentTaskInfo {
        let mut details = Map::new();
        details.insert("questionCount".into(), Value::from(self.question_count));
        if let Some(tool_call_id) = &self.tool_call_id {
            details.insert("toolCallId".into(), Value::String(tool_call_id.clone()));
        }
        AgentTaskInfo {
            base,
            kind: self.kind().into(),
            details,
        }
    }
}

fn tool_output_text(output: &ExecutableToolOutput) -> Result<String, AgentTaskError> {
    match output {
        ExecutableToolOutput::Text(output) => Ok(output.clone()),
        ExecutableToolOutput::Content(content) => Ok(serde_json::to_string(content)?),
    }
}

#[cfg(test)]
mod tests {
    use parking_lot::Mutex;

    use futures_util::FutureExt;

    use super::*;
    use crate::{
        _base::utils::abort::{AbortController, AbortError},
        agent::task::{AgentTaskSettlement, AgentTaskStatus},
    };

    struct RecordingSink {
        signal: AbortSignal,
        output: Mutex<Vec<String>>,
        settlements: Mutex<Vec<AgentTaskSettlement>>,
    }

    #[async_trait]
    impl AgentTaskSink for RecordingSink {
        fn signal(&self) -> AbortSignal {
            self.signal.clone()
        }

        fn append_output(&self, chunk: &str) {
            self.output.lock().push(chunk.into());
        }

        async fn settle(&self, settlement: AgentTaskSettlement) -> Result<bool, AgentTaskError> {
            self.settlements.lock().push(settlement);
            Ok(true)
        }
    }

    fn base() -> AgentTaskInfoBase {
        AgentTaskInfoBase {
            task_id: "question-1".into(),
            description: "Pick a color".into(),
            status: AgentTaskStatus::Running,
            detached: Some(true),
            started_at: 1,
            ended_at: None,
            stop_reason: None,
            terminal_notification_suppressed: None,
            timeout_ms: None,
        }
    }

    #[tokio::test]
    async fn appends_result_then_completes_with_question_details() {
        let run: QuestionTaskRun = Arc::new(|_| {
            async {
                Ok(ExecutableToolResult::success(
                    "{\"answers\":{\"q_0\":\"Yes\"}}",
                ))
            }
            .boxed()
        });
        let task = QuestionBackgroundTask::new(run, "Pick a color", 2, Some("call-1".into()));
        let sink = RecordingSink {
            signal: AbortController::new().signal(),
            output: Mutex::new(Vec::new()),
            settlements: Mutex::new(Vec::new()),
        };

        task.start(&sink).await.unwrap();
        assert_eq!(*sink.output.lock(), vec!["{\"answers\":{\"q_0\":\"Yes\"}}"]);
        assert_eq!(
            sink.settlements.lock()[0].status,
            AgentTaskSettlementStatus::Completed
        );
        let info = task.to_info(base());
        assert_eq!(info.kind, "question");
        assert_eq!(info.details["questionCount"], 2);
        assert_eq!(info.details["toolCallId"], "call-1");
    }

    #[tokio::test]
    async fn aborted_run_settles_as_killed() {
        let controller = AbortController::new();
        controller.abort(Some(AbortError::new("cancelled")));
        let run: QuestionTaskRun = Arc::new(|_| {
            async {
                Err::<ExecutableToolResult, AgentTaskError>(Box::new(AbortError::new("cancelled")))
            }
            .boxed()
        });
        let task = QuestionBackgroundTask::new(run, "Pick a color", 1, None);
        let sink = RecordingSink {
            signal: controller.signal(),
            output: Mutex::new(Vec::new()),
            settlements: Mutex::new(Vec::new()),
        };

        task.start(&sink).await.unwrap();
        assert_eq!(
            sink.settlements.lock()[0].status,
            AgentTaskSettlementStatus::Killed
        );
    }
}
