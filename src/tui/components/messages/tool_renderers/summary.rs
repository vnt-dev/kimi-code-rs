use crate::tui::{
    components::Text,
    theme::current_theme,
    types::{ToolCallBlockData, ToolResultBlockData},
};

use super::{
    truncated::render_truncated,
    types::{RenderedComponents, RendererContext},
};

const GLANCE_SAMPLES: usize = 3;

type GlanceFn = fn(&ToolCallBlockData, &ToolResultBlockData) -> String;

fn with_glance(
    glance: Option<GlanceFn>,
    tool_call: &ToolCallBlockData,
    result: &ToolResultBlockData,
    context: RendererContext,
) -> RenderedComponents {
    if result.is_error.unwrap_or(false) {
        return render_truncated(tool_call, result, context);
    }
    let mut output: RenderedComponents = Vec::new();
    if let Some(glance) = glance {
        let line = glance(tool_call, result);
        if !line.is_empty() {
            output.push(Box::new(Text::new(
                format!("  {}", current_theme().dim(&line)),
                0,
                0,
            )));
        }
    }
    if context.expanded && !result.output.is_empty() {
        output.push(Box::new(Text::new(
            current_theme().dim(&result.output),
            4,
            0,
        )));
    }
    output
}

fn non_empty_lines(text: &str) -> Vec<&str> {
    if text.is_empty() {
        Vec::new()
    } else {
        text.split('\n').filter(|line| !line.is_empty()).collect()
    }
}

fn path_from_grep_line(line: &str) -> &str {
    let Some(first) = line.find(':') else {
        return line;
    };
    if first == 0 {
        return line;
    }
    let Some(second_offset) = line[first + 1..].find(':') else {
        return line;
    };
    &line[..first + 1 + second_offset]
}

fn glance_samples(lines: Vec<&str>) -> String {
    let samples = lines
        .iter()
        .take(GLANCE_SAMPLES)
        .copied()
        .collect::<Vec<_>>();
    let remaining = lines.len().saturating_sub(samples.len());
    let tail = if remaining > 0 {
        format!(", +{remaining} more")
    } else {
        String::new()
    };
    format!("{}{tail}", samples.join(", "))
}

fn grep_glance(_tool_call: &ToolCallBlockData, result: &ToolResultBlockData) -> String {
    let lines = non_empty_lines(&result.output);
    if lines.is_empty() {
        return String::new();
    }
    glance_samples(lines.into_iter().map(path_from_grep_line).collect())
}

fn glob_glance(_tool_call: &ToolCallBlockData, result: &ToolResultBlockData) -> String {
    let lines = non_empty_lines(&result.output);
    if lines.is_empty() {
        String::new()
    } else {
        glance_samples(lines)
    }
}

fn collapsed_summary(
    tool_call: &ToolCallBlockData,
    result: &ToolResultBlockData,
    context: RendererContext,
) -> RenderedComponents {
    with_glance(None, tool_call, result, context)
}

pub fn read_summary(
    tool_call: &ToolCallBlockData,
    result: &ToolResultBlockData,
    context: RendererContext,
) -> RenderedComponents {
    collapsed_summary(tool_call, result, context)
}

pub fn fetch_summary(
    tool_call: &ToolCallBlockData,
    result: &ToolResultBlockData,
    context: RendererContext,
) -> RenderedComponents {
    collapsed_summary(tool_call, result, context)
}

pub fn web_search_summary(
    tool_call: &ToolCallBlockData,
    result: &ToolResultBlockData,
    context: RendererContext,
) -> RenderedComponents {
    collapsed_summary(tool_call, result, context)
}

pub fn think_summary(
    tool_call: &ToolCallBlockData,
    result: &ToolResultBlockData,
    context: RendererContext,
) -> RenderedComponents {
    collapsed_summary(tool_call, result, context)
}

pub fn edit_summary(
    tool_call: &ToolCallBlockData,
    result: &ToolResultBlockData,
    context: RendererContext,
) -> RenderedComponents {
    collapsed_summary(tool_call, result, context)
}

pub fn write_summary(
    tool_call: &ToolCallBlockData,
    result: &ToolResultBlockData,
    context: RendererContext,
) -> RenderedComponents {
    collapsed_summary(tool_call, result, context)
}

pub fn grep_summary(
    tool_call: &ToolCallBlockData,
    result: &ToolResultBlockData,
    context: RendererContext,
) -> RenderedComponents {
    with_glance(Some(grep_glance), tool_call, result, context)
}

pub fn glob_summary(
    tool_call: &ToolCallBlockData,
    result: &ToolResultBlockData,
    context: RendererContext,
) -> RenderedComponents {
    with_glance(Some(glob_glance), tool_call, result, context)
}

#[cfg(test)]
mod tests {
    use serde_json::Map;

    use crate::tui::types::ToolCallBlockData;

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

    fn render(mut components: RenderedComponents) -> String {
        components
            .iter_mut()
            .flat_map(|component| component.render(100))
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
    fn collapsed_and_expanded_summaries_match_original_behavior() {
        let read = call("Read");
        let content = result("1\tfoo\n2\tbar", false);
        assert!(
            render(read_summary(&read, &content, RendererContext::default()))
                .trim()
                .is_empty()
        );
        let expanded = render(read_summary(
            &read,
            &content,
            RendererContext { expanded: true },
        ));
        assert!(expanded.contains("foo"));
        assert!(expanded.contains("bar"));

        for summary in [
            fetch_summary,
            web_search_summary,
            think_summary,
            edit_summary,
            write_summary,
        ] {
            assert!(
                render(summary(&read, &content, RendererContext::default()))
                    .trim()
                    .is_empty()
            );
        }
    }

    #[test]
    fn grep_and_glob_show_three_samples_and_remaining_count() {
        let grep = render(grep_summary(
            &call("Grep"),
            &result("src/a.ts\nsrc/b.ts\nsrc/c.ts\nsrc/d.ts\nsrc/e.ts", false),
            RendererContext::default(),
        ));
        assert!(grep.contains("src/a.ts, src/b.ts, src/c.ts, +2 more"));
        assert!(!grep.contains("src/d.ts"));

        let content_mode = render(grep_summary(
            &call("Grep"),
            &result("src/a.ts:42:    foo()\nsrc/b.ts:7:foo", false),
            RendererContext::default(),
        ));
        assert!(content_mode.contains("src/a.ts:42"));
        assert!(!content_mode.contains("foo()"));

        let glob = render(glob_summary(
            &call("Glob"),
            &result("a.ts\nb.ts\nc.ts\nd.ts", false),
            RendererContext::default(),
        ));
        assert!(glob.contains("a.ts, b.ts, c.ts, +1 more"));
    }

    #[test]
    fn empty_glances_render_nothing_and_errors_use_raw_output() {
        assert!(
            render(grep_summary(
                &call("Grep"),
                &result("", false),
                RendererContext::default(),
            ))
            .trim()
            .is_empty()
        );
        let error = render(read_summary(
            &call("Read"),
            &result("ENOENT: missing", true),
            RendererContext::default(),
        ));
        assert!(error.contains("ENOENT: missing"));
    }
}
