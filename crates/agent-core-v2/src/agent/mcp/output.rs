//! MCP tool-result conversion and output limits.
//!
//! Original: `agent/mcp/output.ts`.

use std::{path::PathBuf, sync::Arc};

use futures_util::future::BoxFuture;

use crate::{
    agent::media::{
        CompressAnnotateOptions, CompressImageContentPartsOptions, ImageCompressionTelemetry,
        PersistOriginalImageOptions, build_unsupported_image_notice, compress_image_content_parts,
        is_model_accepted_image_mime, persist_original_image,
    },
    app::telemetry::TelemetryServiceHandle,
    kosong::contract::message::{ContentPart, MediaUrl},
    tool::ExecutableToolOutput,
};

use super::{McpContentBlock, McpToolResult};

pub const MCP_MAX_OUTPUT_CHARS: usize = 100_000;
const MCP_OUTPUT_TRUNCATED_TEXT: &str = "\n\n[Output truncated: exceeded 100000 character limit. Use pagination or more specific queries to get remaining content.]";
pub const MCP_MAX_BINARY_PART_BYTES: usize = 10 * 1024 * 1024;
const MCP_MAX_BINARY_PART_CHARS: usize = (MCP_MAX_BINARY_PART_BYTES * 4).div_ceil(3);

#[derive(Clone, Default)]
pub struct McpOutputOptions {
    pub originals_dir: Option<PathBuf>,
    pub telemetry: Option<TelemetryServiceHandle>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct McpExecutableOutput {
    pub output: ExecutableToolOutput,
    pub is_error: bool,
    pub note: Option<String>,
    pub truncated: Option<bool>,
}

// Original: convertMCPContentBlock().
pub fn convert_mcp_content_block(block: &McpContentBlock) -> Option<ContentPart> {
    match block.kind.as_str() {
        "text" => block
            .text
            .as_ref()
            .map(|text| ContentPart::Text { text: text.clone() }),
        "image" => block.data.as_ref().map(|data| ContentPart::ImageUrl {
            image_url: MediaUrl {
                url: format!(
                    "data:{};base64,{data}",
                    block.mime_type.as_deref().unwrap_or("image/png")
                ),
                id: None,
            },
        }),
        "audio" => block.data.as_ref().map(|data| ContentPart::AudioUrl {
            audio_url: MediaUrl {
                url: format!(
                    "data:{};base64,{data}",
                    block.mime_type.as_deref().unwrap_or("audio/mpeg")
                ),
                id: None,
            },
        }),
        "resource" => convert_embedded_resource(block),
        "resource_link" => convert_resource_link(block),
        _ => None,
    }
}

fn convert_embedded_resource(block: &McpContentBlock) -> Option<ContentPart> {
    let resource = block.resource.as_ref()?;
    if let Some(text) = &resource.text {
        return Some(ContentPart::Text { text: text.clone() });
    }
    let blob = resource.blob.as_ref()?;
    let mime_type = resource
        .mime_type
        .as_deref()
        .unwrap_or("application/octet-stream");
    media_part_for_blob(mime_type, blob)
}

fn convert_resource_link(block: &McpContentBlock) -> Option<ContentPart> {
    let uri = block.uri.as_ref()?;
    let mime_type = block
        .mime_type
        .as_deref()
        .unwrap_or("application/octet-stream");
    if mime_type.starts_with("image/") {
        if !is_model_accepted_image_mime(mime_type) {
            return Some(ContentPart::Text {
                text: build_unsupported_image_notice(mime_type, Some(uri)),
            });
        }
        return Some(ContentPart::ImageUrl {
            image_url: MediaUrl {
                url: uri.clone(),
                id: None,
            },
        });
    }
    if mime_type.starts_with("audio/") {
        return Some(ContentPart::AudioUrl {
            audio_url: MediaUrl {
                url: uri.clone(),
                id: None,
            },
        });
    }
    if mime_type.starts_with("video/") {
        return Some(ContentPart::VideoUrl {
            video_url: MediaUrl {
                url: uri.clone(),
                id: None,
            },
        });
    }
    None
}

fn media_part_for_blob(mime_type: &str, blob: &str) -> Option<ContentPart> {
    let url = format!("data:{mime_type};base64,{blob}");
    if mime_type.starts_with("image/") {
        Some(ContentPart::ImageUrl {
            image_url: MediaUrl { url, id: None },
        })
    } else if mime_type.starts_with("audio/") {
        Some(ContentPart::AudioUrl {
            audio_url: MediaUrl { url, id: None },
        })
    } else if mime_type.starts_with("video/") {
        Some(ContentPart::VideoUrl {
            video_url: MediaUrl { url, id: None },
        })
    } else {
        None
    }
}

// Original: mcpResultToExecutableOutput(). Filesystem persistence remains
// asynchronous; all in-memory shaping and limits remain synchronous.
pub async fn mcp_result_to_executable_output(
    result: &McpToolResult,
    qualified_tool_name: &str,
    options: &McpOutputOptions,
) -> McpExecutableOutput {
    let converted = result
        .content
        .iter()
        .filter_map(convert_mcp_content_block)
        .collect::<Vec<_>>();
    let wrapped = wrap_media_only(&converted, qualified_tool_name);
    let budgeted = apply_text_budget(&wrapped);
    let originals_dir = options.originals_dir.clone();
    let persist_original = Arc::new(move |bytes: Vec<u8>, mime_type: String| {
        let options = PersistOriginalImageOptions {
            dir: originals_dir.clone(),
            ..PersistOriginalImageOptions::default()
        };
        Box::pin(async move { persist_original_image(&bytes, &mime_type, &options).await })
            as BoxFuture<'static, Option<String>>
    });
    let compressed = compress_image_content_parts(
        &budgeted.parts,
        &CompressImageContentPartsOptions {
            compress: crate::agent::media::CompressImageOptions {
                telemetry: options
                    .telemetry
                    .clone()
                    .map(|client| ImageCompressionTelemetry {
                        client,
                        source: "mcp_tool_result".into(),
                    }),
                ..crate::agent::media::CompressImageOptions::default()
            },
            annotate: Some(CompressAnnotateOptions {
                persist_original: Some(persist_original),
            }),
        },
    )
    .await;
    let capped = apply_binary_part_cap(&compressed.parts);
    McpExecutableOutput {
        output: collapse_single_text(capped.parts),
        is_error: result.is_error,
        note: (!compressed.captions.is_empty()).then(|| compressed.captions.join("\n")),
        truncated: (budgeted.truncated || capped.truncated).then_some(true),
    }
}

fn wrap_media_only(parts: &[ContentPart], qualified_tool_name: &str) -> Vec<ContentPart> {
    let has_media = parts.iter().any(is_media_part);
    let has_non_empty_text = parts
        .iter()
        .any(|part| matches!(part, ContentPart::Text { text } if !text.is_empty()));
    if !has_media || has_non_empty_text {
        return parts.to_vec();
    }
    let mut output = Vec::with_capacity(parts.len() + 2);
    output.push(ContentPart::Text {
        text: format!("<mcp_tool_result name=\"{qualified_tool_name}\">"),
    });
    output.extend_from_slice(parts);
    output.push(ContentPart::Text {
        text: "</mcp_tool_result>".into(),
    });
    output
}

fn is_media_part(part: &ContentPart) -> bool {
    matches!(
        part,
        ContentPart::ImageUrl { .. } | ContentPart::AudioUrl { .. } | ContentPart::VideoUrl { .. }
    )
}

struct LimitedParts {
    parts: Vec<ContentPart>,
    truncated: bool,
}

fn apply_text_budget(parts: &[ContentPart]) -> LimitedParts {
    let mut remaining = MCP_MAX_OUTPUT_CHARS;
    let mut truncated = false;
    let mut output = Vec::with_capacity(parts.len());
    for part in parts {
        match part {
            ContentPart::Text { text } => {
                if remaining == 0 {
                    truncated = true;
                } else if text.len() > remaining {
                    output.push(ContentPart::Text {
                        text: truncate_at_char_boundary(text, remaining),
                    });
                    remaining = 0;
                    truncated = true;
                } else {
                    output.push(part.clone());
                    remaining -= text.len();
                }
            }
            ContentPart::Think { think, encrypted } => {
                let size = think.len() + encrypted.as_ref().map_or(0, String::len);
                if remaining == 0 {
                    truncated = true;
                } else if size > remaining {
                    output.push(ContentPart::Think {
                        think: truncate_at_char_boundary(think, remaining),
                        encrypted: None,
                    });
                    remaining = 0;
                    truncated = true;
                } else {
                    output.push(part.clone());
                    remaining -= size;
                }
            }
            _ => output.push(part.clone()),
        }
    }
    if truncated {
        append_truncation_notice(&mut output);
    }
    LimitedParts {
        parts: output,
        truncated,
    }
}

fn truncate_at_char_boundary(value: &str, max_bytes: usize) -> String {
    if max_bytes >= value.len() {
        return value.into();
    }
    let mut end = max_bytes;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].into()
}

struct CappedParts {
    parts: Vec<ContentPart>,
    truncated: bool,
}

fn apply_binary_part_cap(parts: &[ContentPart]) -> CappedParts {
    let mut output = Vec::with_capacity(parts.len());
    let mut truncated = false;
    for part in parts {
        let Some((kind, url)) = binary_part_url(part) else {
            output.push(part.clone());
            continue;
        };
        if url.len() > MCP_MAX_BINARY_PART_CHARS {
            output.push(ContentPart::Text {
                text: binary_part_too_large_notice(kind, url.len()),
            });
            truncated = true;
        } else {
            output.push(part.clone());
        }
    }
    CappedParts {
        parts: output,
        truncated,
    }
}

fn binary_part_url(part: &ContentPart) -> Option<(&'static str, &str)> {
    match part {
        ContentPart::ImageUrl { image_url } => Some(("image", &image_url.url)),
        ContentPart::AudioUrl { audio_url } => Some(("audio", &audio_url.url)),
        ContentPart::VideoUrl { video_url } => Some(("video", &video_url.url)),
        _ => None,
    }
}

fn binary_part_too_large_notice(kind: &str, url_length: usize) -> String {
    let approximate_mb = (url_length * 3) as f64 / 4.0 / (1024.0 * 1024.0);
    format!(
        "[{kind}_url dropped: ~{approximate_mb:.1} MB exceeds 10 MB per-part limit. Try a smaller resource.]"
    )
}

fn append_truncation_notice(parts: &mut Vec<ContentPart>) {
    for part in parts.iter_mut().rev() {
        if let ContentPart::Text { text } = part {
            text.push_str(MCP_OUTPUT_TRUNCATED_TEXT);
            return;
        }
    }
    parts.push(ContentPart::Text {
        text: MCP_OUTPUT_TRUNCATED_TEXT.into(),
    });
}

fn collapse_single_text(parts: Vec<ContentPart>) -> ExecutableToolOutput {
    match parts.as_slice() {
        [ContentPart::Text { text }] => ExecutableToolOutput::Text(text.clone()),
        _ => ExecutableToolOutput::Content(parts),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_supported_blocks_and_wraps_media_only_output() {
        let image = McpContentBlock {
            kind: "image".into(),
            data: Some("YWJj".into()),
            ..Default::default()
        };
        assert!(matches!(
            convert_mcp_content_block(&image),
            Some(ContentPart::ImageUrl { image_url }) if image_url.url == "data:image/png;base64,YWJj"
        ));
        let wrapped = wrap_media_only(
            &[convert_mcp_content_block(&image).unwrap()],
            "mcp__server__image",
        );
        assert!(
            matches!(&wrapped[0], ContentPart::Text { text } if text == "<mcp_tool_result name=\"mcp__server__image\">")
        );
        assert!(matches!(&wrapped[2], ContentPart::Text { text } if text == "</mcp_tool_result>"));
    }

    #[test]
    fn text_budget_and_binary_cap_preserve_source_notices() {
        let limited = apply_text_budget(&[ContentPart::Text {
            text: "x".repeat(MCP_MAX_OUTPUT_CHARS + 1),
        }]);
        assert!(limited.truncated);
        assert!(
            matches!(&limited.parts[0], ContentPart::Text { text } if text.ends_with(MCP_OUTPUT_TRUNCATED_TEXT))
        );
        let capped = apply_binary_part_cap(&[ContentPart::AudioUrl {
            audio_url: MediaUrl {
                url: "x".repeat(MCP_MAX_BINARY_PART_CHARS + 1),
                id: None,
            },
        }]);
        assert!(capped.truncated);
        assert!(
            matches!(&capped.parts[0], ContentPart::Text { text } if text == "[audio_url dropped: ~10.0 MB exceeds 10 MB per-part limit. Try a smaller resource.]")
        );
    }

    #[tokio::test]
    async fn carries_protocol_error_and_collapses_a_single_text_result() {
        let result = McpToolResult {
            content: vec![McpContentBlock {
                kind: "text".into(),
                text: Some("failed".into()),
                ..Default::default()
            }],
            is_error: true,
        };
        let output = mcp_result_to_executable_output(
            &result,
            "mcp__server__tool",
            &McpOutputOptions::default(),
        )
        .await;
        assert_eq!(output.output, ExecutableToolOutput::Text("failed".into()));
        assert!(output.is_error);
        assert_eq!(output.note, None);
        assert_eq!(output.truncated, None);
    }
}
