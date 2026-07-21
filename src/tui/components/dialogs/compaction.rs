use std::{
    any::Any,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

use crate::tui::{
    components::{
        Component, ComponentRole,
        render::{truncate_to_width, wrap_text_with_ansi},
    },
    theme::{ColorToken, current_theme},
};

const STATUS_BULLET: &str = "● ";
const BLINK_INTERVAL: Duration = Duration::from_millis(500);

struct BlinkWorker {
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl BlinkWorker {
    fn stop(mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(handle) = self.handle.take() {
            handle.thread().unpark();
            let _ = handle.join();
        }
    }
}

/// Transcript block for context-compaction progress and results.
///
/// Original: `compaction.ts`, `CompactionComponent`.
pub struct CompactionComponent {
    instruction: Option<String>,
    tip: Option<String>,
    blink_on: Arc<AtomicBool>,
    blink_worker: Option<BlinkWorker>,
    request_render: Option<Arc<dyn Fn() + Send + Sync>>,
    done: bool,
    canceled: bool,
    tokens_before: Option<u64>,
    tokens_after: Option<u64>,
    summary: Option<String>,
    expanded: bool,
}

impl CompactionComponent {
    pub fn new(
        instruction: Option<String>,
        tip: Option<String>,
        request_render: Option<Arc<dyn Fn() + Send + Sync>>,
    ) -> Self {
        let mut component = Self {
            instruction,
            tip,
            blink_on: Arc::new(AtomicBool::new(true)),
            blink_worker: None,
            request_render,
            done: false,
            canceled: false,
            tokens_before: None,
            tokens_after: None,
            summary: None,
            expanded: false,
        };
        component.start_blink();
        component
    }

    pub fn mark_done(
        &mut self,
        tokens_before: Option<u64>,
        tokens_after: Option<u64>,
        summary: Option<String>,
    ) -> bool {
        if self.done || self.canceled {
            return false;
        }
        self.done = true;
        self.tokens_before = tokens_before;
        self.tokens_after = tokens_after;
        self.summary = summary;
        self.stop_blink();
        self.request_render();
        true
    }

    pub fn mark_canceled(&mut self) -> bool {
        if self.done || self.canceled {
            return false;
        }
        self.canceled = true;
        self.stop_blink();
        self.request_render();
        true
    }

    pub fn set_expanded(&mut self, expanded: bool) -> bool {
        if self.expanded == expanded {
            return false;
        }
        self.expanded = expanded;
        self.request_render();
        true
    }

    pub fn is_expanded(&self) -> bool {
        self.expanded
    }

    pub fn is_finished(&self) -> bool {
        self.done || self.canceled
    }

    pub fn dispose(&mut self) {
        self.stop_blink();
    }

    /// Advances the blink state for runtimes that own animation timing.
    pub fn advance_blink(&mut self) {
        self.blink_on.fetch_xor(true, Ordering::AcqRel);
        self.request_render();
    }

    fn request_render(&self) {
        if let Some(callback) = &self.request_render {
            callback();
        }
    }

    fn start_blink(&mut self) {
        if self.request_render.is_none() || self.blink_worker.is_some() || self.is_finished() {
            return;
        }
        let blink_on = Arc::clone(&self.blink_on);
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let request_render = Arc::clone(self.request_render.as_ref().expect("checked above"));
        let handle = thread::spawn(move || {
            while !thread_stop.load(Ordering::Acquire) {
                thread::park_timeout(BLINK_INTERVAL);
                if thread_stop.load(Ordering::Acquire) {
                    break;
                }
                blink_on.fetch_xor(true, Ordering::AcqRel);
                request_render();
            }
        });
        self.blink_worker = Some(BlinkWorker {
            stop,
            handle: Some(handle),
        });
    }

    fn stop_blink(&mut self) {
        if let Some(worker) = self.blink_worker.take() {
            worker.stop();
        }
    }

    fn build_header(&self) -> String {
        if self.done {
            let mut header = format!(
                "{}{}",
                current_theme().fg(ColorToken::Success, STATUS_BULLET),
                current_theme().bold_fg(ColorToken::Success, "Compaction complete")
            );
            if let (Some(before), Some(after)) = (self.tokens_before, self.tokens_after) {
                header.push_str(&current_theme().dim(&format!(" ({before} → {after} tokens)")));
            }
            if self
                .summary
                .as_deref()
                .is_some_and(|summary| !summary.is_empty())
            {
                header.push_str(&current_theme().dim(&format!(
                    " (Ctrl-O to {} compaction summary)",
                    if self.expanded { "hide" } else { "show" }
                )));
            }
            return header;
        }
        if self.canceled {
            return format!(
                "{}{}",
                current_theme().fg(ColorToken::Warning, STATUS_BULLET),
                current_theme().bold_fg(ColorToken::Warning, "Compaction cancelled")
            );
        }
        let bullet = if self.blink_on.load(Ordering::Relaxed) {
            current_theme().fg(ColorToken::Text, STATUS_BULLET)
        } else {
            "  ".to_owned()
        };
        let tip = self.tip.as_ref().map_or_else(String::new, |tip| {
            current_theme().fg(ColorToken::TextDim, &format!(" · Tip: {tip}"))
        });
        format!(
            "{bullet}{}{tip}",
            current_theme().bold_fg(ColorToken::Primary, "Compacting context...")
        )
    }

    fn render_compaction(&self, width: usize) -> Vec<String> {
        let width = width.max(1);
        let mut lines = vec![
            String::new(),
            truncate_to_width(&self.build_header(), width, "", false),
        ];
        if let Some(instruction) = &self.instruction {
            lines.extend(render_dim_text(&format!("  {instruction}"), width));
        }
        if self.expanded
            && let Some(summary) = self
                .summary
                .as_deref()
                .filter(|summary| !summary.is_empty())
        {
            let indented = summary
                .split('\n')
                .map(|line| format!("  {line}"))
                .collect::<Vec<_>>()
                .join("\n");
            lines.extend(render_dim_text(&indented, width));
        }
        lines
    }
}

impl Component for CompactionComponent {
    fn render(&mut self, width: usize) -> Vec<String> {
        self.render_compaction(width)
    }

    fn invalidate(&mut self) {}

    fn role(&self) -> ComponentRole {
        ComponentRole::Compaction
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

impl Drop for CompactionComponent {
    fn drop(&mut self) {
        self.stop_blink();
    }
}

fn render_dim_text(text: &str, width: usize) -> Vec<String> {
    wrap_text_with_ansi(&current_theme().dim(text), width)
        .into_iter()
        .map(|line| truncate_to_width(&line, width, "", false))
        .collect()
}

#[cfg(test)]
mod tests {
    use std::sync::{Mutex, mpsc};

    use crate::tui::components::render::visible_width;

    use super::*;

    #[test]
    fn transitions_from_live_to_done_and_expands_summary() {
        let renders = Arc::new(Mutex::new(0usize));
        let callback = Arc::clone(&renders);
        let mut compaction = CompactionComponent::new(
            Some("Keep the API decisions".to_owned()),
            Some("Use /compact instructions".to_owned()),
            Some(Arc::new(move || {
                *callback.lock().expect("renders") += 1;
            })),
        );
        let live = compaction.render(80);
        assert!(strip_sgr(&live[1]).contains("Compacting context..."));
        assert!(strip_sgr(&live[1]).contains("Tip: Use /compact instructions"));
        compaction.advance_blink();
        assert!(strip_sgr(&compaction.render(80)[1]).starts_with("  "));

        assert!(compaction.mark_done(
            Some(12_000),
            Some(4_000),
            Some("First line\nSecond line".to_owned())
        ));
        assert!(!compaction.mark_canceled());
        let collapsed = compaction.render(80);
        assert!(strip_sgr(&collapsed[1]).contains("12000 → 4000 tokens"));
        assert!(strip_sgr(&collapsed[1]).contains("Ctrl-O to show"));
        assert!(
            !collapsed
                .iter()
                .any(|line| strip_sgr(line).contains("First line"))
        );

        assert!(compaction.set_expanded(true));
        let expanded = compaction.render(80);
        assert!(strip_sgr(&expanded[1]).contains("Ctrl-O to hide"));
        assert!(
            expanded
                .iter()
                .any(|line| strip_sgr(line).contains("First line"))
        );
        assert!(expanded.iter().all(|line| visible_width(line) <= 80));
    }

    #[test]
    fn cancellation_is_terminal_and_has_warning_header() {
        let mut compaction = CompactionComponent::new(None, None, None);
        assert!(compaction.mark_canceled());
        assert!(!compaction.mark_canceled());
        assert!(!compaction.mark_done(None, None, None));
        assert!(compaction.is_finished());
        assert!(strip_sgr(&compaction.render(40)[1]).contains("Compaction cancelled"));
    }

    #[test]
    fn worker_requests_render_and_dispose_stops_it() {
        let (sender, receiver) = mpsc::channel();
        let mut compaction = CompactionComponent::new(
            None,
            None,
            Some(Arc::new(move || {
                let _ = sender.send(());
            })),
        );
        receiver
            .recv_timeout(Duration::from_millis(800))
            .expect("blink callback");
        compaction.dispose();
        while receiver.try_recv().is_ok() {}
        assert!(receiver.recv_timeout(Duration::from_millis(550)).is_err());
    }

    #[test]
    fn empty_summary_has_no_shortcut_or_expanded_child() {
        let mut compaction = CompactionComponent::new(None, None, None);
        compaction.mark_done(None, None, Some(String::new()));
        compaction.set_expanded(true);
        let lines = compaction.render(30);
        assert_eq!(lines.len(), 2);
        assert!(!strip_sgr(&lines[1]).contains("Ctrl-O"));
    }

    fn strip_sgr(text: &str) -> String {
        let regex = regex::Regex::new(r"\x1b\[[0-9;]*m").expect("valid SGR regex");
        regex.replace_all(text, "").into_owned()
    }
}
