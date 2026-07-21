use std::path::Path;

use url::Url;

use crate::{
    tui::components::{
        Component,
        markdown::{Markdown, MarkdownOptions},
        render::{truncate_to_width, visible_width},
    },
    utils::terminal_hyperlink::to_terminal_hyperlink,
};

const LEFT_MARGIN: usize = 2;
const SIDE_PADDING: usize = 1;
const TITLE_PREFIX: &str = " plan: ";
const TITLE_SUFFIX: &str = " ";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanBoxStatus {
    pub label: String,
    pub color_hex: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PlanBoxOptions {
    pub status: Option<PlanBoxStatus>,
}

/// Renders an ExitPlanMode plan inside a width-aware full border.
///
/// Original:
/// `src/tui/components/messages/plan-box.ts`, `PlanBoxComponent`.
pub struct PlanBoxComponent {
    markdown: Markdown,
    border_hex: String,
    plan_path: Option<String>,
    status: Option<PlanBoxStatus>,
    cached_width: Option<usize>,
    cached_lines: Option<Vec<String>>,
}

impl PlanBoxComponent {
    pub fn new(
        plan: &str,
        border_hex: impl Into<String>,
        plan_path: Option<String>,
        options: PlanBoxOptions,
    ) -> Self {
        Self {
            markdown: Markdown::new(plan.trim(), 0, 0, MarkdownOptions::default()),
            border_hex: border_hex.into(),
            plan_path,
            status: options.status,
            cached_width: None,
            cached_lines: None,
        }
    }

    pub fn invalidate(&mut self) {
        self.cached_width = None;
        self.cached_lines = None;
        self.markdown.invalidate();
    }

    pub fn render(&mut self, width: usize) -> Vec<String> {
        if width == 0 {
            return vec![String::new()];
        }
        if width < LEFT_MARGIN + 4 {
            return self
                .markdown
                .render(width.max(1))
                .into_iter()
                .map(|line| truncate_to_width(&line, width, "…", false))
                .collect();
        }
        if self.cached_width == Some(width)
            && let Some(lines) = &self.cached_lines
        {
            return lines.clone();
        }

        let horizontal_length = width.saturating_sub(LEFT_MARGIN + 2).max(2);
        let content_width = horizontal_length
            .saturating_sub(SIDE_PADDING.saturating_mul(2))
            .max(1);
        let indent = " ".repeat(LEFT_MARGIN);
        let title = self.build_title(horizontal_length);
        let trailing_dashes = horizontal_length.saturating_sub(visible_width(&title));
        let top = format!(
            "{indent}{}{}{}{}",
            paint(&self.border_hex, "┌"),
            paint(&self.border_hex, &title),
            paint(&self.border_hex, &"─".repeat(trailing_dashes)),
            paint(&self.border_hex, "┐")
        );
        let bottom = format!(
            "{indent}{}{}{}",
            paint(&self.border_hex, "└"),
            paint(&self.border_hex, &"─".repeat(horizontal_length)),
            paint(&self.border_hex, "┘")
        );

        let mut lines = vec![top];
        for raw in self.markdown.render(content_width) {
            let padding = content_width.saturating_sub(visible_width(&raw));
            lines.push(format!(
                "{indent}{} {raw}{} {}",
                paint(&self.border_hex, "│"),
                " ".repeat(padding),
                paint(&self.border_hex, "│")
            ));
        }
        lines.push(bottom);

        let fitted = lines
            .into_iter()
            .map(|line| truncate_to_width(&line, width, "…", false))
            .collect::<Vec<_>>();
        self.cached_width = Some(width);
        self.cached_lines = Some(fitted.clone());
        fitted
    }

    fn build_title(&self, horizontal_length: usize) -> String {
        let fallback = " plan ";
        let status_suffix = self.build_status_suffix();
        let fallback_with_status = format!(" plan{status_suffix} ");
        let budget = horizontal_length.saturating_sub(1);
        let fallback_source = if visible_width(&fallback_with_status) <= budget {
            fallback_with_status.as_str()
        } else {
            fallback
        };
        let fallback_title = truncate_to_width(fallback_source, budget, "…", false);
        let Some(plan_path) = self.plan_path.as_deref().filter(|path| !path.is_empty()) else {
            return fallback_title;
        };
        let basename = basename_like(plan_path);
        if basename.is_empty() {
            return fallback_title;
        }

        let linked = if is_absolute_like(plan_path) {
            file_url(plan_path)
                .map(|url| to_terminal_hyperlink(basename, &url))
                .unwrap_or_else(|| basename.to_owned())
        } else {
            basename.to_owned()
        };
        let title = format!("{TITLE_PREFIX}{linked}{status_suffix}{TITLE_SUFFIX}");
        if visible_width(&title) > budget {
            fallback_title
        } else {
            title
        }
    }

    fn build_status_suffix(&self) -> String {
        self.status
            .as_ref()
            .filter(|status| !status.label.is_empty())
            .map_or_else(String::new, |status| {
                format!(" · {}", paint(&status.color_hex, &status.label))
            })
    }
}

fn basename_like(path: &str) -> &str {
    path.rsplit(['/', '\\'])
        .find(|part| !part.is_empty())
        .unwrap_or(path)
}

fn is_absolute_like(path: &str) -> bool {
    Path::new(path).is_absolute() || path.starts_with('/') || path.as_bytes().get(1) == Some(&b':')
}

fn file_url(path: &str) -> Option<String> {
    Url::from_file_path(path)
        .ok()
        .or_else(|| {
            path.starts_with('/').then(|| {
                Url::parse("file:///")
                    .expect("static file URL is valid")
                    .join(path.trim_start_matches('/'))
                    .expect("absolute path segments form a valid file URL")
            })
        })
        .map(Into::into)
}

fn paint(hex: &str, text: &str) -> String {
    let value = u32::from_str_radix(hex.strip_prefix('#').unwrap_or_default(), 16).ok();
    let Some(value) = value.filter(|_| hex.len() == 7) else {
        return text.to_owned();
    };
    let red = (value >> 16) & 0xff;
    let green = (value >> 8) & 0xff;
    let blue = value & 0xff;
    format!("\x1b[38;2;{red};{green};{blue}m{text}\x1b[39m")
}

#[cfg(test)]
mod tests {
    use regex::Regex;
    use std::sync::LazyLock;

    use super::*;

    const SUCCESS: &str = "#4EC87E";
    const ERROR: &str = "#E85454";

    fn strip(text: &str) -> String {
        static ESCAPES: LazyLock<Regex> = LazyLock::new(|| {
            Regex::new("\\x1b\\[[0-9;]*m|\\x1b\\]8;;[^\\x07]*\\x07")
                .expect("valid terminal escape regex")
        });
        ESCAPES.replace_all(text, "").into_owned()
    }

    fn box_for(path: Option<&str>, options: PlanBoxOptions) -> PlanBoxComponent {
        PlanBoxComponent::new("# Hello", SUCCESS, path.map(str::to_owned), options)
    }

    #[test]
    fn falls_back_to_bare_plan_title_without_a_path() {
        let mut component = box_for(None, PlanBoxOptions::default());
        let top = strip(&component.render(60)[0]);
        assert!(top.contains("┌ plan "));
        assert!(!top.contains("plan:"));
    }

    #[test]
    fn shows_only_the_path_basename_and_status() {
        let mut component = box_for(
            Some("/tmp/projects/foo/.kimi-code/plans/rejected-plan.md"),
            PlanBoxOptions {
                status: Some(PlanBoxStatus {
                    label: "Rejected".to_owned(),
                    color_hex: ERROR.to_owned(),
                }),
            },
        );
        let top = strip(&component.render(80)[0]);
        assert!(top.contains(" plan: rejected-plan.md · Rejected "));
        assert!(!top.contains("/tmp/"));
    }

    #[test]
    fn hyperlinks_absolute_paths_but_not_relative_paths() {
        let mut absolute = box_for(Some("/tmp/plan.md"), PlanBoxOptions::default());
        let top = &absolute.render(60)[0];
        assert!(top.contains("\x1b]8;;file:"));
        assert!(top.contains("plan.md\x1b]8;;\x07"));

        let mut relative = box_for(Some("relative/plan.md"), PlanBoxOptions::default());
        let top = &relative.render(60)[0];
        assert!(!top.contains("\x1b]8;;"));
        assert!(strip(top).contains(" plan: plan.md "));
    }

    #[test]
    fn degrades_title_and_keeps_all_lines_within_narrow_widths() {
        let mut component = PlanBoxComponent::new(
            &format!("# Hello\n\n{}", "step with a long description ".repeat(4)),
            SUCCESS,
            Some("/tmp/very-long-slug-name.md".to_owned()),
            PlanBoxOptions::default(),
        );
        let narrow = strip(&component.render(14)[0]);
        assert!(narrow.contains(" plan "));
        assert!(!narrow.contains("plan:"));

        for width in [39, 14, 10, 8, 4, 1] {
            assert!(
                component
                    .render(width)
                    .iter()
                    .all(|line| visible_width(line) <= width)
            );
        }
    }

    #[test]
    fn renders_every_plan_line_without_a_truncation_footer_and_caches() {
        let plan = (1..=30)
            .map(|number| format!("- step {number}"))
            .collect::<Vec<_>>()
            .join("\n");
        let mut component = PlanBoxComponent::new(&plan, SUCCESS, None, PlanBoxOptions::default());
        let first = component.render(80);
        let output = strip(&first.join("\n"));
        assert!(output.contains("step 1"));
        assert!(output.contains("step 30"));
        assert!(!output.contains("more lines"));
        assert_eq!(first, component.render(80));

        component.invalidate();
        assert!(component.cached_lines.is_none());
    }
}
