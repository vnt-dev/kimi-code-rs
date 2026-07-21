use super::{
    goal::goal_summary,
    media::read_media_summary,
    summary::{
        edit_summary, fetch_summary, glob_summary, grep_summary, read_summary, think_summary,
        web_search_summary, write_summary,
    },
    truncated::render_truncated,
    types::ResultRenderer,
};
use crate::tui::components::messages::shell_execution::shell_execution_result_renderer;

/// True when the tool has no dedicated renderer and uses truncated output.
// Original: tool-renderers/registry.ts isGenericToolResult()
pub fn is_generic_tool_result(tool_name: &str) -> bool {
    !matches!(
        tool_name,
        "Read"
            | "ReadMediaFile"
            | "Grep"
            | "Glob"
            | "FetchURL"
            | "WebSearch"
            | "Bash"
            | "Think"
            | "Edit"
            | "Write"
            | "CreateGoal"
            | "GetGoal"
            | "SetGoalBudget"
            | "UpdateGoal"
    )
}

// Original: tool-renderers/registry.ts pickResultRenderer()
pub fn pick_result_renderer(tool_name: &str) -> ResultRenderer {
    match tool_name {
        "Read" => read_summary,
        "ReadMediaFile" => read_media_summary,
        "Grep" => grep_summary,
        "Glob" => glob_summary,
        "FetchURL" => fetch_summary,
        "WebSearch" => web_search_summary,
        "Bash" => shell_execution_result_renderer,
        "Think" => think_summary,
        "Edit" => edit_summary,
        "Write" => write_summary,
        "CreateGoal" | "GetGoal" | "SetGoalBudget" | "UpdateGoal" => goal_summary,
        _ => render_truncated,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::Map;

    use crate::tui::types::{ToolCallBlockData, ToolResultBlockData};

    use super::super::types::{RenderedComponents, RendererContext};
    use super::*;

    fn call(name: &str) -> ToolCallBlockData {
        ToolCallBlockData {
            id: "tc".to_owned(),
            name: name.to_owned(),
            args: Map::new(),
            description: None,
            streaming_arguments: None,
            streaming_started_at_ms: None,
            step: None,
            turn_id: None,
            truncated: None,
        }
    }

    fn result(output: &str, is_error: bool) -> ToolResultBlockData {
        ToolResultBlockData {
            tool_call_id: "tc".to_owned(),
            output: output.to_owned(),
            is_error: Some(is_error),
            synthetic: None,
        }
    }

    fn render(mut components: RenderedComponents, width: usize) -> String {
        components
            .iter_mut()
            .flat_map(|component| component.render(width))
            .map(|line| strip_sgr(&line))
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn strip_sgr(text: &str) -> String {
        let mut output = String::new();
        let mut escape = false;
        for character in text.chars() {
            if character == '\u{1b}' {
                escape = true;
            } else if escape && character == 'm' {
                escape = false;
            } else if !escape {
                output.push(character);
            }
        }
        output
    }

    #[test]
    fn unknown_tools_fall_back_to_wrap_aware_truncation() {
        let tool_call = call("SomethingUnknown");
        let fallback_result = result("a\nb\nc\nd\ne", false);
        let output = render(
            pick_result_renderer(&tool_call.name)(
                &tool_call,
                &fallback_result,
                RendererContext::default(),
            ),
            100,
        );
        assert!(output.contains('a'));
        assert!(output.contains('c'));
        assert!(!output.contains("\nd"));
        assert!(output.contains("... (2 more lines, ctrl+o to expand)"));

        let long = result(&"x".repeat(500), false);
        let output = render(
            pick_result_renderer(&tool_call.name)(&tool_call, &long, RendererContext::default()),
            20,
        );
        assert!(output.contains("... ("));
        assert!(!output.contains(&"x".repeat(500)));
    }

    #[test]
    fn bash_preserves_raw_output_and_read_expands_only_on_request() {
        let bash = call("Bash");
        let bash_result = result("one\ntwo\nthree\nfour", false);
        let output = render(
            pick_result_renderer(&bash.name)(&bash, &bash_result, RendererContext::default()),
            100,
        );
        assert!(output.contains("one"));
        assert!(output.contains("... (1 more lines, ctrl+o to expand)"));

        let read = call("Read");
        let read_result = result("1\tfoo\n2\tbar", false);
        assert!(
            render(
                pick_result_renderer(&read.name)(&read, &read_result, RendererContext::default(),),
                100,
            )
            .trim()
            .is_empty()
        );
        let expanded = render(
            pick_result_renderer(&read.name)(
                &read,
                &read_result,
                RendererContext { expanded: true },
            ),
            100,
        );
        assert!(expanded.contains("foo"));
        assert!(expanded.contains("bar"));
    }

    #[test]
    fn errors_always_remain_visible() {
        let read = call("Read");
        let error = result("ENOENT: foo.ts not found", true);
        let output = render(
            pick_result_renderer(&read.name)(&read, &error, RendererContext::default()),
            100,
        );
        assert!(output.contains("ENOENT: foo.ts not found"));
    }

    #[test]
    fn identifies_only_fallback_tools_as_generic() {
        assert!(is_generic_tool_result("SomethingUnknown"));
        assert!(is_generic_tool_result("mcp__server__do"));
        for name in ["Bash", "Read", "Grep", "Edit", "ReadMediaFile", "GetGoal"] {
            assert!(!is_generic_tool_result(name));
        }
    }
}
