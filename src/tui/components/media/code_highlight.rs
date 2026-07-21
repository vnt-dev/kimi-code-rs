//! Shared syntax-highlighting helpers for code previews.

use std::path::Path;
use std::str::FromStr;
use std::sync::OnceLock;

use two_face::re_exports::syntect::easy::HighlightLines;
use two_face::re_exports::syntect::highlighting::{
    Color, FontStyle, ScopeSelectors, Style, StyleModifier, Theme, ThemeItem,
};
use two_face::re_exports::syntect::parsing::SyntaxSet;
use two_face::theme::EmbeddedThemeName;

static SYNTAX_SET: OnceLock<SyntaxSet> = OnceLock::new();
static THEME: OnceLock<Theme> = OnceLock::new();

const ANSI_COLORS: [(Color, u8); 8] = [
    (rgba(0, 0, 0), 30),
    (rgba(255, 0, 0), 31),
    (rgba(0, 128, 0), 32),
    (rgba(128, 128, 0), 33),
    (rgba(0, 0, 255), 34),
    (rgba(128, 0, 128), 35),
    (rgba(0, 128, 128), 36),
    (rgba(192, 192, 192), 37),
];

const fn rgba(r: u8, g: u8, b: u8) -> Color {
    Color { r, g, b, a: 255 }
}

fn syntax_set() -> &'static SyntaxSet {
    SYNTAX_SET.get_or_init(two_face::syntax::extra_no_newlines)
}

fn theme() -> &'static Theme {
    THEME.get_or_init(|| {
        let mut theme = two_face::theme::extra()
            .get(EmbeddedThemeName::Ansi)
            .clone();
        let plain = theme.settings.foreground.unwrap_or(Color::BLACK);
        theme.scopes.push(ThemeItem {
            // A two-scope selector outranks the ANSI theme's single-scope
            // string rules and recreates the original plain string/regexp
            // overrides without affecting green diff additions.
            scope: ScopeSelectors::from_str("source string, source string.regexp")
                .expect("static scope selectors are valid"),
            style: StyleModifier {
                foreground: Some(plain),
                background: None,
                font_style: Some(FontStyle::empty()),
            },
        });
        theme
    })
}

fn language_for_extension(extension: &str) -> &str {
    match extension {
        "ts" | "tsx" => "typescript",
        "js" | "jsx" => "javascript",
        "py" => "python",
        "rb" => "ruby",
        "rs" => "rust",
        "go" => "go",
        "java" => "java",
        "sh" | "bash" | "zsh" => "bash",
        "json" => "json",
        "yaml" | "yml" => "yaml",
        "toml" => "toml",
        "md" => "markdown",
        "css" => "css",
        "html" => "html",
        "sql" => "sql",
        "c" | "h" => "c",
        "cpp" | "hpp" => "cpp",
        other => other,
    }
}

fn find_language(
    language: &str,
) -> Option<&'static two_face::re_exports::syntect::parsing::SyntaxReference> {
    syntax_set().find_syntax_by_token(language)
}

/// Infers a supported highlighting language from a file path.
///
/// Original: `src/tui/components/media/code-highlight.ts`, `langFromPath()`.
pub fn lang_from_path(file_path: &str) -> Option<String> {
    let extension = Path::new(file_path)
        .extension()
        .and_then(|extension| extension.to_str())?
        .to_lowercase();
    if extension.is_empty() {
        return None;
    }

    let language = language_for_extension(&extension);
    find_language(language).map(|_| language.to_owned())
}

fn color_distance(left: Color, right: Color) -> u32 {
    let red = i32::from(left.r) - i32::from(right.r);
    let green = i32::from(left.g) - i32::from(right.g);
    let blue = i32::from(left.b) - i32::from(right.b);
    (red * red + green * green + blue * blue) as u32
}

fn ansi_code(color: Color) -> u8 {
    // two-face's ANSI theme stores the 16-color palette index in the red
    // channel and marks indexed colors with a zero alpha channel.
    if color.a == 0 && color.g == 0 && color.b == 0 {
        return match color.r {
            1 => 31,
            2 => 32,
            3 => 33,
            4 => 35,
            5 => 34,
            6 => 36,
            7 | 8 => 37,
            9..=16 => 81 + color.r,
            _ => 37,
        };
    }

    ANSI_COLORS
        .iter()
        .min_by_key(|(candidate, _)| color_distance(color, *candidate))
        .map_or(37, |(_, code)| *code)
}

fn render_fragment(style: Style, text: &str, default_foreground: Color) -> String {
    if style.foreground == default_foreground {
        return text.to_owned();
    }

    let code = ansi_code(style.foreground);
    // The original custom cli-highlight theme deliberately resets red string,
    // regexp, and diff-deletion tokens to plain terminal styling.
    if code == 31 {
        text.to_owned()
    } else {
        format!("\x1b[{code}m{text}\x1b[0m")
    }
}

/// Highlights code and returns one terminal-renderable string per input line.
///
/// Unsupported languages and parser errors silently fall back to plain text,
/// matching the original `highlightLines()` behavior.
///
/// Original: `src/tui/components/media/code-highlight.ts`, `highlightLines()`.
pub fn highlight_lines(code: &str, language: Option<&str>) -> Vec<String> {
    let plain_lines = || code.split('\n').map(str::to_owned).collect::<Vec<_>>();
    let Some(language) = language
        .map(str::trim)
        .filter(|language| !language.is_empty())
    else {
        return plain_lines();
    };
    let normalized_language = language.to_lowercase();
    let Some(syntax) = find_language(&normalized_language) else {
        return plain_lines();
    };

    let theme = theme();
    let default_foreground = theme.settings.foreground.unwrap_or(Color::BLACK);
    let mut highlighter = HighlightLines::new(syntax, theme);
    let mut highlighted = Vec::new();
    for line in code.split('\n') {
        let Ok(fragments) = highlighter.highlight_line(line, syntax_set()) else {
            return plain_lines();
        };
        highlighted.push(
            fragments
                .into_iter()
                .map(|(style, text)| render_fragment(style, text, default_foreground))
                .collect(),
        );
    }
    highlighted
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_known_file_extensions_to_supported_languages() {
        assert_eq!(lang_from_path("src/foo.ts").as_deref(), Some("typescript"));
        assert_eq!(lang_from_path("src/foo.TS").as_deref(), Some("typescript"));
        assert_eq!(lang_from_path("src/foo.hpp").as_deref(), Some("cpp"));
    }

    #[test]
    fn treats_unsupported_extensions_as_plain_text() {
        assert_eq!(lang_from_path("src/foo.abcxyz"), None);
        assert_eq!(lang_from_path("README"), None);
    }

    #[test]
    fn unsupported_languages_return_plain_lines() {
        assert_eq!(
            highlight_lines("hello\nworld", Some("abcxyz")),
            ["hello", "world"]
        );
        assert_eq!(highlight_lines("hello\n", None), ["hello", ""]);
    }

    #[test]
    fn omits_red_but_preserves_other_syntax_colors() {
        let javascript =
            highlight_lines("const s = 'str';\nconst r = /re+/g;", Some("javascript")).join("\n");
        assert!(!javascript.contains("\x1b[31m"));
        assert!(javascript.contains("\x1b[34m"), "{javascript:?}");

        let diff = highlight_lines("+ added\n- removed", Some("diff")).join("\n");
        assert!(!diff.contains("\x1b[31m"));
        assert!(diff.contains("\x1b[32m"), "{diff:?}");
    }

    #[test]
    fn normalizes_language_names() {
        assert_eq!(
            highlight_lines("const value = 1;", Some(" JavaScript ")),
            highlight_lines("const value = 1;", Some("javascript"))
        );
    }
}
