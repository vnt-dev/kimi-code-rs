//! Agent task wrapper for a subagent completion.
//!
//! Original: `session/subagent/tools/subagent-task.ts`, `SubagentTask`.

use std::sync::Arc;

use async_trait::async_trait;
use futures_util::future::{BoxFuture, Shared};
use serde_json::{Map, Value};

use crate::{
    _base::utils::abort::{AbortController, is_abort_error, link_abort_signal},
    agent::task::{
        AgentTask, AgentTaskError, AgentTaskInfo, AgentTaskInfoBase, AgentTaskSettlement,
        AgentTaskSettlementStatus, AgentTaskSink,
    },
    kosong::contract::usage::TokenUsage,
};

pub type SubagentCompletionFuture = Shared<
    BoxFuture<'static, Result<SubagentCompletion, Arc<dyn std::error::Error + Send + Sync>>>,
>;

#[derive(Clone, Debug, PartialEq)]
pub struct SubagentCompletion {
    pub result: String,
    pub usage: Option<TokenUsage>,
}

#[derive(Clone)]
pub struct SubagentHandle {
    pub agent_id: String,
    pub profile_name: String,
    pub completion: SubagentCompletionFuture,
}

pub struct SubagentTask {
    handle: SubagentHandle,
    description: String,
    abort_controller: AbortController,
}

impl SubagentTask {
    pub fn new(
        handle: SubagentHandle,
        description: String,
        abort_controller: AbortController,
    ) -> Self {
        Self {
            handle,
            description,
            abort_controller,
        }
    }
}

#[async_trait]
impl AgentTask for SubagentTask {
    fn id_prefix(&self) -> &str {
        "agent"
    }
    fn kind(&self) -> &str {
        "agent"
    }
    fn description(&self) -> &str {
        &self.description
    }
    async fn start(&self, sink: &dyn AgentTaskSink) -> Result<(), AgentTaskError> {
        let signal = sink.signal();
        let mut link = link_abort_signal(&signal, self.abort_controller.clone());
        match self.handle.completion.clone().await {
            Ok(outcome) => {
                sink.append_output(&outcome.result);
                sink.settle(AgentTaskSettlement {
                    status: AgentTaskSettlementStatus::Completed,
                    stop_reason: None,
                })
                .await?;
            }
            Err(error)
                if signal.aborted()
                    && (is_abort_error(error.as_ref())
                        || signal.reason().is_some_and(|reason| {
                            std::ptr::addr_eq(Arc::as_ptr(&reason), Arc::as_ptr(&error))
                        })) =>
            {
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
        link.unlink();
        Ok(())
    }
    fn to_info(&self, base: AgentTaskInfoBase) -> AgentTaskInfo {
        AgentTaskInfo {
            base,
            kind: "agent".into(),
            details: Map::from_iter([
                (
                    "agentId".into(),
                    Value::String(self.handle.agent_id.clone()),
                ),
                (
                    "subagentType".into(),
                    Value::String(self.handle.profile_name.clone()),
                ),
            ]),
        }
    }
}
