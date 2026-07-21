use std::any::Any;

use crate::tui::{
    components::{
        Component, ComponentRole,
        media::{
            code_highlight::{highlight_lines, lang_from_path},
            diff_preview::{ClusteredDiffOptions, render_diff_lines_clustered},
        },
        render::{truncate_to_width, visible_width},
    },
    keys::{EditorKey, ListKey, matches_editor_key, matches_list_key},
    reverse_rpc::{DiffDisplayBlock, FileContentDisplayBlock},
    theme::{ColorToken, current_theme},
    utils::printable_key::printable_char,
};

const ELLIPSIS: &str = "…";

type CloseCallback = dyn FnMut() + Send;
type RowsProvider = dyn Fn() -> usize + Send + Sync;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApprovalPreviewBlock {
    Diff(DiffDisplayBlock),
    FileContent(FileContentDisplayBlock),
}

struct BuiltBody {
    lines: Vec<String>,
    title: String,
}

/// Full-screen, snapshot-based approval diff or file-content viewer.
///
/// Original: `approval-preview.ts`, `ApprovalPreviewViewer`.
pub struct ApprovalPreviewViewer {
    pub focused: bool,
    block: ApprovalPreviewBlock,
    on_close: Box<CloseCallback>,
    rows: Box<RowsProvider>,
    body_lines: Vec<String>,
    header_title: String,
    scroll_top: usize,
}

impl ApprovalPreviewViewer {
    pub fn new<C, R>(block: ApprovalPreviewBlock, on_close: C, rows: R) -> Self
    where
        C: FnMut() + Send + 'static,
        R: Fn() -> usize + Send + Sync + 'static,
    {
        let built = build_body(&block);
        Self {
            focused: false,
            block,
            on_close: Box::new(on_close),
            rows: Box::new(rows),
            body_lines: built.lines,
            header_title: built.title,
            scroll_top: 0,
        }
    }

    pub fn scroll_top(&self) -> usize {
        self.scroll_top
    }

    pub fn body_len(&self) -> usize {
        self.body_lines.len()
    }

    /// Original: `ApprovalPreviewViewer.handleInput()`.
    pub fn handle_input_event(&mut self, data: &str) {
        let visible = self.viewable_rows();
        let key = printable_char(data);
        if matches_editor_key(data, EditorKey::Escape)
            || matches_editor_key(data, EditorKey::Ctrl('e'))
            || matches!(key.as_str(), "q" | "Q")
        {
            (self.on_close)();
            return;
        }
        if matches_editor_key(data, EditorKey::Up) || key == "k" {
            self.scroll_by(-1);
            return;
        }
        if matches_editor_key(data, EditorKey::Down) || key == "j" {
            self.scroll_by(1);
            return;
        }
        if matches_list_key(data, ListKey::PageUp)
            || key == " "
            || matches_editor_key(data, EditorKey::Ctrl('b'))
        {
            self.scroll_by(
                -isize::try_from(visible.saturating_sub(1).max(1)).unwrap_or(isize::MAX),
            );
            return;
        }
        if matches_list_key(data, ListKey::PageDown)
            || matches_editor_key(data, EditorKey::Ctrl('f'))
        {
            self.scroll_by(isize::try_from(visible.saturating_sub(1).max(1)).unwrap_or(isize::MAX));
            return;
        }
        if matches_editor_key(data, EditorKey::Home) || key == "g" {
            self.scroll_to(0);
            return;
        }
        if matches_editor_key(data, EditorKey::End) || key == "G" {
            self.scroll_to(self.max_scroll());
        }
    }

    /// Original: `ApprovalPreviewViewer.scrollBy()`.
    fn scroll_by(&mut self, delta: isize) {
        self.scroll_to(self.scroll_top.saturating_add_signed(delta));
    }

    /// Original: `ApprovalPreviewViewer.scrollTo()`.
    fn scroll_to(&mut self, target: usize) {
        self.scroll_top = target.min(self.max_scroll());
    }

    /// Original: `ApprovalPreviewViewer.maxScroll()`.
    fn max_scroll(&self) -> usize {
        self.body_lines.len().saturating_sub(self.viewable_rows())
    }

    /// Original: `ApprovalPreviewViewer.viewableRows()`.
    fn viewable_rows(&self) -> usize {
        (self.rows)().saturating_sub(4).max(1)
    }

    /// Original: `ApprovalPreviewViewer.render()`.
    pub fn render_viewer(&mut self, width: usize) -> Vec<String> {
        let rows = (self.rows)().max(3);
        let body_height = rows.saturating_sub(2);
        let header = self.render_header(width);
        let body = self.render_body(width, body_height);
        let footer = self.render_footer(width, body_height);
        std::iter::once(header)
            .chain(body)
            .chain(std::iter::once(footer))
            .collect()
    }

    /// Original: `ApprovalPreviewViewer.renderHeader()`.
    fn render_header(&self, width: usize) -> String {
        let title = current_theme().bold_fg(ColorToken::Primary, " Preview ");
        fit_exactly(&format!("{title}{}", self.header_title), width)
    }

    /// Original: `ApprovalPreviewViewer.renderBody()`.
    fn render_body(&mut self, width: usize, body_height: usize) -> Vec<String> {
        let inner_width = width.saturating_sub(4).max(1);
        self.scroll_top = self.scroll_top.min(self.max_scroll());
        let view_rows = body_height.saturating_sub(2);
        let theme = current_theme();
        let top = theme.fg(
            ColorToken::Primary,
            &format!("╭{}╮", "─".repeat(width.saturating_sub(2))),
        );
        let bottom = theme.fg(
            ColorToken::Primary,
            &format!("╰{}╯", "─".repeat(width.saturating_sub(2))),
        );
        let mut output = vec![top];
        for index in 0..view_rows {
            let raw = self
                .body_lines
                .get(self.scroll_top + index)
                .map_or("", String::as_str);
            output.push(format!(
                "{}{}{}",
                theme.fg(ColorToken::Primary, "│"),
                fit_exactly(raw, inner_width),
                theme.fg(ColorToken::Primary, " │")
            ));
        }
        output.push(bottom);
        output
    }

    /// Original: `ApprovalPreviewViewer.renderFooter()`.
    fn render_footer(&self, width: usize, body_height: usize) -> String {
        let theme = current_theme();
        let total = self.body_lines.len();
        let view_rows = body_height.saturating_sub(2).max(1);
        let max_scroll = total.saturating_sub(view_rows);
        let percent = if max_scroll == 0 {
            100
        } else {
            ((self.scroll_top as f64 / max_scroll as f64) * 100.0).round() as usize
        };
        let line_from = if total == 0 { 0 } else { self.scroll_top + 1 };
        let line_to = total.min(self.scroll_top + view_rows);
        let position = theme.fg(
            ColorToken::TextMuted,
            &format!(" {line_from}-{line_to} / {total} ({percent}%) "),
        );
        let key = |text: &str| theme.bold_fg(ColorToken::Primary, text);
        let dim = |text: &str| theme.fg(ColorToken::TextMuted, text);
        let keys = format!(
            "{} {}  {} {}  {} {}  {} {}",
            key("↑↓"),
            dim("line"),
            key("PgUp/PgDn"),
            dim("page"),
            key("g/G"),
            dim("top/bot"),
            key("Q/Esc/Ctrl+E"),
            dim("cancel")
        );
        let left = format!(" {keys}");
        let left_width = visible_width(&left);
        let right_width = visible_width(&position);
        if left_width + 2 + right_width <= width {
            format!(
                "{left}{}{position}",
                " ".repeat(width - left_width - right_width)
            )
        } else {
            fit_exactly(&left, width)
        }
    }
}

impl Component for ApprovalPreviewViewer {
    fn render(&mut self, width: usize) -> Vec<String> {
        self.render_viewer(width)
    }

    fn handle_input(&mut self, data: &str) {
        self.handle_input_event(data);
    }

    fn invalidate(&mut self) {
        let built = build_body(&self.block);
        self.body_lines = built.lines;
        self.header_title = built.title;
    }

    fn role(&self) -> ComponentRole {
        ComponentRole::Other
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Original: `approval-preview.ts`, `padToWidth()`.
fn pad_to_width(line: &str, width: usize) -> String {
    let current_width = visible_width(line);
    if current_width == width {
        line.to_owned()
    } else if current_width > width {
        truncate_to_width(line, width, ELLIPSIS, false)
    } else {
        format!("{line}{}", " ".repeat(width - current_width))
    }
}

/// Original: `approval-preview.ts`, `fitExactly()`.
fn fit_exactly(line: &str, width: usize) -> String {
    let fitted = if visible_width(line) > width {
        truncate_to_width(line, width, ELLIPSIS, false)
    } else {
        line.to_owned()
    };
    pad_to_width(&fitted, width)
}

/// Original: `approval-preview.ts`, `buildBody()`.
fn build_body(block: &ApprovalPreviewBlock) -> BuiltBody {
    match block {
        ApprovalPreviewBlock::Diff(block) => build_diff_body(block),
        ApprovalPreviewBlock::FileContent(block) => build_file_content_body(block),
    }
}

/// Original: `approval-preview.ts`, `buildDiffBody()`.
fn build_diff_body(block: &DiffDisplayBlock) -> BuiltBody {
    let rendered = render_diff_lines_clustered(
        &block.old_text,
        &block.new_text,
        &block.path,
        &ClusteredDiffOptions {
            context_lines: Some(3),
            old_start: Some(block.old_start.unwrap_or(1)),
            new_start: Some(block.new_start.unwrap_or(1)),
            ..ClusteredDiffOptions::default()
        },
    );
    let mut lines = rendered.into_iter();
    BuiltBody {
        title: lines
            .next()
            .unwrap_or_default()
            .trim_start_matches(' ')
            .to_owned(),
        lines: lines.collect(),
    }
}

/// Original: `approval-preview.ts`, `buildFileContentBody()`.
fn build_file_content_body(block: &FileContentDisplayBlock) -> BuiltBody {
    let inferred;
    let language = if let Some(language) = block.language.as_deref() {
        Some(language)
    } else {
        inferred = lang_from_path(&block.path);
        inferred.as_deref()
    };
    let lines = highlight_lines(&block.content, language)
        .into_iter()
        .enumerate()
        .map(|(index, line)| {
            format!(
                "{}{}",
                current_theme().fg(ColorToken::DiffGutter, &format!("{:>4}  ", index + 1)),
                line
            )
        })
        .collect();
    BuiltBody {
        lines,
        title: current_theme().fg(ColorToken::TextStrong, &block.path),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    };

    use super::*;

    fn file_block(lines: usize) -> ApprovalPreviewBlock {
        ApprovalPreviewBlock::FileContent(FileContentDisplayBlock {
            path: "src/main.rs".to_owned(),
            content: (1..=lines)
                .map(|line| format!("let value_{line} = {line};"))
                .collect::<Vec<_>>()
                .join("\n"),
            language: Some("rust".to_owned()),
        })
    }

    fn plain(text: &str) -> String {
        let ansi = regex::Regex::new("\\x1b\\[[0-9;]*m").expect("ANSI regex");
        ansi.replace_all(text, "").into_owned()
    }

    #[test]
    fn file_body_is_numbered_and_snapshot_renders_exact_terminal_height() {
        let mut viewer = ApprovalPreviewViewer::new(file_block(20), || {}, || 10);
        assert_eq!(viewer.body_len(), 20);
        let rendered = viewer.render_viewer(60);
        assert_eq!(rendered.len(), 10);
        assert!(plain(&rendered[0]).contains("Preview src/main.rs"));
        assert!(plain(&rendered.join("\n")).contains("   1  let value_1"));
        assert!(rendered.iter().all(|line| visible_width(line) <= 60));
    }

    #[test]
    fn scroll_keys_clamp_page_and_follow_dynamic_terminal_rows() {
        let rows = Arc::new(AtomicUsize::new(9));
        let row_source = Arc::clone(&rows);
        let mut viewer = ApprovalPreviewViewer::new(
            file_block(30),
            || {},
            move || row_source.load(Ordering::Relaxed),
        );
        viewer.handle_input_event("G");
        assert_eq!(viewer.scroll_top(), 25);
        viewer.handle_input_event("k");
        assert_eq!(viewer.scroll_top(), 24);
        viewer.handle_input_event("g");
        viewer.handle_input_event("\u{1b}[6~");
        assert_eq!(viewer.scroll_top(), 4);
        viewer.handle_input_event(" ");
        assert_eq!(viewer.scroll_top(), 0);
        rows.store(14, Ordering::Relaxed);
        viewer.handle_input_event("G");
        assert_eq!(viewer.scroll_top(), 20);
    }

    #[test]
    fn every_close_key_dispatches_callback() {
        for key in ["\u{1b}", "\u{5}", "q", "Q"] {
            let closes = Arc::new(Mutex::new(0));
            let recorded = Arc::clone(&closes);
            let mut viewer = ApprovalPreviewViewer::new(
                file_block(1),
                move || *recorded.lock().expect("close count") += 1,
                || 10,
            );
            viewer.handle_input_event(key);
            assert_eq!(*closes.lock().expect("close count"), 1);
        }
    }

    #[test]
    fn diff_moves_cluster_header_into_viewer_chrome() {
        let block = ApprovalPreviewBlock::Diff(DiffDisplayBlock {
            path: "src/lib.rs".to_owned(),
            old_text: "one\ntwo\nthree".to_owned(),
            new_text: "one\nchanged\nthree".to_owned(),
            old_start: None,
            new_start: None,
            is_summary: None,
        });
        let viewer = ApprovalPreviewViewer::new(block, || {}, || 10);
        assert!(plain(&viewer.header_title).contains("+1 -1 src/lib.rs"));
        assert!(
            !viewer
                .body_lines
                .iter()
                .any(|line| plain(line).contains("src/lib.rs"))
        );
        assert!(
            viewer
                .body_lines
                .iter()
                .any(|line| plain(line).contains("changed"))
        );
    }

    #[test]
    fn invalidate_rebuilds_theme_styled_snapshot_and_footer_reports_position() {
        let mut viewer = ApprovalPreviewViewer::new(file_block(12), || {}, || 8);
        viewer.handle_input_event("G");
        let footer = plain(viewer.render_viewer(120).last().expect("footer"));
        assert!(footer.contains("9-12 / 12 (100%)"));
        viewer.invalidate();
        assert_eq!(viewer.body_len(), 12);
    }

    #[test]
    fn exact_fit_helpers_handle_wide_text_and_zero_width() {
        assert_eq!(visible_width(&fit_exactly("猫", 4)), 4);
        assert_eq!(visible_width(&fit_exactly("abcdef", 4)), 4);
        assert_eq!(fit_exactly("abc", 0), "");
    }
}
