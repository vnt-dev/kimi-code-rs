/// Semantic colors consumed by TUI components.
///
/// Original:
///   apps/kimi-code/src/tui/theme/colors.ts
///   ColorPalette
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColorPalette {
    pub primary: String,
    pub accent: String,
    pub text: String,
    pub text_strong: String,
    pub text_dim: String,
    pub text_muted: String,
    pub border: String,
    pub border_focus: String,
    pub success: String,
    pub warning: String,
    pub error: String,
    pub diff_added: String,
    pub diff_removed: String,
    pub diff_added_strong: String,
    pub diff_removed_strong: String,
    pub diff_gutter: String,
    pub diff_meta: String,
    pub role_user: String,
    pub shell_mode: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolvedTheme {
    Dark,
    Light,
}

/// Original:
///   apps/kimi-code/src/tui/theme/colors.ts
///   getBuiltInPalette()
pub fn get_built_in_palette(theme: ResolvedTheme) -> ColorPalette {
    match theme {
        ResolvedTheme::Dark => dark_colors(),
        ResolvedTheme::Light => light_colors(),
    }
}

pub fn dark_colors() -> ColorPalette {
    ColorPalette {
        primary: "#4FA8FF".to_owned(),
        accent: "#5BC0BE".to_owned(),
        text: "#E0E0E0".to_owned(),
        text_strong: "#F5F5F5".to_owned(),
        text_dim: "#888888".to_owned(),
        text_muted: "#6B6B6B".to_owned(),
        border: "#5A5A5A".to_owned(),
        border_focus: "#E8A838".to_owned(),
        success: "#4EC87E".to_owned(),
        warning: "#E8A838".to_owned(),
        error: "#E85454".to_owned(),
        diff_added: "#4EC87E".to_owned(),
        diff_removed: "#E85454".to_owned(),
        diff_added_strong: "#7AD99B".to_owned(),
        diff_removed_strong: "#F08585".to_owned(),
        diff_gutter: "#6B6B6B".to_owned(),
        diff_meta: "#888888".to_owned(),
        role_user: "#FFCB6B".to_owned(),
        shell_mode: "#BD93F9".to_owned(),
    }
}

pub fn light_colors() -> ColorPalette {
    ColorPalette {
        primary: "#1565C0".to_owned(),
        accent: "#00838F".to_owned(),
        text: "#1A1A1A".to_owned(),
        text_strong: "#1A1A1A".to_owned(),
        text_dim: "#454545".to_owned(),
        text_muted: "#5F5F5F".to_owned(),
        border: "#737373".to_owned(),
        border_focus: "#92660A".to_owned(),
        success: "#0E7A38".to_owned(),
        warning: "#92660A".to_owned(),
        error: "#B91C1C".to_owned(),
        diff_added: "#0E7A38".to_owned(),
        diff_removed: "#B91C1C".to_owned(),
        diff_added_strong: "#0E7A38".to_owned(),
        diff_removed_strong: "#B91C1C".to_owned(),
        diff_gutter: "#737373".to_owned(),
        diff_meta: "#5F5F5F".to_owned(),
        role_user: "#9A4A00".to_owned(),
        shell_mode: "#7C3AED".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selects_the_original_built_in_palettes() {
        assert_eq!(get_built_in_palette(ResolvedTheme::Dark).primary, "#4FA8FF");
        assert_eq!(
            get_built_in_palette(ResolvedTheme::Light).primary,
            "#1565C0"
        );
    }
}
