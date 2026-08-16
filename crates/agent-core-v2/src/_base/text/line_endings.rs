use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LineEndingStyle {
    Lf,
    CrLf,
    Mixed,
}

/// Incremental line-ending flags, updated per chunk of text and classified at the end.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LineEndingFlags {
    pub has_crlf: bool,
    pub has_lf: bool,
    pub has_lone_cr: bool,
}

/// Records which line-ending kinds appear in `text` into `flags`.
pub fn update_line_ending_flags(flags: &mut LineEndingFlags, text: &str) {
    let bytes = text.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'\r' {
            if bytes.get(index + 1) == Some(&b'\n') {
                flags.has_crlf = true;
                index += 2;
            } else {
                flags.has_lone_cr = true;
                index += 1;
            }
        } else {
            if bytes[index] == b'\n' {
                flags.has_lf = true;
            }
            index += 1;
        }
    }
}

/// Classifies accumulated line-ending flags into a style.
pub fn classify_line_ending_style(flags: LineEndingFlags) -> LineEndingStyle {
    if flags.has_lone_cr || (flags.has_crlf && flags.has_lf) {
        LineEndingStyle::Mixed
    } else if flags.has_crlf {
        LineEndingStyle::CrLf
    } else {
        LineEndingStyle::Lf
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelTextView {
    pub text: String,
    pub line_ending_style: LineEndingStyle,
}

// Original:
//   packages/agent-core-v2/src/_base/text/line-endings.ts
//   detectLineEndingStyle()
pub fn detect_line_ending_style(text: &str) -> LineEndingStyle {
    let mut flags = LineEndingFlags::default();
    update_line_ending_flags(&mut flags, text);
    classify_line_ending_style(flags)
}

/// Splits `text` into lines, keeping each line terminator (`\n`, `\r` or `\r\n`)
/// attached to its line, mirroring JavaScript's `split(/\r\n|\r|\n/)` semantics.
pub fn split_lines_with_breaks(text: &str) -> Vec<&str> {
    let bytes = text.as_bytes();
    let mut lines = Vec::new();
    let mut start = 0;
    let mut index = 0;
    while index < bytes.len() {
        if matches!(bytes[index], b'\r' | b'\n') {
            index += 1;
            if bytes[index - 1] == b'\r' && bytes.get(index) == Some(&b'\n') {
                index += 1;
            }
            lines.push(&text[start..index]);
            start = index;
        } else {
            index += 1;
        }
    }
    if start < text.len() {
        lines.push(&text[start..]);
    }
    lines
}

// Original:
//   packages/agent-core-v2/src/_base/text/line-endings.ts
//   toModelTextView()
pub fn to_model_text_view(raw: &str) -> ModelTextView {
    let line_ending_style = detect_line_ending_style(raw);
    let text = if line_ending_style == LineEndingStyle::CrLf {
        raw.replace("\r\n", "\n")
    } else {
        raw.to_owned()
    };
    ModelTextView {
        text,
        line_ending_style,
    }
}

// Original:
//   packages/agent-core-v2/src/_base/text/line-endings.ts
//   materializeModelText()
pub fn materialize_model_text(text: &str, line_ending_style: LineEndingStyle) -> String {
    if line_ending_style != LineEndingStyle::CrLf {
        return text.to_owned();
    }
    text.replace("\r\n", "\n").replace('\n', "\r\n")
}

// Original:
//   packages/agent-core-v2/src/_base/text/line-endings.ts
//   makeCarriageReturnsVisible()
pub fn make_carriage_returns_visible(text: &str) -> String {
    text.replace('\r', "\\r")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_lf_crlf_and_mixed_inputs() {
        for text in ["", "no newline", "a\nb\n"] {
            assert_eq!(detect_line_ending_style(text), LineEndingStyle::Lf);
        }
        assert_eq!(
            detect_line_ending_style("a\r\nb\r\n"),
            LineEndingStyle::CrLf
        );
        for text in ["a\rb", "a\r\nb\n", "a\n\r\nb"] {
            assert_eq!(detect_line_ending_style(text), LineEndingStyle::Mixed);
        }
    }

    #[test]
    fn model_view_normalizes_only_uniform_crlf_text() {
        assert_eq!(
            to_model_text_view("a\r\nb\r\n"),
            ModelTextView {
                text: "a\nb\n".to_owned(),
                line_ending_style: LineEndingStyle::CrLf,
            }
        );
        assert_eq!(
            to_model_text_view("a\r\nb\n"),
            ModelTextView {
                text: "a\r\nb\n".to_owned(),
                line_ending_style: LineEndingStyle::Mixed,
            }
        );
    }

    #[test]
    fn materialization_is_crlf_idempotent_and_leaves_other_styles_unchanged() {
        assert_eq!(
            materialize_model_text("a\nb\r\nc", LineEndingStyle::CrLf),
            "a\r\nb\r\nc"
        );
        assert_eq!(materialize_model_text("a\nb", LineEndingStyle::Lf), "a\nb");
        assert_eq!(
            materialize_model_text("a\r\nb\n", LineEndingStyle::Mixed),
            "a\r\nb\n"
        );
    }

    #[test]
    fn carriage_returns_are_rendered_as_visible_escape_text() {
        assert_eq!(make_carriage_returns_visible("a\r\nb\rc"), "a\\r\nb\\rc");
    }

    #[test]
    fn model_view_serialization_preserves_typescript_field_and_enum_names() {
        assert_eq!(
            serde_json::to_value(to_model_text_view("a\r\n")).unwrap(),
            serde_json::json!({
                "text": "a\n",
                "lineEndingStyle": "crlf",
            })
        );
    }
}
