use unicode_segmentation::UnicodeSegmentation;

use super::visible_width;

/// Original:
///   packages/pi-tui/src/utils.ts
///   wrapTextWithAnsi()
///
/// Wraps at spaces and CJK grapheme boundaries while carrying active SGR and
/// OSC 8 hyperlink state across physical terminal lines.
pub fn wrap_text_with_ansi(text: &str, width: usize) -> Vec<String> {
    if text.is_empty() {
        return vec![String::new()];
    }

    let width = width.max(1);
    let mut result = Vec::new();
    let mut tracker = AnsiCodeTracker::default();
    for input_line in text.split('\n') {
        let prefix = (!result.is_empty()).then(|| tracker.active_codes());
        let line = format!("{}{input_line}", prefix.unwrap_or_default());
        result.extend(wrap_single_line(&line, width));
        update_tracker(input_line, &mut tracker);
    }
    if result.is_empty() {
        vec![String::new()]
    } else {
        result
    }
}

fn wrap_single_line(line: &str, width: usize) -> Vec<String> {
    if line.is_empty() {
        return vec![String::new()];
    }
    if visible_width(line) <= width {
        return vec![line.to_owned()];
    }

    let mut wrapped = Vec::new();
    let mut tracker = AnsiCodeTracker::default();
    let mut current_line = String::new();
    let mut current_width = 0;

    for token in split_ansi_tokens(line) {
        let token_width = visible_width(&token);
        let is_whitespace = visible_text(&token).trim().is_empty();

        if token_width > width && !is_whitespace {
            if current_width > 0 {
                current_line.push_str(&tracker.line_end_reset());
                wrapped.push(current_line);
            }

            let broken = break_long_word(&token, width, &mut tracker);
            if let Some((last, previous)) = broken.split_last() {
                wrapped.extend(previous.iter().cloned());
                current_line = last.clone();
                current_width = visible_width(last);
            } else {
                current_line = String::new();
                current_width = 0;
            }
            continue;
        }

        if current_width + token_width > width && current_width > 0 {
            current_line = current_line.trim_end().to_owned();
            current_line.push_str(&tracker.line_end_reset());
            wrapped.push(current_line);
            if is_whitespace {
                current_line = tracker.active_codes();
                current_width = 0;
            } else {
                current_line = tracker.active_codes();
                current_line.push_str(&token);
                current_width = token_width;
            }
        } else {
            current_line.push_str(&token);
            current_width += token_width;
        }
        update_tracker(&token, &mut tracker);
    }

    if !current_line.is_empty() {
        wrapped.push(current_line);
    }
    if wrapped.is_empty() {
        vec![String::new()]
    } else {
        wrapped
            .into_iter()
            .map(|line| line.trim_end().to_owned())
            .collect()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OscTerminator {
    Bell,
    StringTerminator,
}

impl OscTerminator {
    fn as_str(self) -> &'static str {
        match self {
            Self::Bell => "\u{7}",
            Self::StringTerminator => "\u{1b}\\",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ActiveHyperlink {
    params: String,
    url: String,
    terminator: OscTerminator,
}

#[derive(Default)]
struct AnsiCodeTracker {
    bold: bool,
    dim: bool,
    italic: bool,
    underline: bool,
    blink: bool,
    inverse: bool,
    hidden: bool,
    strikethrough: bool,
    foreground: Option<String>,
    background: Option<String>,
    hyperlink: Option<ActiveHyperlink>,
}

impl AnsiCodeTracker {
    fn process(&mut self, code: &str) {
        if let Some(hyperlink) = parse_hyperlink(code) {
            self.hyperlink = hyperlink;
            return;
        }
        let Some(parameters) = code
            .strip_prefix("\u{1b}[")
            .and_then(|value| value.strip_suffix('m'))
        else {
            return;
        };
        if parameters.is_empty() || parameters == "0" {
            self.reset_sgr();
            return;
        }

        let parts: Vec<&str> = parameters.split(';').collect();
        let mut index = 0;
        while index < parts.len() {
            let Ok(value) = parts[index].parse::<u16>() else {
                index += 1;
                continue;
            };
            if matches!(value, 38 | 48) {
                let length = if parts.get(index + 1) == Some(&"5") && parts.get(index + 2).is_some()
                {
                    3
                } else if parts.get(index + 1) == Some(&"2") && parts.get(index + 4).is_some() {
                    5
                } else {
                    0
                };
                if length > 0 {
                    let color = parts[index..index + length].join(";");
                    if value == 38 {
                        self.foreground = Some(color);
                    } else {
                        self.background = Some(color);
                    }
                    index += length;
                    continue;
                }
            }
            match value {
                0 => self.reset_sgr(),
                1 => self.bold = true,
                2 => self.dim = true,
                3 => self.italic = true,
                4 => self.underline = true,
                5 => self.blink = true,
                7 => self.inverse = true,
                8 => self.hidden = true,
                9 => self.strikethrough = true,
                21 => self.bold = false,
                22 => {
                    self.bold = false;
                    self.dim = false;
                }
                23 => self.italic = false,
                24 => self.underline = false,
                25 => self.blink = false,
                27 => self.inverse = false,
                28 => self.hidden = false,
                29 => self.strikethrough = false,
                39 => self.foreground = None,
                49 => self.background = None,
                30..=37 | 90..=97 => self.foreground = Some(value.to_string()),
                40..=47 | 100..=107 => self.background = Some(value.to_string()),
                _ => {}
            }
            index += 1;
        }
    }

    fn reset_sgr(&mut self) {
        self.bold = false;
        self.dim = false;
        self.italic = false;
        self.underline = false;
        self.blink = false;
        self.inverse = false;
        self.hidden = false;
        self.strikethrough = false;
        self.foreground = None;
        self.background = None;
    }

    fn active_codes(&self) -> String {
        let mut codes = Vec::new();
        for (enabled, code) in [
            (self.bold, "1"),
            (self.dim, "2"),
            (self.italic, "3"),
            (self.underline, "4"),
            (self.blink, "5"),
            (self.inverse, "7"),
            (self.hidden, "8"),
            (self.strikethrough, "9"),
        ] {
            if enabled {
                codes.push(code.to_owned());
            }
        }
        if let Some(color) = &self.foreground {
            codes.push(color.clone());
        }
        if let Some(color) = &self.background {
            codes.push(color.clone());
        }
        let mut result = if codes.is_empty() {
            String::new()
        } else {
            format!("\u{1b}[{}m", codes.join(";"))
        };
        if let Some(hyperlink) = &self.hyperlink {
            result.push_str(&format_hyperlink(hyperlink));
        }
        result
    }

    fn line_end_reset(&self) -> String {
        let mut result = String::new();
        if self.underline {
            result.push_str("\u{1b}[24m");
        }
        if let Some(hyperlink) = &self.hyperlink {
            result.push_str("\u{1b}]8;;");
            result.push_str(hyperlink.terminator.as_str());
        }
        result
    }
}

fn parse_hyperlink(code: &str) -> Option<Option<ActiveHyperlink>> {
    let body = code.strip_prefix("\u{1b}]8;")?;
    let (body, terminator) = if let Some(body) = body.strip_suffix('\u{7}') {
        (body, OscTerminator::Bell)
    } else if let Some(body) = body.strip_suffix("\u{1b}\\") {
        (body, OscTerminator::StringTerminator)
    } else {
        return None;
    };
    let (params, url) = body.split_once(';')?;
    Some((!url.is_empty()).then(|| ActiveHyperlink {
        params: params.to_owned(),
        url: url.to_owned(),
        terminator,
    }))
}

fn format_hyperlink(hyperlink: &ActiveHyperlink) -> String {
    format!(
        "\u{1b}]8;{};{}{}",
        hyperlink.params,
        hyperlink.url,
        hyperlink.terminator.as_str()
    )
}

fn update_tracker(text: &str, tracker: &mut AnsiCodeTracker) {
    let mut index = 0;
    while index < text.len() {
        if let Some(end) = escape_sequence_end(text, index) {
            tracker.process(&text[index..end]);
            index = end;
        } else {
            index += text[index..].chars().next().map_or(1, char::len_utf8);
        }
    }
}

fn split_ansi_tokens(text: &str) -> Vec<String> {
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Kind {
        Space,
        Word,
    }

    fn flush(tokens: &mut Vec<String>, current: &mut String) {
        if !current.is_empty() {
            tokens.push(std::mem::take(current));
        }
    }

    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut pending_ansi = String::new();
    let mut current_kind = None;
    let mut index = 0;
    while index < text.len() {
        if let Some(end) = escape_sequence_end(text, index) {
            pending_ansi.push_str(&text[index..end]);
            index = end;
            continue;
        }
        let next_escape = text.as_bytes()[index..]
            .iter()
            .position(|byte| *byte == 0x1b)
            .map_or(text.len(), |offset| index + offset);
        for grapheme in UnicodeSegmentation::graphemes(&text[index..next_escape], true) {
            let kind = if grapheme == " " {
                Kind::Space
            } else {
                Kind::Word
            };
            if kind == Kind::Word && is_cjk_grapheme(grapheme) {
                flush(&mut tokens, &mut current);
                let mut token = std::mem::take(&mut pending_ansi);
                token.push_str(grapheme);
                tokens.push(token);
                current_kind = None;
                continue;
            }
            if !current.is_empty() && current_kind != Some(kind) {
                flush(&mut tokens, &mut current);
            }
            current.push_str(&std::mem::take(&mut pending_ansi));
            current.push_str(grapheme);
            current_kind = Some(kind);
        }
        index = next_escape;
    }
    if !pending_ansi.is_empty() {
        if !current.is_empty() {
            current.push_str(&pending_ansi);
        } else if let Some(last) = tokens.last_mut() {
            last.push_str(&pending_ansi);
        } else {
            current = pending_ansi;
        }
    }
    flush(&mut tokens, &mut current);
    tokens
}

fn break_long_word(word: &str, width: usize, tracker: &mut AnsiCodeTracker) -> Vec<String> {
    let mut lines = Vec::new();
    let mut current_line = tracker.active_codes();
    let mut current_width = 0;
    let mut index = 0;
    while index < word.len() {
        if let Some(end) = escape_sequence_end(word, index) {
            let code = &word[index..end];
            current_line.push_str(code);
            tracker.process(code);
            index = end;
            continue;
        }
        let next_escape = word.as_bytes()[index..]
            .iter()
            .position(|byte| *byte == 0x1b)
            .map_or(word.len(), |offset| index + offset);
        for grapheme in UnicodeSegmentation::graphemes(&word[index..next_escape], true) {
            let grapheme_width = visible_width(grapheme);
            if current_width + grapheme_width > width && current_width > 0 {
                current_line.push_str(&tracker.line_end_reset());
                lines.push(current_line);
                current_line = tracker.active_codes();
                current_width = 0;
            }
            current_line.push_str(grapheme);
            current_width += grapheme_width;
        }
        index = next_escape;
    }
    if !current_line.is_empty() {
        lines.push(current_line);
    }
    if lines.is_empty() {
        vec![String::new()]
    } else {
        lines
    }
}

fn escape_sequence_end(text: &str, index: usize) -> Option<usize> {
    if text.as_bytes().get(index) != Some(&0x1b) {
        return None;
    }
    let bytes = text.as_bytes();
    let next = *bytes.get(index + 1)?;
    match next {
        b'[' => {
            let mut end = index + 2;
            while end < bytes.len() {
                if (0x40..=0x7e).contains(&bytes[end]) {
                    return Some(end + 1);
                }
                end += 1;
            }
            None
        }
        b']' | b'_' | b'P' | b'^' => {
            let mut end = index + 2;
            while end < bytes.len() {
                if bytes[end] == 0x07 {
                    return Some(end + 1);
                }
                if bytes[end] == 0x1b && bytes.get(end + 1) == Some(&b'\\') {
                    return Some(end + 2);
                }
                end += 1;
            }
            None
        }
        _ => Some((index + 2).min(bytes.len())),
    }
}

fn visible_text(text: &str) -> String {
    let mut visible = String::new();
    let mut index = 0;
    while index < text.len() {
        if let Some(end) = escape_sequence_end(text, index) {
            index = end;
            continue;
        }
        let Some(character) = text[index..].chars().next() else {
            break;
        };
        if !character.is_control() {
            visible.push(character);
        }
        index += character.len_utf8();
    }
    visible
}

fn is_cjk_grapheme(grapheme: &str) -> bool {
    grapheme.chars().next().is_some_and(|character| {
        matches!(character as u32,
            0x2e80..=0x2fff | 0x3005..=0x3007 | 0x3021..=0x3029 |
            0x3040..=0x30ff | 0x3100..=0x312f | 0x3130..=0x318f |
            0x31a0..=0x31bf | 0x31f0..=0x31ff | 0x3400..=0x4dbf |
            0x4e00..=0x9fff | 0xa960..=0xa97f | 0xac00..=0xd7af |
            0xf900..=0xfaff | 0x20000..=0x323af
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wraps_words_cjk_and_long_tokens_to_visible_width() {
        assert_eq!(
            wrap_text_with_ansi("hello world again", 10),
            ["hello", "world", "again"]
        );
        assert_eq!(wrap_text_with_ansi("ab中文cd", 4), ["ab中", "文cd"]);
        let long = wrap_text_with_ansi("abcdefgh", 3);
        assert_eq!(long, ["abc", "def", "gh"]);
        assert!(long.iter().all(|line| visible_width(line) <= 3));
    }

    #[test]
    fn preserves_sgr_state_across_wrapped_lines() {
        let wrapped = wrap_text_with_ansi("\u{1b}[31mhello world in red\u{1b}[39m", 8);
        assert_eq!(wrapped.len(), 3);
        assert!(
            wrapped
                .iter()
                .skip(1)
                .all(|line| line.starts_with("\u{1b}[31m"))
        );
        assert!(wrapped.iter().all(|line| visible_width(line) <= 8));
    }

    #[test]
    fn closes_and_reopens_osc_hyperlinks_with_the_original_terminator() {
        let url = "https://example.com";
        let open = format!("\u{1b}]8;;{url}\u{7}");
        let close = "\u{1b}]8;;\u{7}";
        let wrapped = wrap_text_with_ansi(&format!("{open}0123456789{close}"), 6);
        assert_eq!(wrapped.len(), 2);
        assert!(wrapped.iter().all(|line| line.starts_with(&open)));
        assert!(wrapped[0].ends_with(close));
        assert!(wrapped.iter().all(|line| visible_width(line) <= 6));
    }
}
