use std::any::Any;

use crate::tui::{
    components::{Component, ComponentRole, Container, Text},
    theme::{ColorToken, current_theme},
    types::{ToolCallBlockData, ToolResultBlockData},
};

use super::tool_renderers::truncated::{
    PREVIEW_LINES, TruncatedOutputComponent, TruncatedOutputOptions,
};

#[derive(Debug, Clone, Default)]
pub struct ShellExecutionOptions {
    pub command: Option<String>,
    pub result: Option<ToolResultBlockData>,
    pub expanded: bool,
    pub show_command: bool,
    pub command_preview_lines: Option<usize>,
    pub result_preview_lines: Option<usize>,
    pub tail_output: bool,
    pub expand_hint: Option<bool>,
}

pub struct ShellExecutionComponent {
    children: Container,
}

impl ShellExecutionComponent {
    pub fn new(options: ShellExecutionOptions) -> Self {
        let mut component = Self {
            children: Container::new(),
        };
        if options.show_command {
            component.add_command_preview(
                options.command.as_deref().unwrap_or_default(),
                options.command_preview_lines,
            );
        }
        if let Some(result) = options.result {
            component.add_result_preview(
                &result,
                options.expanded,
                options.result_preview_lines.unwrap_or(PREVIEW_LINES),
                options.tail_output,
                options.expand_hint.unwrap_or(true),
            );
        }
        component
    }

    /// Original: shell-execution.ts ShellExecutionComponent.addCommandPreview()
    fn add_command_preview(&mut self, command: &str, preview_lines: Option<usize>) {
        if command.is_empty() {
            return;
        }
        let line_limit = preview_lines.unwrap_or(usize::MAX);
        for (index, line) in command.split('\n').take(line_limit).enumerate() {
            let text = if index == 0 {
                format!(
                    "{}{}",
                    current_theme().fg(ColorToken::ShellMode, "$ "),
                    current_theme().dim(line)
                )
            } else {
                format!("  {}", current_theme().dim(line))
            };
            self.children.add_child(Text::new(text, 2, 0));
        }
    }

    /// Original: shell-execution.ts ShellExecutionComponent.addResultPreview()
    fn add_result_preview(
        &mut self,
        result: &ToolResultBlockData,
        expanded: bool,
        preview_lines: usize,
        tail_output: bool,
        expand_hint: bool,
    ) {
        if result.output.is_empty() {
            return;
        }
        self.children.add_child(TruncatedOutputComponent::new(
            &result.output,
            TruncatedOutputOptions {
                expanded,
                is_error: result.is_error.unwrap_or(false),
                max_lines: Some(preview_lines),
                expand_hint: Some(expand_hint),
                tail: Some(tail_output),
                color: Some(ColorToken::TextMuted),
                ..TruncatedOutputOptions::default()
            },
        ));
    }
}

impl Component for ShellExecutionComponent {
    fn render(&mut self, width: usize) -> Vec<String> {
        self.children.render(width)
    }

    fn invalidate(&mut self) {
        self.children.invalidate();
    }

    fn role(&self) -> ComponentRole {
        ComponentRole::Other
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Original: shell-execution.ts shellExecutionResultRenderer()
pub fn shell_execution_result_renderer(
    _tool_call: &ToolCallBlockData,
    result: ToolResultBlockData,
    expanded: bool,
) -> Vec<Box<dyn Component>> {
    vec![Box::new(ShellExecutionComponent::new(
        ShellExecutionOptions {
            result: Some(result),
            expanded,
            ..ShellExecutionOptions::default()
        },
    ))]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn result(output: &str) -> ToolResultBlockData {
        ToolResultBlockData {
            tool_call_id: "call_shell".to_owned(),
            output: output.to_owned(),
            is_error: Some(false),
            synthetic: None,
        }
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
    fn renders_multiline_command_with_prompt_indentation() {
        let mut component = ShellExecutionComponent::new(ShellExecutionOptions {
            command: Some("printf hello\nprintf world".to_owned()),
            show_command: true,
            ..ShellExecutionOptions::default()
        });
        let output = component
            .render(100)
            .iter()
            .map(|line| strip_sgr(line).trim_end().to_owned())
            .collect::<Vec<_>>();
        assert!(output.contains(&"  $ printf hello".to_owned()));
        assert!(output.contains(&"    printf world".to_owned()));
    }

    #[test]
    fn collapses_and_expands_shell_results() {
        let output = "line1\nline2\nline3\nline4\nline5";
        let mut collapsed = ShellExecutionComponent::new(ShellExecutionOptions {
            result: Some(result(output)),
            ..ShellExecutionOptions::default()
        });
        let collapsed = strip_sgr(&collapsed.render(100).join("\n"));
        assert!(collapsed.contains("line1"));
        assert!(collapsed.contains("line3"));
        assert!(!collapsed.contains("line4"));
        assert!(collapsed.contains("... (2 more lines, ctrl+o to expand)"));

        let mut expanded = ShellExecutionComponent::new(ShellExecutionOptions {
            result: Some(result(output)),
            expanded: true,
            ..ShellExecutionOptions::default()
        });
        let expanded = strip_sgr(&expanded.render(100).join("\n"));
        assert!(expanded.contains("line4"));
        assert!(expanded.contains("line5"));
        assert!(!expanded.contains("ctrl+o to expand"));
    }

    #[test]
    fn supports_unbounded_or_capped_command_preview() {
        let command = (1..=20)
            .map(|index| format!("step{index}"))
            .collect::<Vec<_>>()
            .join("\n");
        let mut unbounded = ShellExecutionComponent::new(ShellExecutionOptions {
            command: Some(command.clone()),
            show_command: true,
            ..ShellExecutionOptions::default()
        });
        assert!(strip_sgr(&unbounded.render(100).join("\n")).contains("step20"));

        let mut capped = ShellExecutionComponent::new(ShellExecutionOptions {
            command: Some(command),
            show_command: true,
            command_preview_lines: Some(2),
            ..ShellExecutionOptions::default()
        });
        let capped = strip_sgr(&capped.render(100).join("\n"));
        assert!(capped.contains("step2"));
        assert!(!capped.contains("step3"));
    }

    #[test]
    fn trims_trailing_but_preserves_internal_empty_lines() {
        let mut component = ShellExecutionComponent::new(ShellExecutionOptions {
            result: Some(result("a\n\nb\n\n\n")),
            ..ShellExecutionOptions::default()
        });
        let output = strip_sgr(&component.render(100).join("\n"));
        assert!(output.contains('a'));
        assert!(output.contains('b'));
        assert!(!output.contains("more lines"));
    }

    #[test]
    fn result_renderer_does_not_duplicate_the_command() {
        let tool_call = ToolCallBlockData {
            id: "call_1".to_owned(),
            name: "Bash".to_owned(),
            args: serde_json::Map::from_iter([(
                "command".to_owned(),
                serde_json::Value::String("echo hidden".to_owned()),
            )]),
            description: None,
            streaming_arguments: None,
            streaming_started_at_ms: None,
            step: None,
            turn_id: None,
            truncated: None,
        };
        let mut components = shell_execution_result_renderer(&tool_call, result("ok"), false);
        let rendered = components
            .iter_mut()
            .flat_map(|component| component.render(100))
            .map(|line| strip_sgr(&line))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(!rendered.contains("$ echo"));
        assert!(rendered.contains("ok"));
    }
}
