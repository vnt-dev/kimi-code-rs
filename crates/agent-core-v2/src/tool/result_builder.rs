const DEFAULT_MAX_CHARS: usize = 50_000;
const DEFAULT_MAX_LINE_LENGTH: usize = 2000;
const TRUNCATION_MARKER: &str = "[...truncated]";
const TRUNCATION_MESSAGE: &str = "Output is truncated to fit in the message.";

#[derive(Clone, Copy, Debug, Default)]
pub struct ToolResultBuilderOptions {
    pub max_chars: Option<usize>,
    pub max_line_length: Option<Option<usize>>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ToolResultBuilderResult {
    pub is_error: bool,
    pub output: String,
    pub truncated: bool,
    pub brief: Option<String>,
}

pub struct ToolResultBuilder {
    max_chars: usize,
    max_line_length: Option<usize>,
    buffer: String,
    n_chars: usize,
    truncated: bool,
}

impl ToolResultBuilder {
    pub fn new(options: ToolResultBuilderOptions) -> Result<Self, &'static str> {
        let max_line_length = options
            .max_line_length
            .unwrap_or(Some(DEFAULT_MAX_LINE_LENGTH));
        if max_line_length.is_some_and(|limit| limit <= js_len(TRUNCATION_MARKER)) {
            return Err("maxLineLength must be greater than the truncation marker length.");
        }
        Ok(Self {
            max_chars: options.max_chars.unwrap_or(DEFAULT_MAX_CHARS),
            max_line_length,
            buffer: String::new(),
            n_chars: 0,
            truncated: false,
        })
    }

    pub fn n_chars(&self) -> usize {
        self.n_chars
    }

    pub fn truncated(&self) -> bool {
        self.truncated
    }

    // Original: ToolResultBuilder.write(); counts JavaScript UTF-16 code units.
    pub fn write(&mut self, text: &str) -> usize {
        if self.n_chars >= self.max_chars {
            if !text.is_empty() && !self.truncated {
                self.push_marker();
            }
            return 0;
        }
        let lines = split_lines(text);
        let mut written = 0;
        for original in lines {
            if self.n_chars >= self.max_chars {
                if !self.truncated {
                    self.push_marker();
                }
                break;
            }
            let remaining = self.max_chars - self.n_chars;
            let limit = self
                .max_line_length
                .map_or(remaining, |line_limit| remaining.min(line_limit));
            let mut line = original.to_owned();
            if js_len(&line) > limit {
                let break_start = line.trim_end_matches(['\r', '\n']).len();
                let line_break = &line[break_start..];
                let suffix = format!("{TRUNCATION_MARKER}{line_break}");
                let effective = limit.max(js_len(&suffix));
                line = slice_utf16(&line, effective - js_len(&suffix)) + &suffix;
                self.truncated = true;
            }
            let count = js_len(&line);
            self.buffer.push_str(&line);
            written += count;
            self.n_chars += count;
        }
        written
    }

    pub fn ok(&self, message: &str, brief: Option<String>) -> ToolResultBuilderResult {
        let mut message = message.to_owned();
        if !message.is_empty() && !message.ends_with('.') {
            message.push('.');
        }
        if self.truncated {
            message = if message.is_empty() {
                TRUNCATION_MESSAGE.into()
            } else {
                format!("{message} {TRUNCATION_MESSAGE}")
            };
        }
        let append = !message.is_empty() && (self.truncated || self.buffer.is_empty());
        ToolResultBuilderResult {
            is_error: false,
            output: if append {
                append_message(&self.buffer, &message)
            } else {
                self.buffer.clone()
            },
            truncated: self.truncated,
            brief,
        }
    }

    pub fn error(&self, message: &str, brief: Option<String>) -> ToolResultBuilderResult {
        let message = if self.truncated {
            if message.is_empty() {
                TRUNCATION_MESSAGE.into()
            } else {
                format!("{message} {TRUNCATION_MESSAGE}")
            }
        } else {
            message.to_owned()
        };
        ToolResultBuilderResult {
            is_error: true,
            output: append_message(&self.buffer, &message),
            truncated: self.truncated,
            brief,
        }
    }

    fn push_marker(&mut self) {
        self.buffer.push_str(TRUNCATION_MARKER);
        self.n_chars += js_len(TRUNCATION_MARKER);
        self.truncated = true;
    }
}

impl Default for ToolResultBuilder {
    fn default() -> Self {
        Self {
            max_chars: DEFAULT_MAX_CHARS,
            max_line_length: Some(DEFAULT_MAX_LINE_LENGTH),
            buffer: String::new(),
            n_chars: 0,
            truncated: false,
        }
    }
}

fn append_message(output: &str, message: &str) -> String {
    if message.is_empty() {
        output.to_owned()
    } else if output.is_empty() {
        message.to_owned()
    } else if output.ends_with('\n') {
        format!("{output}{message}")
    } else {
        format!("{output}\n{message}")
    }
}

fn split_lines(text: &str) -> Vec<&str> {
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

fn js_len(value: &str) -> usize {
    value.encode_utf16().count()
}

fn slice_utf16(value: &str, units: usize) -> String {
    String::from_utf16_lossy(&value.encode_utf16().take(units).collect::<Vec<_>>())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncates_lines_and_total_output_with_source_messages() {
        let mut builder = ToolResultBuilder::new(ToolResultBuilderOptions {
            max_chars: Some(20),
            max_line_length: Some(Some(16)),
        })
        .unwrap();
        assert_eq!(builder.write("12345678901234567890\n"), 16);
        assert!(builder.truncated());
        assert_eq!(builder.n_chars(), 16);
        let result = builder.ok("Done", Some("brief".into()));
        assert_eq!(
            result.output,
            "1[...truncated]\nDone. Output is truncated to fit in the message."
        );
        assert!(result.truncated);
    }

    #[test]
    fn appends_marker_once_after_total_limit_and_counts_utf16() {
        let mut builder = ToolResultBuilder::new(ToolResultBuilderOptions {
            max_chars: Some(2),
            max_line_length: Some(None),
        })
        .unwrap();
        assert_eq!(builder.write("😀"), 2);
        assert_eq!(builder.write("x"), 0);
        assert_eq!(builder.write("y"), 0);
        assert_eq!(builder.n_chars(), 2 + js_len(TRUNCATION_MARKER));
        assert_eq!(
            builder.error("failed", None).output,
            "😀[...truncated]\nfailed Output is truncated to fit in the message."
        );
    }

    #[test]
    fn success_message_is_only_appended_for_empty_or_truncated_output() {
        let mut builder = ToolResultBuilder::default();
        builder.write("content");
        assert_eq!(builder.ok("Done", None).output, "content");
        assert_eq!(
            ToolResultBuilder::default().ok("Done", None).output,
            "Done."
        );
    }
}
