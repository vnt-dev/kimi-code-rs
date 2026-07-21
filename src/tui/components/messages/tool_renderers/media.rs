use std::sync::LazyLock;

use regex::Regex;
use serde_json::Value;

use crate::tui::{
    components::Text,
    theme::current_theme,
    types::{ToolCallBlockData, ToolResultBlockData},
};

use super::{
    truncated::render_truncated,
    types::{RenderedComponents, RendererContext},
};

static PATH_TAG_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"^<(image|video)\s+path="([^"]+)">$"#).expect("media path tag regex must compile")
});
static DATA_URL_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?s)^data:([^;]+);base64,(.*)$").expect("media data URL regex must compile")
});

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaKind {
    Image,
    Video,
}

impl MediaKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Image => "image",
            Self::Video => "video",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadMediaSummary {
    pub kind: MediaKind,
    pub path: Option<String>,
    pub mime_type: Option<String>,
    pub bytes: Option<usize>,
    pub url: Option<String>,
}

fn bytes_from_base64(base64: &str) -> usize {
    if base64.is_empty() {
        return 0;
    }
    let padding = if base64.ends_with("==") {
        2
    } else if base64.ends_with('=') {
        1
    } else {
        0
    };
    (base64.len().saturating_mul(3) / 4).saturating_sub(padding)
}

// Original: tool-renderers/media.ts parseReadMediaOutput()
pub fn parse_read_media_output(output: &str) -> Option<ReadMediaSummary> {
    let parsed = serde_json::from_str::<Value>(output).ok()?;
    let parts = parsed.as_array()?;
    let mut kind = None;
    let mut path = None;
    let mut mime_type = None;
    let mut bytes = None;
    let mut url = None;
    let mut found_media = false;

    for raw in parts {
        let Some(part) = raw.as_object() else {
            continue;
        };
        let part_type = part.get("type").and_then(Value::as_str);
        if part_type == Some("text") {
            if let Some(text) = part.get("text").and_then(Value::as_str)
                && let Some(captures) = PATH_TAG_RE.captures(text)
            {
                kind = match captures.get(1).map(|value| value.as_str()) {
                    Some("image") => Some(MediaKind::Image),
                    Some("video") => Some(MediaKind::Video),
                    _ => kind,
                };
                path = captures.get(2).map(|value| value.as_str().to_owned());
            }
            continue;
        }
        let (media_kind, holder_key) = match part_type {
            Some("image_url") => (MediaKind::Image, "imageUrl"),
            Some("video_url") => (MediaKind::Video, "videoUrl"),
            _ => continue,
        };
        found_media = true;
        kind = Some(media_kind);
        let Some(media_url) = part
            .get(holder_key)
            .and_then(Value::as_object)
            .and_then(|holder| holder.get("url"))
            .and_then(Value::as_str)
        else {
            continue;
        };
        if let Some(captures) = DATA_URL_RE.captures(media_url) {
            mime_type = captures.get(1).map(|value| value.as_str().to_owned());
            bytes = captures
                .get(2)
                .map(|value| bytes_from_base64(value.as_str()));
        } else {
            url = Some(media_url.to_owned());
        }
    }

    if !found_media {
        return None;
    }
    Some(ReadMediaSummary {
        kind: kind?,
        path,
        mime_type,
        bytes,
        url,
    })
}

fn format_bytes(bytes: usize) -> String {
    if bytes < 1_024 {
        format!("{bytes} B")
    } else if bytes < 1_024 * 1_024 {
        format!("{:.1} KB", bytes as f64 / 1_024.0)
    } else {
        format!("{:.1} MB", bytes as f64 / 1_024.0 / 1_024.0)
    }
}

fn meta_segments(summary: &ReadMediaSummary) -> Vec<String> {
    let mut segments = Vec::new();
    if let Some(mime_type) = &summary.mime_type {
        segments.push(mime_type.clone());
    }
    if let Some(bytes) = summary.bytes {
        segments.push(format_bytes(bytes));
    }
    segments
}

pub fn read_media_chip(_tool_call: &ToolCallBlockData, result: &ToolResultBlockData) -> String {
    if result.is_error.unwrap_or(false) {
        return String::new();
    }
    let Some(summary) = parse_read_media_output(&result.output) else {
        return String::new();
    };
    let metadata = meta_segments(&summary);
    if metadata.is_empty() {
        if summary.url.is_some() {
            format!("{} · uploaded", summary.kind.as_str())
        } else {
            summary.kind.as_str().to_owned()
        }
    } else {
        format!("{} ({})", summary.kind.as_str(), metadata.join(", "))
    }
}

pub fn read_media_summary(
    tool_call: &ToolCallBlockData,
    result: &ToolResultBlockData,
    context: RendererContext,
) -> RenderedComponents {
    if result.is_error.unwrap_or(false) {
        return render_truncated(tool_call, result, context);
    }
    let Some(summary) = parse_read_media_output(&result.output) else {
        return render_truncated(tool_call, result, context);
    };
    if !context.expanded {
        return Vec::new();
    }
    let theme = current_theme();
    let mut output: RenderedComponents = Vec::new();
    if let Some(path) = &summary.path {
        output.push(Box::new(Text::new(format!("  {}", theme.dim(path)), 0, 0)));
    }
    let metadata = meta_segments(&summary);
    let mut tail = vec![summary.kind.as_str().to_owned()];
    if !metadata.is_empty() {
        tail.push(metadata.join(", "));
    }
    if let Some(url) = summary.url {
        tail.push(url);
    }
    output.push(Box::new(Text::new(
        format!("  {}", theme.dim(&tail.join(" · "))),
        0,
        0,
    )));
    output
}

#[cfg(test)]
mod tests {
    use serde_json::Map;

    use super::*;

    const PNG_BASE64: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNkYAAAAAYAAjCB0C8AAAAASUVORK5CYII=";

    fn call() -> ToolCallBlockData {
        ToolCallBlockData {
            id: "tc".to_owned(),
            name: "ReadMediaFile".to_owned(),
            args: Map::new(),
            description: None,
            streaming_arguments: None,
            streaming_started_at_ms: None,
            subagent: None,
            step: None,
            turn_id: None,
            truncated: None,
        }
    }

    fn result(output: &str, is_error: bool) -> ToolResultBlockData {
        ToolResultBlockData {
            tool_call_id: "tc".to_owned(),
            output: output.to_owned(),
            is_error: Some(is_error),
            synthetic: None,
        }
    }

    fn image_output(path: &str) -> String {
        serde_json::json!([
            {"type": "text", "text": format!(r#"<image path="{path}">"#)},
            {"type": "image_url", "imageUrl": {"url": format!("data:image/png;base64,{PNG_BASE64}")}},
            {"type": "text", "text": "</image>"}
        ])
        .to_string()
    }

    fn render(mut components: RenderedComponents) -> String {
        components
            .iter_mut()
            .flat_map(|component| component.render(100))
            .map(|line| strip_sgr(&line))
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn strip_sgr(text: &str) -> String {
        let mut output = String::new();
        let mut escape = false;
        for character in text.chars() {
            if character == '\u{1b}' {
                escape = true;
            } else if escape && character == 'm' {
                escape = false;
            } else if !escape {
                output.push(character);
            }
        }
        output
    }

    #[test]
    fn extracts_image_video_and_uploaded_url_metadata() {
        let image = parse_read_media_output(&image_output("/tmp/a.png")).expect("image");
        assert_eq!(image.kind, MediaKind::Image);
        assert_eq!(image.path.as_deref(), Some("/tmp/a.png"));
        assert_eq!(image.mime_type.as_deref(), Some("image/png"));
        assert!(image.bytes.is_some_and(|bytes| bytes > 0));

        let video = serde_json::json!([
            {"type": "text", "text": r#"<video path="/tmp/a.mp4">"#},
            {"type": "video_url", "videoUrl": {"url": "https://cdn.example/v/abc"}}
        ])
        .to_string();
        let video = parse_read_media_output(&video).expect("video");
        assert_eq!(video.kind, MediaKind::Video);
        assert_eq!(video.path.as_deref(), Some("/tmp/a.mp4"));
        assert_eq!(video.url.as_deref(), Some("https://cdn.example/v/abc"));
        assert_eq!(video.bytes, None);
    }

    #[test]
    fn rejects_non_json_and_envelopes_without_media() {
        assert!(parse_read_media_output("not json").is_none());
        assert!(parse_read_media_output(r#"[{"type":"text","text":"hi"}]"#).is_none());
    }

    #[test]
    fn chip_summarizes_media_and_suppresses_errors() {
        let call = call();
        let output = image_output("/tmp/a.png");
        let chip = read_media_chip(&call, &result(&output, false));
        assert!(chip.contains("image (image/png,"));
        assert!(chip.ends_with(" B)"));
        assert_eq!(read_media_chip(&call, &result("boom", true)), "");
        assert_eq!(read_media_chip(&call, &result("garbage", false)), "");
    }

    #[test]
    fn expanded_summary_never_renders_base64() {
        let call = call();
        let output = image_output("/tmp/a.png");
        assert!(
            render(read_media_summary(
                &call,
                &result(&output, false),
                RendererContext::default(),
            ))
            .trim()
            .is_empty()
        );
        let expanded = render(read_media_summary(
            &call,
            &result(&output, false),
            RendererContext { expanded: true },
        ));
        assert!(expanded.contains("/tmp/a.png"));
        assert!(expanded.contains("image/png"));
        assert!(!expanded.contains(PNG_BASE64));
    }

    #[test]
    fn errors_and_unexpected_output_fall_back_to_truncated_text() {
        let call = call();
        assert!(
            render(read_media_summary(
                &call,
                &result("File not found", true),
                RendererContext::default(),
            ))
            .contains("File not found")
        );
        assert!(
            render(read_media_summary(
                &call,
                &result(r#""some plain string output""#, false),
                RendererContext::default(),
            ))
            .contains("some plain string output")
        );
    }
}
