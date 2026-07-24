use std::any::Any;

use crate::tui::{
    components::{Component, ComponentRole, render::truncate_to_width},
    keys::{EditorKey, matches_editor_key},
    theme::{ColorToken, current_theme},
};

type CloseCallback = dyn FnMut() + Send;

/// Visible placeholder dialog for an operation whose v2 service is not ready.
///
/// This is intentionally a real interactive dialog rather than a silent
/// placeholder: Enter and Esc both close it, and focus remains in the dialog
/// until then.
pub struct MigrationNoticeDialog {
    title: String,
    message: String,
    on_close: Box<CloseCallback>,
}

impl MigrationNoticeDialog {
    pub fn new<C>(title: impl Into<String>, message: impl Into<String>, on_close: C) -> Self
    where
        C: FnMut() + Send + 'static,
    {
        Self {
            title: title.into(),
            message: message.into(),
            on_close: Box::new(on_close),
        }
    }
}

impl Component for MigrationNoticeDialog {
    fn render(&mut self, width: usize) -> Vec<String> {
        let width = width.max(1);
        [
            current_theme().fg(ColorToken::Primary, &"─".repeat(width)),
            current_theme().bold_fg(ColorToken::Primary, &format!(" {}", self.title)),
            current_theme().fg(ColorToken::TextMuted, " Enter confirm · Esc cancel"),
            String::new(),
            format!("  {}", current_theme().fg(ColorToken::Text, &self.message)),
            String::new(),
            current_theme().fg(ColorToken::Primary, &"─".repeat(width)),
        ]
        .into_iter()
        .map(|line| truncate_to_width(&line, width, "", false))
        .collect()
    }

    fn handle_input(&mut self, data: &str) {
        if matches_editor_key(data, EditorKey::Escape) || matches_editor_key(data, EditorKey::Enter)
        {
            (self.on_close)();
        }
    }

    fn invalidate(&mut self) {}

    fn role(&self) -> ComponentRole {
        ComponentRole::Other
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::tui::components::render::visible_width;

    #[test]
    fn renders_to_width_and_closes_with_enter_or_escape() {
        for close_key in ["\r", "\u{1b}"] {
            let closed = Arc::new(Mutex::new(0));
            let callback = Arc::clone(&closed);
            let mut dialog = MigrationNoticeDialog::new(
                "Model",
                "The v2 model catalog is not connected yet.",
                move || *callback.lock().expect("closed") += 1,
            );

            assert!(
                dialog
                    .render(36)
                    .iter()
                    .all(|line| visible_width(line) <= 36)
            );
            dialog.handle_input(close_key);
            assert_eq!(*closed.lock().expect("closed"), 1);
        }
    }
}
