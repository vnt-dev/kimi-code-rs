use std::{any::Any, sync::Arc};

use indexmap::IndexMap;

use crate::{
    tui::{
        components::{Component, ComponentRole, render::truncate_to_width},
        theme::{ColorToken, current_theme},
    },
    utils::usage::usage_format::format_token_count,
};

use super::tool_call::{
    SubagentPhase, ToolCallComponent, ToolCallSubagentSnapshot, format_elapsed,
};

const STATUS_BULLET: &str = "● ";
const DETACH_HINT_TEXT: &str = "Press Ctrl+B to run in background";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct PhaseCounts {
    done: usize,
    failed: usize,
    backgrounded: usize,
    running: usize,
    waiting: usize,
    starting: usize,
}

impl PhaseCounts {
    fn terminal(self) -> usize {
        self.done + self.failed + self.backgrounded
    }
}

/// Groups Agent cards from one step while retaining each hidden state machine.
///
/// Original: `src/tui/components/messages/agent-group.ts`,
/// `AgentGroupComponent`.
pub struct AgentGroupComponent {
    entries: IndexMap<String, ToolCallComponent>,
    request_render: Option<Arc<dyn Fn() + Send + Sync>>,
    render_cache: Option<(usize, Vec<ToolCallSubagentSnapshot>, Vec<String>)>,
}

impl AgentGroupComponent {
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

    pub fn tool_components(&self) -> impl Iterator<Item = &ToolCallComponent> {
        self.entries.values()
    }

    pub fn attach(&mut self, tool_call_id: impl Into<String>, tool_call: ToolCallComponent) {
        let tool_call_id = tool_call_id.into();
        if self.entries.contains_key(&tool_call_id) {
            return;
        }
        self.entries.insert(tool_call_id, tool_call);
        self.changed();
    }

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
        for component in self.entries.values_mut() {
            component.dispose();
        }
    }

    fn changed(&mut self) {
        self.render_cache = None;
        if let Some(request_render) = &self.request_render {
            request_render();
        }
    }

    fn snapshots(&self) -> Vec<ToolCallSubagentSnapshot> {
        self.entries
            .values()
            .map(ToolCallComponent::get_subagent_snapshot)
            .collect()
    }

    fn build_header(snapshots: &[ToolCallSubagentSnapshot]) -> String {
        let total = snapshots.len();
        let counts = count_phases(snapshots);
        let all_done = counts.terminal() == total;
        let bullet = current_theme().fg(
            if all_done {
                ColorToken::Success
            } else {
                ColorToken::Text
            },
            STATUS_BULLET,
        );
        let elapsed_seconds = max_elapsed_seconds(snapshots);
        if all_done {
            let types = snapshots
                .iter()
                .filter_map(|snapshot| snapshot.agent_name.as_deref())
                .collect::<std::collections::BTreeSet<_>>();
            let label = if types.len() == 1 {
                format!(
                    "{total} {} agents finished",
                    types.first().copied().unwrap_or("agent")
                )
            } else {
                format!("{total} agents finished")
            };
            let tool_count = snapshots.iter().map(|snapshot| snapshot.tool_count).sum();
            let tokens = snapshots.iter().map(|snapshot| snapshot.tokens).sum();
            return format!(
                "{bullet}{}{}",
                current_theme().bold_fg(ColorToken::Primary, &label),
                format_header_tail(tool_count, tokens, elapsed_seconds)
            );
        }

        let breakdown = format_breakdown_parts(counts);
        let label = if breakdown.is_empty() {
            format!("Running {total} agents")
        } else {
            format!("Running {total} agents ({})", breakdown.join(", "))
        };
        format!(
            "{bullet}{}{}",
            current_theme().bold_fg(ColorToken::Primary, &label),
            format_header_tail(0, 0, elapsed_seconds)
        )
    }

    fn append_lines(lines: &mut Vec<String>, snapshot: &ToolCallSubagentSnapshot, is_last: bool) {
        let branch = if is_last { "└─" } else { "├─" };
        let agent_type = snapshot.agent_name.as_deref().unwrap_or("agent");
        let description = if snapshot.tool_call_description.is_empty() {
            "(no description)"
        } else {
            &snapshot.tool_call_description
        };
        lines.push(format!(
            "  {branch} {} {}{}{}",
            current_theme().fg(ColorToken::Primary, agent_type),
            current_theme().dim(&format!("· {description}")),
            format_stats(snapshot),
            format_line_tail(snapshot)
        ));

        let second_branch = if is_last { "   " } else { "│  " };
        if snapshot.phase == Some(SubagentPhase::Failed) {
            let error = snapshot
                .error_text
                .as_deref()
                .unwrap_or("Failed")
                .lines()
                .next()
                .unwrap_or("Failed");
            lines.push(format!(
                "  {second_branch}    {}",
                current_theme().fg(ColorToken::Error, &format!("Error: {error}"))
            ));
        } else if !matches!(
            snapshot.phase,
            Some(SubagentPhase::Done | SubagentPhase::Backgrounded)
        ) {
            let activity = snapshot
                .latest_activity
                .as_deref()
                .unwrap_or_else(|| fallback_activity_for_phase(snapshot.phase));
            lines.push(format!(
                "  {second_branch}    {}",
                current_theme().dim(activity)
            ));
        }
    }
}

impl Component for AgentGroupComponent {
    fn render(&mut self, width: usize) -> Vec<String> {
        let snapshots = self.snapshots();
        if let Some((cached_width, cached_snapshots, lines)) = &self.render_cache
            && *cached_width == width
            && *cached_snapshots == snapshots
        {
            return lines.clone();
        }

        let mut lines = vec![String::new(), Self::build_header(&snapshots)];
        for (index, snapshot) in snapshots.iter().enumerate() {
            Self::append_lines(&mut lines, snapshot, index + 1 == snapshots.len());
        }
        if should_show_detach_hint(&snapshots) {
            lines.push(format!("  {}", current_theme().dim(DETACH_HINT_TEXT)));
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
        for component in self.entries.values_mut() {
            component.invalidate();
        }
    }

    fn role(&self) -> ComponentRole {
        ComponentRole::AgentGroup
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

fn count_phases(snapshots: &[ToolCallSubagentSnapshot]) -> PhaseCounts {
    let mut counts = PhaseCounts::default();
    for snapshot in snapshots {
        match snapshot.phase {
            Some(SubagentPhase::Done) => counts.done += 1,
            Some(SubagentPhase::Failed) => counts.failed += 1,
            Some(SubagentPhase::Backgrounded) => counts.backgrounded += 1,
            Some(SubagentPhase::Running) => counts.running += 1,
            Some(SubagentPhase::Queued) => counts.waiting += 1,
            Some(SubagentPhase::Spawning) | None => counts.starting += 1,
        }
    }
    counts
}

fn format_breakdown_parts(counts: PhaseCounts) -> Vec<String> {
    [
        (counts.done, "done"),
        (counts.failed, "failed"),
        (counts.backgrounded, "backgrounded"),
        (counts.running, "running"),
        (counts.waiting, "waiting"),
        (counts.starting, "starting"),
    ]
    .into_iter()
    .filter(|(count, _)| *count > 0)
    .map(|(count, label)| format!("{count} {label}"))
    .collect()
}

fn format_stats(snapshot: &ToolCallSubagentSnapshot) -> String {
    let mut parts = vec![format!(
        "{} tool{}",
        snapshot.tool_count,
        if snapshot.tool_count == 1 { "" } else { "s" }
    )];
    if let Some(elapsed) = snapshot.elapsed_seconds {
        parts.push(format_elapsed(elapsed));
    }
    if snapshot.tokens > 0 {
        parts.push(format!(
            "{} tok",
            format_token_count(snapshot.tokens as f64)
        ));
    }
    current_theme().dim(&format!(" · {}", parts.join(" · ")))
}

fn format_line_tail(snapshot: &ToolCallSubagentSnapshot) -> String {
    let separator = current_theme().dim(" · ");
    let status = match snapshot.phase {
        Some(SubagentPhase::Done) => current_theme().fg(ColorToken::Success, "✓ Completed"),
        Some(SubagentPhase::Failed) => current_theme().fg(ColorToken::Error, "✗ Failed"),
        Some(SubagentPhase::Backgrounded) => current_theme().dim("◐ backgrounded"),
        Some(SubagentPhase::Queued) => current_theme().fg(ColorToken::Primary, "Waiting"),
        Some(SubagentPhase::Running) => current_theme().fg(ColorToken::Primary, "Running"),
        Some(SubagentPhase::Spawning) | None => current_theme().fg(ColorToken::Primary, "Starting"),
    };
    format!("{separator}{status}")
}

fn fallback_activity_for_phase(phase: Option<SubagentPhase>) -> &'static str {
    match phase {
        Some(SubagentPhase::Queued) => "Waiting to start…",
        Some(SubagentPhase::Running) => "Still working…",
        Some(SubagentPhase::Spawning) | None => "Starting…",
        Some(SubagentPhase::Done | SubagentPhase::Failed | SubagentPhase::Backgrounded) => "",
    }
}

fn format_header_tail(tool_count: usize, tokens: u64, elapsed_seconds: Option<u64>) -> String {
    let mut parts = Vec::new();
    if tool_count > 0 {
        parts.push(format!(
            "{tool_count} tool{}",
            if tool_count == 1 { "" } else { "s" }
        ));
    }
    if tokens > 0 {
        parts.push(format!("{} tok", format_token_count(tokens as f64)));
    }
    if let Some(elapsed) = elapsed_seconds {
        parts.push(format_elapsed(elapsed));
    }
    if parts.is_empty() {
        String::new()
    } else {
        current_theme().dim(&format!(" · {}", parts.join(" · ")))
    }
}

fn max_elapsed_seconds(snapshots: &[ToolCallSubagentSnapshot]) -> Option<u64> {
    snapshots
        .iter()
        .filter_map(|snapshot| snapshot.elapsed_seconds)
        .max()
}

fn should_show_detach_hint(snapshots: &[ToolCallSubagentSnapshot]) -> bool {
    snapshots.iter().any(|snapshot| {
        matches!(
            snapshot.phase,
            None | Some(SubagentPhase::Running | SubagentPhase::Queued | SubagentPhase::Spawning)
        )
    })
}

#[cfg(test)]
mod tests {
    use regex::Regex;
    use serde_json::Value;

    use crate::tui::{
        components::render::visible_width,
        types::{ToolCallBlockData, ToolResultBlockData},
    };

    use super::*;
    use crate::tui::components::messages::tool_call::{SubagentSpawnMeta, SubagentTextKind};

    fn agent(id: &str, description: &str, name: &str, started: bool) -> ToolCallComponent {
        let mut component = ToolCallComponent::new(
            ToolCallBlockData {
                id: id.to_owned(),
                name: "Agent".to_owned(),
                args: [(
                    "description".to_owned(),
                    Value::String(description.to_owned()),
                )]
                .into_iter()
                .collect(),
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
        );
        let meta = SubagentSpawnMeta {
            agent_id: format!("sub-{id}"),
            agent_name: Some(name.to_owned()),
            run_in_background: false,
        };
        component.state_mut().on_subagent_spawned(&meta);
        if started {
            component.state_mut().on_subagent_started(&meta);
        }
        component
    }

    fn strip(text: &str) -> String {
        Regex::new("\\x1b\\[[0-9;]*m")
            .expect("valid SGR regex")
            .replace_all(text, "")
            .into_owned()
    }

    #[test]
    fn renders_active_breakdown_rows_activity_and_detach_hint() {
        let mut running = agent("one", "inspect project", "explore", true);
        running.state_mut().append_sub_tool_call(
            "read",
            "Read",
            [("path".to_owned(), Value::String("src/a.ts".to_owned()))]
                .into_iter()
                .collect(),
        );
        let waiting = agent("two", "write tests", "coder", false);
        let mut group = AgentGroupComponent::new(None);
        group.attach("one", running);
        group.attach("two", waiting);
        let output = strip(&group.render(120).join("\n"));
        assert!(output.contains("Running 2 agents (1 running, 1 waiting)"));
        assert!(output.contains("explore · inspect project · 0 tools"));
        assert!(output.contains("Using Read (src/a.ts)"));
        assert!(output.contains("coder · write tests · 0 tools"));
        assert!(output.contains("Waiting to start…"));
        assert!(output.contains(DETACH_HINT_TEXT));
    }

    #[test]
    fn terminal_rows_show_done_failure_and_hide_detach_hint() {
        let done = agent("one", "inspect", "explore", true);
        let failed = agent("two", "review", "coder", true);
        let mut group = AgentGroupComponent::new(None);
        group.attach("one", done);
        group.attach("two", failed);
        group.with_entry_mut("one", |component| {
            component
                .state_mut()
                .on_subagent_completed(Default::default(), "done");
        });
        group.with_entry_mut("two", |component| {
            component.state_mut().on_subagent_failed("review failed");
        });
        let output = strip(&group.render(120).join("\n"));
        assert!(output.contains("2 agents finished"));
        assert!(output.contains("✓ Completed"));
        assert!(output.contains("✗ Failed"));
        assert!(output.contains("Error: review failed"));
        assert!(!output.contains(DETACH_HINT_TEXT));
    }

    #[test]
    fn detached_agent_stays_backgrounded_after_result_and_widths_are_bounded() {
        let a = agent("one", "inspect", "explore", true);
        let b = agent("two", "review", "coder", true);
        let mut group = AgentGroupComponent::new(None);
        group.attach("one", a);
        group.attach("two", b);
        group.with_entry_mut("one", |component| {
            component.state_mut().mark_backgrounded();
            component.set_result(ToolResultBlockData {
                tool_call_id: "one".to_owned(),
                output: "agent_id: sub-one".to_owned(),
                is_error: Some(false),
                synthetic: None,
            });
        });
        let output = strip(&group.render(120).join("\n"));
        assert!(output.contains("◐ backgrounded"));
        assert!(!output.contains("✓ Completed"));

        group.with_entry_mut("two", |component| {
            component
                .state_mut()
                .append_subagent_text("working", SubagentTextKind::Text);
        });
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
