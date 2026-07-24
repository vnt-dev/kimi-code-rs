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

/// Inputs for [`run_single_execution`].
///
/// This keeps the method-level counterpart of the original
/// `runSingleExecution()` readable without an eight-argument Rust signature.
pub struct RunSingleExecutionInput<'a> {
    pub tool_name: &'a str,
    pub tool_call_id: String,
    pub execution: &'a RunnableToolExecution,
    pub turn_id: i64,
    pub trace: Option<LlmRequestTrace>,
    pub metadata: Option<Value>,
    pub signal: AbortSignal,
    pub on_update: Option<ToolUpdateCallback>,
}

pub async fn run_single_execution(input: RunSingleExecutionInput<'_>) -> ToolResult {
    let RunSingleExecutionInput {
        tool_name,
        tool_call_id,
        execution,
        turn_id,
        trace,
        metadata,
        signal,
        on_update,
    } = input;
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
    // Original: `normalizeAndMergeResult()` restores `coerced.delivery` after
    // normalization, whose output-focused portion intentionally omits it.
    let delivery = raw.delivery.clone();
    let mut normalized = normalize_tool_result(raw);
    normalized.delivery = delivery;
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
        kosong::contract::message::ContentPart,
        tool::{
            ExecutableToolOutput, ExecutableToolResult, ToolDelivery, ToolDeliveryKind,
            ToolDeliveryMessage,
        },
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
        let result = run_single_execution(RunSingleExecutionInput {
            tool_name: "Read",
            tool_call_id: "call-1".into(),
            execution: &execution,
            turn_id: 2,
            trace: None,
            metadata: None,
            signal: AbortController::new().signal(),
            on_update: None,
        })
        .await;
        assert_eq!(
            result.output,
            ExecutableToolOutput::Text("Tool output is empty.".into())
        );
        assert_eq!(result.description.as_deref(), Some("read a file"));
        assert_eq!(result.approval_rule.as_deref(), Some("always"));
        assert_eq!(result.stop_batch_after_this, Some(true));
    }

    #[tokio::test]
    async fn execution_preserves_delivery_after_normalization() {
        let delivery = ToolDelivery {
            kind: ToolDeliveryKind::Steer,
            message: ToolDeliveryMessage {
                content: vec![ContentPart::Text {
                    text: "follow up".into(),
                }],
                tool_calls: None,
                origin: None,
            },
        };
        let expected_delivery = delivery.clone();
        let execute = Arc::new(move |_context: ExecutableToolContext| {
            let delivery = delivery.clone();
            Box::pin(async move {
                ExecutableToolResult {
                    delivery: Some(delivery),
                    ..ExecutableToolResult::success("done")
                }
            }) as BoxFuture<'static, ExecutableToolResult>
        });
        let execution = RunnableToolExecution::new("always", execute);
        let result = run_single_execution(RunSingleExecutionInput {
            tool_name: "Read",
            tool_call_id: "call-1".into(),
            execution: &execution,
            turn_id: 2,
            trace: None,
            metadata: None,
            signal: AbortController::new().signal(),
            on_update: None,
        })
        .await;
        assert_eq!(result.delivery, Some(expected_delivery));
    }
}
