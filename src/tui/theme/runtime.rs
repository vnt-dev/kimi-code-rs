use std::sync::{LazyLock, RwLock};

use super::colors::{ColorPalette, dark_colors};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorToken {
    Primary,
    Accent,
    Text,
    TextStrong,
    TextDim,
    TextMuted,
    Border,
    BorderFocus,
    Success,
    Warning,
    Error,
    DiffAdded,
    DiffRemoved,
    DiffAddedStrong,
    DiffRemovedStrong,
    DiffGutter,
    DiffMeta,
    RoleUser,
    ShellMode,
}

pub struct Theme {
    palette: RwLock<ColorPalette>,
}

impl Theme {
    pub fn new(palette: ColorPalette) -> Self {
        Self {
            palette: RwLock::new(palette),
        }
    }

    pub fn palette(&self) -> ColorPalette {
        match self.palette.read() {
            Ok(palette) => palette.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }

    pub fn set_palette(&self, palette: ColorPalette) {
        match self.palette.write() {
            Ok(mut current) => *current = palette,
            Err(poisoned) => *poisoned.into_inner() = palette,
        }
    }

    pub fn color(&self, token: ColorToken) -> String {
        let palette = self.palette();
        match token {
            ColorToken::Primary => palette.primary,
            ColorToken::Accent => palette.accent,
            ColorToken::Text => palette.text,
            ColorToken::TextStrong => palette.text_strong,
            ColorToken::TextDim => palette.text_dim,
            ColorToken::TextMuted => palette.text_muted,
            ColorToken::Border => palette.border,
            ColorToken::BorderFocus => palette.border_focus,
            ColorToken::Success => palette.success,
            ColorToken::Warning => palette.warning,
            ColorToken::Error => palette.error,
            ColorToken::DiffAdded => palette.diff_added,
            ColorToken::DiffRemoved => palette.diff_removed,
            ColorToken::DiffAddedStrong => palette.diff_added_strong,
            ColorToken::DiffRemovedStrong => palette.diff_removed_strong,
            ColorToken::DiffGutter => palette.diff_gutter,
            ColorToken::DiffMeta => palette.diff_meta,
            ColorToken::RoleUser => palette.role_user,
            ColorToken::ShellMode => palette.shell_mode,
        }
    }

    pub fn fg(&self, token: ColorToken, text: &str) -> String {
        style_hex(&self.color(token), text, None)
    }

    pub fn bold_fg(&self, token: ColorToken, text: &str) -> String {
        style_hex(&self.color(token), text, Some(("1", "22")))
    }

    pub fn dim_fg(&self, token: ColorToken, text: &str) -> String {
        style_hex(&self.color(token), text, Some(("2", "22")))
    }

    pub fn italic_fg(&self, token: ColorToken, text: &str) -> String {
        style_hex(&self.color(token), text, Some(("3", "23")))
    }

    pub fn underline_fg(&self, token: ColorToken, text: &str) -> String {
        style_hex(&self.color(token), text, Some(("4", "24")))
    }

    pub fn bold(&self, text: &str) -> String {
        format!("\u{1b}[1m{text}\u{1b}[22m")
    }
    pub fn dim(&self, text: &str) -> String {
        format!("\u{1b}[2m{text}\u{1b}[22m")
    }
    pub fn italic(&self, text: &str) -> String {
        format!("\u{1b}[3m{text}\u{1b}[23m")
    }
    pub fn underline(&self, text: &str) -> String {
        format!("\u{1b}[4m{text}\u{1b}[24m")
    }
}

pub fn current_theme() -> &'static Theme {
    static THEME: LazyLock<Theme> = LazyLock::new(|| Theme::new(dark_colors()));
    &THEME
}

fn style_hex(hex: &str, text: &str, modifier: Option<(&str, &str)>) -> String {
    let Some((red, green, blue)) = parse_hex(hex) else {
        return text.to_owned();
    };
    match modifier {
        Some((open, close)) => format!(
            "\u{1b}[38;2;{red};{green};{blue}m\u{1b}[{open}m{text}\u{1b}[{close}m\u{1b}[39m"
        ),
        None => format!("\u{1b}[38;2;{red};{green};{blue}m{text}\u{1b}[39m"),
    }
}

fn parse_hex(hex: &str) -> Option<(u8, u8, u8)> {
    let value = u32::from_str_radix(hex.strip_prefix('#')?, 16).ok()?;
    (hex.len() == 7).then_some((
        ((value >> 16) & 0xff) as u8,
        ((value >> 8) & 0xff) as u8,
        (value & 0xff) as u8,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::{components::render::visible_width, theme::colors::light_colors};

    #[test]
    fn styles_truecolor_text_without_changing_visible_width() {
        let theme = Theme::new(dark_colors());
        let styled = theme.bold_fg(ColorToken::Success, "done");
        assert!(styled.contains("38;2;78;200;126"));
        assert_eq!(visible_width(&styled), 4);
    }

    #[test]
    fn global_theme_switches_palette_in_place() {
        let theme = current_theme();
        let original = theme.palette();
        theme.set_palette(light_colors());
        assert_eq!(theme.color(ColorToken::Primary), "#1565C0");
        theme.set_palette(original);
    }
}
