//! Image-ingestion format gate.
//!
//! Original: `packages/agent-core-v2/src/agent/media/image-compress.ts`,
//! `gateImageFormatParts()`. Compression and crop codecs are migrated in a
//! later unit; this pure gate intentionally precedes every codec path.

use crate::kosong::contract::message::{ContentPart, MediaUrl};

use super::{
    build_malformed_image_notice, build_unsupported_image_notice, decode_base64_prefix,
    is_data_url, is_model_accepted_image_mime, normalize_image_mime, parse_image_data_url,
    resolve_effective_image_mime, unsupported_image_mime_from_url,
};

// Original: gateImageFormatParts().
pub fn gate_image_format_parts(parts: &[ContentPart]) -> Vec<ContentPart> {
    let mut output = Vec::with_capacity(parts.len());
    for part in parts {
        let ContentPart::ImageUrl { image_url } = part else {
            output.push(part.clone());
            continue;
        };
        let url = &image_url.url;
        let Some(parsed) = parse_image_data_url(url) else {
            if is_data_url(url) {
                output.push(ContentPart::Text {
                    text: build_malformed_image_notice(url),
                });
            } else if let Some(mime) = unsupported_image_mime_from_url(url) {
                output.push(ContentPart::Text {
                    text: build_unsupported_image_notice(&mime, Some(url)),
                });
            } else {
                output.push(part.clone());
            }
            continue;
        };
        let effective_mime =
            resolve_effective_image_mime(&parsed.mime_type, &decode_base64_prefix(&parsed.base64));
        if !is_model_accepted_image_mime(&effective_mime) {
            output.push(ContentPart::Text {
                text: build_unsupported_image_notice(&effective_mime, None),
            });
            continue;
        }
        let canonical_url = format!(
            "data:{};base64,{}",
            normalize_image_mime(&effective_mime),
            parsed.base64
        );
        if url != &canonical_url {
            output.push(ContentPart::ImageUrl {
                image_url: MediaUrl {
                    url: canonical_url,
                    id: image_url.id.clone(),
                },
            });
        } else {
            output.push(part.clone());
        }
    }
    output
}

#[derive(Clone, Debug, PartialEq)]
pub struct ImageVariantDescription {
    pub width: f64,
    pub height: f64,
    pub byte_length: f64,
    pub mime_type: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ImageCompressionCaptionInput {
    pub original: ImageVariantDescription,
    pub final_variant: ImageVariantDescription,
    pub original_path: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ImageCompressionCaptionExtraction {
    pub captions: Vec<String>,
    pub text: String,
}

// Original: buildImageCompressionCaption().
pub fn build_image_compression_caption(input: &ImageCompressionCaptionInput) -> String {
    let mut sentences = vec![
        format!(
            "Image compressed to fit model limits: original {} -> sent {}.",
            describe_image_variant(&input.original),
            describe_image_variant(&input.final_variant)
        ),
        "Fine detail may be lost.".into(),
    ];
    if let Some(path) = input
        .original_path
        .as_deref()
        .filter(|path| !path.is_empty())
    {
        sentences.push(format!("The uncompressed original is saved at \"{path}\"; if you need fine detail (e.g. small text), call ReadMediaFile on that path with the region parameter (original-pixel coordinates) to view a crop at full fidelity."));
    } else {
        sentences.push("The uncompressed original was not preserved.".into());
    }
    format!("<system>{}</system>", sentences.join(" "))
}

// Original: extractImageCompressionCaptions().
pub fn extract_image_compression_captions(text: &str) -> ImageCompressionCaptionExtraction {
    const OPENING: &str = "<system>Image compressed to fit model limits:";
    if !text.contains(OPENING) {
        return ImageCompressionCaptionExtraction {
            captions: Vec::new(),
            text: text.into(),
        };
    }
    let mut captions = Vec::new();
    let mut remainder = String::new();
    let mut cursor = text;
    while let Some(start) = cursor.find(OPENING) {
        remainder.push_str(&cursor[..start]);
        let body_start = start + "<system>".len();
        let Some(end) = cursor[body_start..].find("</system>") else {
            remainder.push_str(&cursor[start..]);
            return ImageCompressionCaptionExtraction {
                captions,
                text: remainder,
            };
        };
        let end = body_start + end;
        captions.push(cursor[body_start..end].into());
        cursor = &cursor[end + "</system>".len()..];
    }
    remainder.push_str(cursor);
    ImageCompressionCaptionExtraction {
        captions,
        text: remainder,
    }
}

// Original: formatByteSize().
pub fn format_byte_size(bytes: f64) -> String {
    if bytes < 1024.0 {
        return format!("{} B", js_number(bytes));
    }
    if bytes < 1024.0 * 1024.0 {
        return format!("{} KB", js_number((bytes / 1024.0).round()));
    }
    format!("{:.1} MB", bytes / (1024.0 * 1024.0))
}

fn describe_image_variant(variant: &ImageVariantDescription) -> String {
    let size = format!(
        "{} ({})",
        variant.mime_type,
        format_byte_size(variant.byte_length)
    );
    if variant.width > 0.0 && variant.height > 0.0 {
        format!(
            "{}x{} {size}",
            js_number(variant.width),
            js_number(variant.height)
        )
    } else {
        size
    }
}
fn js_number(value: f64) -> String {
    if value.fract() == 0.0 {
        format!("{value:.0}")
    } else {
        value.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn image(url: &str) -> ContentPart {
        ContentPart::ImageUrl {
            image_url: MediaUrl {
                url: url.into(),
                id: Some("id".into()),
            },
        }
    }
    #[test]
    fn gates_malformed_and_unsupported_images_and_canonicalizes_supported_data_urls() {
        let parts = gate_image_format_parts(&[
            image("data:image/png,invalid"),
            image("a.heic"),
            image("DATA:image/jpg;base64,aGVsbG8="),
            image("https://x.test/image.jpg"),
        ]);
        assert!(
            matches!(&parts[0], ContentPart::Text { text } if text.contains("not a valid data URL"))
        );
        assert!(matches!(&parts[1], ContentPart::Text { text } if text.contains("image/heic")));
        assert!(
            matches!(&parts[2], ContentPart::ImageUrl { image_url } if image_url.url == "data:image/jpeg;base64,aGVsbG8=" && image_url.id.as_deref() == Some("id"))
        );
        assert_eq!(parts[3], image("https://x.test/image.jpg"));
    }
    #[test]
    fn captions_round_trip_and_byte_sizes_match_source_formatting() {
        let caption = build_image_compression_caption(&ImageCompressionCaptionInput {
            original: ImageVariantDescription {
                width: 4000.0,
                height: 3000.0,
                byte_length: 3.75 * 1024.0 * 1024.0,
                mime_type: "image/png".into(),
            },
            final_variant: ImageVariantDescription {
                width: 2000.0,
                height: 1500.0,
                byte_length: 1280.0,
                mime_type: "image/jpeg".into(),
            },
            original_path: Some("/tmp/original.png".into()),
        });
        let extracted = extract_image_compression_captions(&format!("before{caption}after"));
        assert_eq!(extracted.text, "beforeafter");
        assert_eq!(extracted.captions.len(), 1);
        assert!(extracted.captions[0].contains("4000x3000 image/png (3.8 MB)"));
        assert_eq!(format_byte_size(1023.0), "1023 B");
        assert_eq!(format_byte_size(1536.0), "2 KB");
    }
}
