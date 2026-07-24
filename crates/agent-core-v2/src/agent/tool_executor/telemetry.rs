//! Tool execution telemetry classification.
//!
//! Original: `toolExecutorService.ts`, `toolTelemetryOutcome()` and
//! `toolTelemetryErrorType()`.

use crate::{
    kosong::contract::message::ContentPart,
    tool::{ExecutableToolOutput, ToolResult},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ToolTelemetryOutcome {
    Success,
    Error,
    Cancelled,
}

pub fn tool_telemetry_outcome(result: &ToolResult) -> ToolTelemetryOutcome {
    if !result.is_error {
        ToolTelemetryOutcome::Success
    } else {
        let text = tool_output_text(&result.output).to_lowercase();
        if text.contains("aborted")
            || text.contains("cancelled")
            || text.contains("manually interrupted")
        {
            ToolTelemetryOutcome::Cancelled
        } else {
            ToolTelemetryOutcome::Error
        }
    }
}

pub fn tool_telemetry_error_type(outcome: ToolTelemetryOutcome) -> &'static str {
    match outcome {
        ToolTelemetryOutcome::Cancelled => "cancelled",
        _ => "error",
    }
}

fn tool_output_text(output: &ExecutableToolOutput) -> String {
    match output {
        ExecutableToolOutput::Text(text) => text.clone(),
        ExecutableToolOutput::Content(parts) => parts
            .iter()
            .filter_map(|part| match part {
                ContentPart::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool::ExecutableToolResult;

    #[test]
    fn telemetry_classifies_success_error_and_cancellation_text() {
        assert_eq!(
            tool_telemetry_outcome(&ToolResult::from(ExecutableToolResult::success("ok"))),
            ToolTelemetryOutcome::Success
        );
        assert_eq!(
            tool_telemetry_outcome(&ToolResult::from(ExecutableToolResult::error("bad input"))),
            ToolTelemetryOutcome::Error
        );
        let cancelled = tool_telemetry_outcome(&ToolResult::from(ExecutableToolResult::error(
            "Tool was aborted",
        )));
        assert_eq!(cancelled, ToolTelemetryOutcome::Cancelled);
        assert_eq!(tool_telemetry_error_type(cancelled), "cancelled");
    }
}
