#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaUrlKind {
    Audio,
    Image,
    Video,
}

impl MediaUrlKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Audio => "audio",
            Self::Image => "image",
            Self::Video => "video",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataUrlSummary {
    pub mime: String,
    pub bytes: Option<usize>,
}

/// Original:
///   apps/kimi-code/src/tui/utils/media-url.ts
///   mediaUrlPartToText()
pub fn media_url_part_to_text(kind: MediaUrlKind, url: &str) -> String {
    if let Some(summary) = summarize_data_url(url) {
        let size = summary
            .bytes
            .map(|bytes| format!(", {}", format_byte_size(bytes)))
            .unwrap_or_default();
        return format!("[{} {}{size}]", kind.as_str(), summary.mime);
    }
    format!("<{} url=\"{}\">", kind.as_str(), escape_attribute(url))
}

/// Original:
///   apps/kimi-code/src/tui/utils/media-url.ts
///   summarizeDataUrl()
pub fn summarize_data_url(url: &str) -> Option<DataUrlSummary> {
    let content = url.strip_prefix("data:")?;
    let (header, data) = content.split_once(',').unwrap_or((content, ""));
    let mut header_parts = header.split(';');
    let raw_mime = header_parts.next().unwrap_or_default();
    let mime = if raw_mime.is_empty() {
        "application/octet-stream"
    } else {
        raw_mime
    };
    let base64 = header_parts.any(|parameter| parameter.eq_ignore_ascii_case("base64"));
    Some(DataUrlSummary {
        mime: mime.to_owned(),
        bytes: base64.then(|| estimate_base64_bytes(data)),
    })
}

fn estimate_base64_bytes(data: &str) -> usize {
    let compact = data
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect::<String>();
    if compact.is_empty() {
        return 0;
    }
    let padding = if compact.ends_with("==") {
        2
    } else if compact.ends_with('=') {
        1
    } else {
        0
    };
    compact
        .len()
        .saturating_mul(3)
        .checked_div(4)
        .unwrap_or_default()
        .saturating_sub(padding)
}

fn format_byte_size(bytes: usize) -> String {
    if bytes < 1_024 {
        return format!("{bytes} B");
    }
    let kibibytes = bytes as f64 / 1_024.0;
    if kibibytes < 1_024.0 {
        return format_one_decimal(kibibytes) + " KB";
    }
    format_one_decimal(kibibytes / 1_024.0) + " MB"
}

fn format_one_decimal(value: f64) -> String {
    if value >= 10.0 {
        format!("{value:.0}")
    } else {
        format!("{value:.1}")
    }
}

fn escape_attribute(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_non_data_urls_as_escaped_references() {
        assert_eq!(
            media_url_part_to_text(MediaUrlKind::Image, "file:///tmp/a&b\".png"),
            "<image url=\"file:///tmp/a&amp;b&quot;.png\">"
        );
    }

    #[test]
    fn summarizes_base64_payload_without_returning_it() {
        assert_eq!(
            media_url_part_to_text(MediaUrlKind::Image, "data:image/png;base64,qrs="),
            "[image image/png, 2 B]"
        );
        assert_eq!(
            media_url_part_to_text(
                MediaUrlKind::Video,
                &format!("data:video/mp4;base64,{}", "A".repeat(1_368))
            ),
            "[video video/mp4, 1.0 KB]"
        );
    }

    #[test]
    fn parses_mime_parameters_whitespace_and_padding() {
        assert_eq!(
            summarize_data_url("data:image/png;charset=utf-8;BASE64,AQID\nBA=="),
            Some(DataUrlSummary {
                mime: "image/png".to_owned(),
                bytes: Some(4),
            })
        );
        assert_eq!(
            summarize_data_url("data:,hello"),
            Some(DataUrlSummary {
                mime: "application/octet-stream".to_owned(),
                bytes: None,
            })
        );
    }

    #[test]
    fn rejects_regular_urls_and_handles_missing_commas() {
        assert_eq!(summarize_data_url("https://example.com/a.png"), None);
        assert_eq!(
            summarize_data_url("data:image/png;base64"),
            Some(DataUrlSummary {
                mime: "image/png".to_owned(),
                bytes: Some(0),
            })
        );
    }
}
