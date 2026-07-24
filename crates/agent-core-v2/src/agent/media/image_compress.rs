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
}
