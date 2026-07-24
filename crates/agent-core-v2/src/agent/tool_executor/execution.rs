//! Single runnable tool execution.
//!
//! Original: `toolExecutorService.ts`, `runSingleExecution()`.

use serde_json::Value;

use crate::{
    _base::utils::abort::AbortSignal,
    kosong::contract::request_trace::LlmRequestTrace,
    tool::{ExecutableToolContext, RunnableToolExecution, ToolResult, ToolUpdateCallback},
};

use super::{aborted_tool_output, normalize_tool_result, race_with_abort_grace};

pub async fn run_single_execution(
    tool_name: &str,
    tool_call_id: String,
    execution: &RunnableToolExecution,
    turn_id: i64,
    trace: Option<LlmRequestTrace>,
    metadata: Option<Value>,
    signal: AbortSignal,
    on_update: Option<ToolUpdateCallback>,
) -> ToolResult {
    if signal.aborted() {
        return ToolResult::from(crate::tool::ExecutableToolResult::error(
            aborted_tool_output(tool_name, &signal),
        ));
    }
    let Ok(turn_id) = u64::try_from(turn_id) else {
        return ToolResult::from(crate::tool::ExecutableToolResult::error(format!(
            "Tool \"{tool_name}\" failed: invalid negative turn ID"
        )));
    };
    let signal_for_fallback = signal.clone();
    let name_for_fallback = tool_name.to_owned();
    let raw = race_with_abort_grace(
        execution.execute(ExecutableToolContext {
            turn_id,
            tool_call_id,
            trace,
            metadata,
            signal: signal.clone(),
            on_update,
            on_foreground_task_start: None,
        }),
        &signal,
        move || {
            crate::tool::ExecutableToolResult::error(aborted_tool_output(
                &name_for_fallback,
                &signal_for_fallback,
            ))
        },
    )
    .await;
    let mut normalized = normalize_tool_result(raw);
    normalized.description = execution.description.clone();
    normalized.display = execution.display.clone();
    normalized.approval_rule = Some(execution.approval_rule.clone());
    normalized.stop_batch_after_this = normalized
        .stop_batch_after_this
        .or(execution.stop_batch_after_this);
    normalized
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use futures_util::future::BoxFuture;

    use super::*;
    use crate::{
        _base::utils::abort::AbortController,
        tool::{ExecutableToolOutput, ExecutableToolResult},
    };

    #[tokio::test]
    async fn execution_normalizes_output_and_merges_execution_fields() {
        let execute = Arc::new(|_context: ExecutableToolContext| {
            Box::pin(async { ExecutableToolResult::success("") })
                as BoxFuture<'static, ExecutableToolResult>
        });
        let mut execution = RunnableToolExecution::new("always", execute);
        execution.description = Some("read a file".into());
        execution.stop_batch_after_this = Some(true);
        let result = run_single_execution(
            "Read",
            "call-1".into(),
            &execution,
            2,
            None,
            None,
            AbortController::new().signal(),
            None,
        )
        .await;
        assert_eq!(
            result.output,
            ExecutableToolOutput::Text("Tool output is empty.".into())
        );
        assert_eq!(result.description.as_deref(), Some("read a file"));
        assert_eq!(result.approval_rule.as_deref(), Some("always"));
        assert_eq!(result.stop_batch_after_this, Some(true));
    }
}
