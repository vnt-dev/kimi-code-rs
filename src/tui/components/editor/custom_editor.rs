use crate::tui::{
    components::render::visible_width,
    theme::{ColorToken, current_theme},
};

const CAPS_LOCK_BIT: u32 = 64;
const CTRL_BIT: u32 = 4;
const SHIFT_BIT: u32 = 1;
const EDITOR_LEFT_PADDING: usize = 4;
const CURSOR_BLOCK: &str = "\u{1b}[7m \u{1b}[0m";

/// Normalizes Kitty CSI-u Ctrl+letter events reported with Caps Lock.
///
/// Original: `custom-editor.ts`, `normalizeCapsLockedCtrl()`.
pub fn normalize_caps_locked_ctrl(data: &str) -> String {
    let Some(body) = data
        .strip_prefix("\u{1b}[")
        .and_then(|value| value.strip_suffix('u'))
    else {
        return data.to_owned();
    };
    let Some((codepoint_text, modifier_and_tail)) = body.split_once(';') else {
        return data.to_owned();
    };
    if codepoint_text.is_empty()
        || !codepoint_text.bytes().all(|byte| byte.is_ascii_digit())
        || modifier_and_tail.contains(';')
    {
        return data.to_owned();
    }
    let modifier_end = modifier_and_tail
        .find(':')
        .unwrap_or(modifier_and_tail.len());
    let modifier_text = &modifier_and_tail[..modifier_end];
    let tail = &modifier_and_tail[modifier_end..];
    if modifier_text.is_empty()
        || !modifier_text.bytes().all(|byte| byte.is_ascii_digit())
        || !valid_kitty_tail(tail)
    {
        return data.to_owned();
    }
    let (Ok(codepoint), Ok(modifier_plus_one)) =
        (codepoint_text.parse::<u32>(), modifier_text.parse::<u32>())
    else {
        return data.to_owned();
    };
    let Some(modifier) = modifier_plus_one.checked_sub(1) else {
        return data.to_owned();
    };
    if modifier & CAPS_LOCK_BIT == 0
        || modifier & CTRL_BIT == 0
        || modifier & SHIFT_BIT != 0
        || !(65..=90).contains(&codepoint)
    {
        return data.to_owned();
    }
    let lowered_codepoint = codepoint + 32;
    let stripped_modifier = (modifier & !CAPS_LOCK_BIT) + 1;
    format!("\u{1b}[{lowered_codepoint};{stripped_modifier}{tail}u")
}

fn valid_kitty_tail(tail: &str) -> bool {
    tail.is_empty()
        || (tail.starts_with(':')
            && tail
                .split(':')
                .skip(1)
                .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit())))
}

/// Highlights the leading slash command and the `/goal next manage` command
/// path while preserving existing ANSI escapes.
///
/// Original: `custom-editor.ts`, `highlightFirstSlashToken()`.
pub fn highlight_first_slash_token(line: &str) -> Option<String> {
    let visible = strip_sgr(line);
    let characters = visible.chars().collect::<Vec<_>>();
    let slash_index = characters.iter().position(|character| *character == '/')?;
    if characters[..slash_index]
        .iter()
        .any(|character| !matches!(character, ' ' | '\t'))
    {
        return None;
    }
    let mut end = slash_index + 1;
    while end < characters.len() && !is_token_space(characters[end]) {
        end += 1;
    }
    let visible_token = characters[slash_index..end].iter().collect::<String>();
    if visible_token[1..].contains('/') {
        return None;
    }
    let mut ranges = vec![(slash_index, end)];
    if visible_token == "/goal" {
        ranges.extend(goal_command_path_ranges(&characters, end));
    }
    Some(highlight_visible_ranges(line, &ranges))
}

fn goal_command_path_ranges(characters: &[char], command_end: usize) -> Vec<(usize, usize)> {
    let Some(next) = read_token_range(characters, command_end) else {
        return Vec::new();
    };
    if characters[next.0..next.1].iter().collect::<String>() != "next" {
        return Vec::new();
    }
    let mut ranges = vec![next];
    if let Some(manage) = read_token_range(characters, next.1)
        && characters[manage.0..manage.1].iter().collect::<String>() == "manage"
    {
        ranges.push(manage);
    }
    ranges
}

fn read_token_range(characters: &[char], start: usize) -> Option<(usize, usize)> {
    let mut token_start = start;
    while token_start < characters.len() && is_token_space(characters[token_start]) {
        token_start += 1;
    }
    if token_start >= characters.len() {
        return None;
    }
    let mut token_end = token_start;
    while token_end < characters.len() && !is_token_space(characters[token_end]) {
        token_end += 1;
    }
    Some((token_start, token_end))
}

fn is_token_space(character: char) -> bool {
    matches!(character, ' ' | '\t')
}

fn highlight_visible_ranges(line: &str, ranges: &[(usize, usize)]) -> String {
    let mut output = String::new();
    let mut raw_cursor = 0;
    for &(start, end) in ranges {
        let raw_start = map_visible_index_to_raw(line, start);
        let raw_end = map_visible_index_to_raw(line, end);
        output.push_str(&line[raw_cursor..raw_start]);
        output.push_str(&current_theme().bold_fg(ColorToken::Primary, &line[raw_start..raw_end]));
        raw_cursor = raw_end;
    }
    output.push_str(&line[raw_cursor..]);
    output
}

pub fn inject_argument_hint(
    line: &str,
    hint: &str,
    real_text_length: usize,
    width: usize,
) -> String {
    let cursor_index = line.find(CURSOR_BLOCK);
    let content_width = width
        .saturating_sub(EDITOR_LEFT_PADDING.saturating_mul(2))
        .max(1);
    let available = content_width
        .saturating_sub(real_text_length)
        .saturating_sub(usize::from(cursor_index.is_some()));
    let trimmed = truncate_hint(hint, available);
    if trimmed.is_empty() {
        return line.to_owned();
    }
    let insert_at = cursor_index.map_or_else(
        || map_visible_index_to_raw(line, EDITOR_LEFT_PADDING + real_text_length),
        |index| index + CURSOR_BLOCK.len(),
    );
    let trailing_width = visible_width(&line[insert_at..]);
    let remaining = trailing_width.saturating_sub(visible_width(&trimmed));
    format!(
        "{}{}{}",
        &line[..insert_at],
        current_theme().fg(ColorToken::TextDim, &trimmed),
        " ".repeat(remaining)
    )
}

fn truncate_hint(hint: &str, max_length: usize) -> String {
    if max_length == 0 {
        return String::new();
    }
    let characters = hint.chars().collect::<Vec<_>>();
    if characters.len() <= max_length {
        return hint.to_owned();
    }
    if max_length == 1 {
        return "…".to_owned();
    }
    format!(
        "{}…",
        characters[..max_length - 1].iter().collect::<String>()
    )
}

/// Overlays a prompt token in the editor's four-cell left padding.
pub fn inject_prompt_symbol(
    line: &str,
    symbol: &str,
    paint: Option<&dyn Fn(&str) -> String>,
) -> Option<String> {
    if line.len() < 4 || line.as_bytes()[..4].iter().any(|byte| *byte != b' ') {
        return None;
    }
    let rendered = paint.map_or_else(|| symbol.to_owned(), |paint| paint(symbol));
    Some(format!("  {rendered} {}", &line[4..]))
}

#[derive(Debug, Clone, Copy, Default)]
pub struct SideBorderOptions<'a> {
    pub connected_above: bool,
    pub label: Option<&'a str>,
}

/// Converts the base editor's horizontal separators and padded content into a
/// complete box while leaving inner ANSI styling intact.
pub fn wrap_with_side_borders(
    lines: &[String],
    paint: &dyn Fn(&str) -> String,
    options: SideBorderOptions<'_>,
) -> Vec<String> {
    let mut seen_top = false;
    lines
        .iter()
        .map(|line| {
            let plain = strip_sgr(line);
            if plain.starts_with('─') {
                let is_top = !seen_top;
                let (left_corner, right_corner) = if seen_top {
                    ('╰', '╯')
                } else if options.connected_above {
                    ('├', '┤')
                } else {
                    ('╭', '╮')
                };
                seen_top = true;
                let plain_chars = plain.chars().collect::<Vec<_>>();
                if plain_chars.len() == 1 {
                    return paint(&left_corner.to_string());
                }
                let middle = plain_chars[1..plain_chars.len() - 1]
                    .iter()
                    .collect::<String>();
                if is_top
                    && let Some(label) = options.label
                    && middle.chars().all(|character| character == '─')
                    && visible_width(label) <= middle.chars().count()
                {
                    return format!(
                        "{}{}{}{}",
                        paint(&left_corner.to_string()),
                        label,
                        paint(&"─".repeat(middle.chars().count() - visible_width(label))),
                        paint(&right_corner.to_string())
                    );
                }
                return paint(&format!("{left_corner}{middle}{right_corner}"));
            }
            if line.is_empty() {
                return String::new();
            }
            let first = line.chars().next().unwrap_or_default();
            let last = line.chars().next_back().unwrap_or_default();
            let first_end = first.len_utf8();
            let last_start = line.len() - last.len_utf8();
            let head = if first == ' ' {
                paint("│")
            } else {
                first.to_string()
            };
            if first_end >= line.len() {
                return head;
            }
            let tail = if last == ' ' {
                paint("│")
            } else {
                last.to_string()
            };
            format!("{head}{}{tail}", &line[first_end..last_start])
        })
        .collect()
}

fn strip_sgr(text: &str) -> String {
    let mut output = String::with_capacity(text.len());
    let mut index = 0;
    while index < text.len() {
        if let Some(end) = sgr_end(text, index) {
            index = end;
            continue;
        }
        let character = text[index..].chars().next().unwrap_or_default();
        output.push(character);
        index += character.len_utf8();
    }
    output
}

fn map_visible_index_to_raw(line: &str, visible_index: usize) -> usize {
    let mut visible_count = 0;
    let mut index = 0;
    while index < line.len() && visible_count < visible_index {
        if let Some(end) = sgr_end(line, index) {
            index = end;
            continue;
        }
        let character = line[index..].chars().next().unwrap_or_default();
        index += character.len_utf8();
        visible_count += 1;
    }
    index
}

fn sgr_end(text: &str, start: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    if bytes.get(start) != Some(&0x1b) || bytes.get(start + 1) != Some(&b'[') {
        return None;
    }
    let mut index = start + 2;
    while index < bytes.len() && (bytes[index].is_ascii_digit() || bytes[index] == b';') {
        index += 1;
    }
    (bytes.get(index) == Some(&b'm')).then_some(index + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_caps_locked_ctrl_kitty_sequences() {
        assert_eq!(normalize_caps_locked_ctrl("\u{1b}[68;69u"), "\u{1b}[100;5u");
        assert_eq!(normalize_caps_locked_ctrl("\u{1b}[67;69u"), "\u{1b}[99;5u");
        assert_eq!(
            normalize_caps_locked_ctrl("\u{1b}[68;69:3:1u"),
            "\u{1b}[100;5:3:1u"
        );
        assert_eq!(normalize_caps_locked_ctrl("\u{1b}[68;65u"), "\u{1b}[68;65u");
        assert_eq!(normalize_caps_locked_ctrl("\u{1b}[68;5u"), "\u{1b}[68;5u");
        assert_eq!(normalize_caps_locked_ctrl("\u{1b}[68;70u"), "\u{1b}[68;70u");
        assert_eq!(normalize_caps_locked_ctrl("\u{1b}[68;71u"), "\u{1b}[100;7u");
        for value in [
            "\u{1b}[49;69u",
            "\u{1b}[91;69u",
            "H",
            "\u{4}",
            "\u{1b}[A",
            "",
        ] {
            assert_eq!(normalize_caps_locked_ctrl(value), value);
        }
    }

    #[test]
    fn highlights_leading_slash_and_goal_path_without_losing_ansi() {
        let output = highlight_first_slash_token("  /help rest").expect("slash should highlight");
        assert_eq!(strip_sgr(&output), "  /help rest");
        assert!(output.contains("/help"));

        let line = "/goal next manage Ship\u{1b}[7m \u{1b}[0m";
        let output = highlight_first_slash_token(line).expect("goal path should highlight");
        assert_eq!(strip_sgr(&output), strip_sgr(line));
        assert!(output.matches("38;2").count() >= 3);
        assert!(output.contains("\u{1b}[7m"));
        assert!(highlight_first_slash_token("hello /not-cmd").is_none());
        assert!(highlight_first_slash_token("/user/desktop foo").is_none());
        assert!(highlight_first_slash_token("hello world").is_none());
    }

    #[test]
    fn injects_prompt_symbol_and_optional_paint_in_padding() {
        assert_eq!(
            inject_prompt_symbol("    hello world", ">", None).as_deref(),
            Some("  > hello world")
        );
        assert_eq!(
            inject_prompt_symbol("    hello", "!", Some(&|text| format!("<{text}>"))).as_deref(),
            Some("  <!> hello")
        );
        assert!(inject_prompt_symbol("   ", ">", None).is_none());
        assert!(inject_prompt_symbol("  x hello", ">", None).is_none());
    }

    #[test]
    fn injects_and_truncates_argument_hint_without_changing_width() {
        let line = "    /goal \u{1b}[7m \u{1b}[0m                    ";
        let output = inject_argument_hint(line, "[status|cancel]", 6, 32);
        assert!(strip_sgr(&output).contains("/goal  [status|cancel]"));
        assert_eq!(visible_width(&output), visible_width(line));

        let short = inject_argument_hint("    /g      ", "long hint", 2, 12);
        assert!(strip_sgr(&short).contains('…'));
        assert_eq!(visible_width(&short), 12);
        assert_eq!(inject_argument_hint("    full", "hint", 4, 8), "    full");
    }

    #[test]
    fn wraps_horizontal_and_content_rows_with_box_borders() {
        let lines = vec!["─".repeat(10), "   hi     ".to_owned(), "─".repeat(10)];
        let output = wrap_with_side_borders(&lines, &str::to_owned, SideBorderOptions::default());
        assert_eq!(output[0], "╭────────╮");
        assert_eq!(output[1], "│  hi    │");
        assert_eq!(output[2], "╰────────╯");

        let connected = wrap_with_side_borders(
            &lines,
            &str::to_owned,
            SideBorderOptions {
                connected_above: true,
                label: None,
            },
        );
        assert!(connected[0].starts_with('├'));
        assert!(connected[0].ends_with('┤'));
    }

    #[test]
    fn preserves_inner_styles_outer_content_and_paints_edges() {
        let paint = |text: &str| format!("<{text}>");
        let lines = vec![
            "─".repeat(5),
            "  x  ".to_owned(),
            "  abc".to_owned(),
            "─".repeat(5),
            "   item1  ".to_owned(),
        ];
        let output = wrap_with_side_borders(&lines, &paint, SideBorderOptions::default());
        assert_eq!(output[0], "<╭───╮>");
        assert_eq!(output[1], "<│> x <│>");
        assert_eq!(output[2], "<│> abc");
        assert_eq!(output[4], "<│>  item1 <│>");
    }

    #[test]
    fn overlays_label_only_on_plain_top_border_when_it_fits() {
        let border = "─".repeat(30);
        let lines = vec![border.clone(), "   x   ".to_owned(), border];
        let output = wrap_with_side_borders(
            &lines,
            &str::to_owned,
            SideBorderOptions {
                connected_above: false,
                label: Some(" ! shell mode "),
            },
        );
        assert!(output[0].starts_with("╭ ! shell mode "));
        assert_eq!(output[0].chars().count(), 30);
        assert!(!output[2].contains("shell mode"));

        let scroll = vec!["─── ↑ 5 more ────".to_owned()];
        let output = wrap_with_side_borders(
            &scroll,
            &str::to_owned,
            SideBorderOptions {
                connected_above: false,
                label: Some(" ! shell mode "),
            },
        );
        assert!(output[0].contains("↑ 5 more"));
        assert!(!output[0].contains("shell mode"));
    }
}
