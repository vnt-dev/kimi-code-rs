use std::{any::Any, sync::Arc};

use indexmap::IndexMap;

use crate::tui::{
    components::{Component, ComponentRole, render::truncate_to_width},
    theme::{ColorToken, current_theme},
};

use super::tool_call::{ReadPhase, ToolCallComponent, ToolCallReadSnapshot};

const STATUS_BULLET: &str = "● ";
const FAILURE_MARK: &str = "✗ ";

/// Groups two or more Read tool cards into one summary and tree.
///
/// Original: `src/tui/components/messages/read-group.ts`,
/// `ReadGroupComponent`.
pub struct ReadGroupComponent {
    entries: IndexMap<String, ToolCallComponent>,
    request_render: Option<Arc<dyn Fn() + Send + Sync>>,
    render_cache: Option<(usize, Vec<ToolCallReadSnapshot>, Vec<String>)>,
}

impl ReadGroupComponent {
    pub fn new(request_render: Option<Arc<dyn Fn() + Send + Sync>>) -> Self {
        Self {
            entries: IndexMap::new(),
            request_render,
            render_cache: None,
        }
    }

    pub fn size(&self) -> usize {
        self.entries.len()
    }

    /// Moves a standalone card into the group as its hidden state container.
    /// Re-attaching an existing id is a no-op.
    pub fn attach(&mut self, tool_call_id: impl Into<String>, tool_call: ToolCallComponent) {
        let tool_call_id = tool_call_id.into();
        if self.entries.contains_key(&tool_call_id) {
            return;
        }
        self.entries.insert(tool_call_id, tool_call);
        self.changed();
    }

    /// Applies an event to a grouped hidden card and refreshes the group.
    pub fn with_entry_mut<R>(
        &mut self,
        tool_call_id: &str,
        update: impl FnOnce(&mut ToolCallComponent) -> R,
    ) -> Option<R> {
        let result = self.entries.get_mut(tool_call_id).map(update);
        if result.is_some() {
            self.changed();
        }
        result
    }

    pub fn dispose(&mut self) {
        for tool_call in self.entries.values_mut() {
            tool_call.dispose();
        }
    }

    fn changed(&mut self) {
        self.render_cache = None;
        if let Some(request_render) = &self.request_render {
            request_render();
        }
    }

    fn snapshots(&self) -> Vec<ToolCallReadSnapshot> {
        self.entries
            .values()
            .map(ToolCallComponent::get_read_snapshot)
            .collect()
    }

    fn build_header(&self, snapshots: &[ToolCallReadSnapshot]) -> String {
        let total = snapshots.len();
        let pending = snapshots
            .iter()
            .filter(|snapshot| snapshot.phase == ReadPhase::Pending)
            .count();
        let failed = snapshots
            .iter()
            .filter(|snapshot| snapshot.phase == ReadPhase::Failed)
            .count();
        let total_lines = snapshots
            .iter()
            .map(|snapshot| snapshot.lines)
            .sum::<usize>();
        let theme = current_theme();

        if pending > 0 {
            return format!(
                "{}{}",
                theme.fg(ColorToken::Text, STATUS_BULLET),
                theme.bold_fg(ColorToken::Primary, &format!("Reading {total} files…"))
            );
        }
        if failed == total && total > 0 {
            return format!(
                "{}{}{}",
                theme.fg(ColorToken::Error, FAILURE_MARK),
                theme.bold_fg(ColorToken::Error, &format!("Read {total} files")),
                theme.fg(ColorToken::Error, " · failed")
            );
        }

        let failure = if failed > 0 {
            theme.fg(ColorToken::Error, &format!(" · {failed} failed"))
        } else {
            String::new()
        };
        format!(
            "{}{}{}{}",
            theme.fg(ColorToken::Success, STATUS_BULLET),
            theme.bold_fg(ColorToken::Primary, &format!("Read {total} files")),
            theme.dim(&format!(
                " · {total_lines} {}",
                if total_lines == 1 { "line" } else { "lines" }
            )),
            failure
        )
    }

    fn build_body_line(snapshot: &ToolCallReadSnapshot, is_last: bool) -> String {
        let branch = if is_last { "└─" } else { "├─" };
        let path = snapshot.file_path.as_deref().unwrap_or_default();
        let tail = match snapshot.phase {
            ReadPhase::Pending => current_theme().dim(" · reading…"),
            ReadPhase::Failed => current_theme().fg(ColorToken::Error, " · failed"),
            ReadPhase::Done => current_theme().dim(&format!(
                " · {} {}",
                snapshot.lines,
                if snapshot.lines == 1 { "line" } else { "lines" }
            )),
        };
        format!(
            "  {branch} {}{tail}",
            current_theme().fg(ColorToken::Text, path)
        )
    }
}

impl Component for ReadGroupComponent {
    fn render(&mut self, width: usize) -> Vec<String> {
        let snapshots = self.snapshots();
        if let Some((cached_width, cached_snapshots, lines)) = &self.render_cache
            && *cached_width == width
            && *cached_snapshots == snapshots
        {
            return lines.clone();
        }

        let mut lines = vec![String::new(), self.build_header(&snapshots)];
        let visible = snapshots
            .iter()
            .filter(|snapshot| {
                snapshot
                    .file_path
                    .as_ref()
                    .is_some_and(|path| !path.is_empty())
            })
            .collect::<Vec<_>>();
        for (index, snapshot) in visible.iter().enumerate() {
            lines.push(Self::build_body_line(snapshot, index + 1 == visible.len()));
        }
        let fitted = lines
            .into_iter()
            .map(|line| truncate_to_width(&line, width, "…", false))
            .collect::<Vec<_>>();
        self.render_cache = Some((width, snapshots, fitted.clone()));
        fitted
    }

    fn invalidate(&mut self) {
        self.render_cache = None;
        for tool_call in self.entries.values_mut() {
            tool_call.invalidate();
        }
    }

    fn role(&self) -> ComponentRole {
        ComponentRole::ReadGroup
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use regex::Regex;
    use serde_json::Value;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use crate::tui::{
        components::render::visible_width,
        types::{ToolCallBlockData, ToolResultBlockData},
    };

    use super::*;

    fn call(id: &str, path: Option<&str>) -> ToolCallComponent {
        ToolCallComponent::new(
            ToolCallBlockData {
                id: id.to_owned(),
                name: "Read".to_owned(),
                args: path.map_or_else(Default::default, |path| {
                    [("path".to_owned(), Value::String(path.to_owned()))]
                        .into_iter()
                        .collect()
                }),
                description: None,
                streaming_arguments: None,
                streaming_started_at_ms: None,
                subagent: None,
                step: None,
                turn_id: None,
                truncated: None,
            },
            None,
            None,
        )
    }

    fn result(id: &str, output: &str, failed: bool) -> ToolResultBlockData {
        ToolResultBlockData {
            tool_call_id: id.to_owned(),
            output: output.to_owned(),
            is_error: Some(failed),
            synthetic: None,
        }
    }

    fn strip(text: &str) -> String {
        Regex::new("\\x1b\\[[0-9;]*m")
            .expect("valid SGR regex")
            .replace_all(text, "")
            .into_owned()
    }

    #[test]
    fn renders_pending_then_done_summary_and_tree() {
        let mut group = ReadGroupComponent::new(None);
        group.attach("one", call("one", Some("src/one.rs")));
        group.attach("two", call("two", Some("src/two.rs")));
        assert_eq!(group.size(), 2);
        let pending = strip(&group.render(80).join("\n"));
        assert!(pending.contains("Reading 2 files…"));
        assert!(pending.contains("├─ src/one.rs · reading…"));
        assert!(pending.contains("└─ src/two.rs · reading…"));

        group.with_entry_mut("one", |call| call.set_result(result("one", "a\nb", false)));
        group.with_entry_mut("two", |call| call.set_result(result("two", "c", false)));
        let done = strip(&group.render(80).join("\n"));
        assert!(done.contains("Read 2 files · 3 lines"));
        assert!(done.contains("src/one.rs · 2 lines"));
        assert!(done.contains("src/two.rs · 1 line"));
    }

    #[test]
    fn renders_partial_and_total_failures() {
        let mut group = ReadGroupComponent::new(None);
        group.attach("one", call("one", Some("one.rs")));
        group.attach("two", call("two", Some("two.rs")));
        group.with_entry_mut("one", |call| call.set_result(result("one", "ok", false)));
        group.with_entry_mut("two", |call| {
            call.set_result(result("two", "missing", true))
        });
        let partial = strip(&group.render(80).join("\n"));
        assert!(partial.contains("Read 2 files · 1 line · 1 failed"));
        assert!(partial.contains("two.rs · failed"));

        group.with_entry_mut("one", |call| {
            call.set_result(result("one", "missing", true))
        });
        let failed = strip(&group.render(80).join("\n"));
        assert!(failed.contains("Read 2 files · failed"));
    }

    #[test]
    fn ignores_duplicate_ids_requests_render_and_fits_width() {
        let renders = Arc::new(AtomicUsize::new(0));
        let callback_renders = Arc::clone(&renders);
        let mut group = ReadGroupComponent::new(Some(Arc::new(move || {
            callback_renders.fetch_add(1, Ordering::Relaxed);
        })));
        group.attach("one", call("one", Some("very/long/path/to/one.rs")));
        group.attach("one", call("duplicate", Some("ignored.rs")));
        assert_eq!(group.size(), 1);
        assert_eq!(renders.load(Ordering::Relaxed), 1);
        group.with_entry_mut("one", |call| call.set_result(result("one", "ok", false)));
        assert_eq!(renders.load(Ordering::Relaxed), 2);
        for width in [1, 4, 10, 39] {
            assert!(
                group
                    .render(width)
                    .iter()
                    .all(|line| visible_width(line) <= width)
            );
        }
    }
}
