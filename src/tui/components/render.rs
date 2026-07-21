use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

/// Original:
///   packages/pi-tui/src/utils.ts
///   visibleWidth()
pub fn visible_width(text: &str) -> usize {
    let clean = visible_text(text);
    UnicodeSegmentation::graphemes(clean.as_str(), true)
        .map(UnicodeWidthStr::width)
        .sum()
}

/// Original:
///   packages/pi-tui/src/utils.ts
///   truncateToWidth()
pub fn truncate_to_width(text: &str, max_width: usize, ellipsis: &str, pad: bool) -> String {
    if max_width == 0 {
        return String::new();
    }
    let text_width = visible_width(text);
    if text_width <= max_width {
        return if pad {
            format!("{text}{}", " ".repeat(max_width - text_width))
        } else {
            text.to_owned()
        };
    }

    let ellipsis_width = visible_width(ellipsis);
    if ellipsis_width >= max_width {
        let (clipped, width) = truncate_fragment(ellipsis, max_width);
        return if pad {
            format!("{clipped}{}", " ".repeat(max_width - width))
        } else {
            clipped
        };
    }

    let target = max_width - ellipsis_width;
    let (mut prefix, prefix_width) = truncate_ansi_prefix(text, target);
    prefix.push_str(ellipsis);
    if prefix.contains('\u{1b}') {
        prefix.push_str("\u{1b}[0m");
    }
    if pad {
        prefix.push_str(&" ".repeat(max_width - prefix_width - ellipsis_width));
    }
    prefix
}

fn visible_text(text: &str) -> String {
    let mut clean = String::with_capacity(text.len());
    let mut index = 0;
    while index < text.len() {
        if text.as_bytes()[index] == 0x1b {
            index = escape_end(text, index);
            continue;
        }
        let Some(character) = text[index..].chars().next() else {
            break;
        };
        index += character.len_utf8();
        match character {
            '\t' => clean.push_str("   "),
            value if value.is_control() => {}
            value => clean.push(value),
        }
    }
    clean
}

fn truncate_fragment(text: &str, width: usize) -> (String, usize) {
    let mut result = String::new();
    let mut used = 0;
    for grapheme in UnicodeSegmentation::graphemes(text, true) {
        let grapheme_width = UnicodeWidthStr::width(grapheme);
        if used + grapheme_width > width {
            break;
        }
        result.push_str(grapheme);
        used += grapheme_width;
    }
    (result, used)
}

fn truncate_ansi_prefix(text: &str, target_width: usize) -> (String, usize) {
    let mut result = String::new();
    let mut used = 0;
    let mut index = 0;
    while index < text.len() {
        if text.as_bytes()[index] == 0x1b {
            let end = escape_end(text, index);
            result.push_str(&text[index..end]);
            index = end;
            continue;
        }
        let remainder = &text[index..];
        let Some(grapheme) = UnicodeSegmentation::graphemes(remainder, true).next() else {
            break;
        };
        let rendered = if grapheme == "\t" { "   " } else { grapheme };
        let width = UnicodeWidthStr::width(rendered);
        if used + width > target_width {
            break;
        }
        if !grapheme.chars().all(char::is_control) || grapheme == "\t" {
            result.push_str(rendered);
        }
        used += width;
        index += grapheme.len();
    }
    (result, used)
}

fn escape_end(text: &str, start: usize) -> usize {
    let bytes = text.as_bytes();
    if start + 1 >= bytes.len() {
        return bytes.len();
    }
    match bytes[start + 1] {
        b'[' => {
            let mut index = start + 2;
            while index < bytes.len() {
                if (0x40..=0x7e).contains(&bytes[index]) {
                    return index + 1;
                }
                index += 1;
            }
            bytes.len()
        }
        b']' | b'_' | b'P' | b'^' => {
            let mut index = start + 2;
            while index < bytes.len() {
                if bytes[index] == 0x07 {
                    return index + 1;
                }
                if bytes[index] == 0x1b && index + 1 < bytes.len() && bytes[index + 1] == b'\\' {
                    return index + 2;
                }
                index += 1;
            }
            bytes.len()
        }
        _ => (start + 2).min(bytes.len()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn measures_unicode_tabs_and_terminal_control_sequences() {
        assert_eq!(visible_width("abc"), 3);
        assert_eq!(visible_width("a\tb"), 5);
        assert_eq!(visible_width("\u{1b}[31mred\u{1b}[0m"), 3);
        assert_eq!(
            visible_width("\u{1b}]8;;https://x\u{7}link\u{1b}]8;;\u{7}"),
            4
        );
        assert_eq!(visible_width("A\u{301}"), 1);
        assert_eq!(visible_width("\u{1f63a}"), 2);
    }

    #[test]
    fn truncates_plain_wide_and_styled_text() {
        assert_eq!(truncate_to_width("abcdef", 4, "…", false), "abc…");
        assert_eq!(
            truncate_to_width("a\u{1f63a}bc", 4, "…", false),
            "a\u{1f63a}…"
        );
        let styled = truncate_to_width("\u{1b}[31mabcdef\u{1b}[39m", 4, "…", false);
        assert_eq!(visible_width(&styled), 4);
        assert!(styled.starts_with("\u{1b}[31mabc…"));
        assert!(styled.ends_with("\u{1b}[0m"));
    }

    #[test]
    fn handles_tiny_widths_and_optional_padding() {
        assert_eq!(truncate_to_width("abcdef", 1, "…", false), "…");
        assert_eq!(truncate_to_width("abcdef", 2, "...", false), "..");
        assert_eq!(truncate_to_width("a", 3, "…", true), "a  ");
        assert_eq!(truncate_to_width("", 3, "…", true), "   ");
        assert_eq!(truncate_to_width("abc", 0, "…", true), "");
    }
}
