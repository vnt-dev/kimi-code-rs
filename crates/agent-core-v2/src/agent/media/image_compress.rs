//! Image-ingestion format gate.
//!
//! Original: `packages/agent-core-v2/src/agent/media/image-compress.ts`,
//! `gateImageFormatParts()`. Compression and crop codecs are migrated in a
//! later unit; this pure gate intentionally precedes every codec path.

use std::{
    io::Cursor,
    sync::{Mutex, OnceLock},
};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use image::{
    DynamicImage, ImageFormat, ImageReader, codecs::jpeg::JpegEncoder, imageops::FilterType,
};

use crate::kosong::contract::message::{ContentPart, MediaUrl};

use super::{
    build_malformed_image_notice, build_unsupported_image_notice, decode_base64_prefix,
    is_data_url, is_model_accepted_image_mime, normalize_image_mime, parse_image_data_url,
    resolve_effective_image_mime, unsupported_image_mime_from_url,
};

pub const MAX_IMAGE_EDGE_PX: u32 = 2000;
pub const IMAGE_BYTE_BUDGET: usize = 3_932_160;
pub const READ_IMAGE_BYTE_BUDGET: usize = 262_144;
pub const MAX_IMAGE_DECODE_BYTES: usize = 67_108_864;
const MAX_DECODE_PIXELS: u64 = 100_000_000;

static CONFIGURED_MAX_EDGE: OnceLock<Mutex<Option<u32>>> = OnceLock::new();
static CONFIGURED_READ_BUDGET: OnceLock<Mutex<Option<usize>>> = OnceLock::new();

pub fn set_configured_max_image_edge_px(value: Option<f64>) {
    *CONFIGURED_MAX_EDGE
        .get_or_init(|| Mutex::new(None))
        .lock()
        .unwrap() = value
        .filter(|v| v.is_finite() && *v > 0.0 && v.fract() == 0.0)
        .map(|v| v as u32);
}
pub fn resolve_max_image_edge_px() -> u32 {
    CONFIGURED_MAX_EDGE
        .get_or_init(|| Mutex::new(None))
        .lock()
        .unwrap()
        .unwrap_or(MAX_IMAGE_EDGE_PX)
}
pub fn set_configured_read_image_byte_budget(value: Option<f64>) {
    *CONFIGURED_READ_BUDGET
        .get_or_init(|| Mutex::new(None))
        .lock()
        .unwrap() = value
        .filter(|v| v.is_finite() && *v > 0.0 && v.fract() == 0.0)
        .map(|v| v as usize);
}
pub fn resolve_read_image_byte_budget() -> usize {
    CONFIGURED_READ_BUDGET
        .get_or_init(|| Mutex::new(None))
        .lock()
        .unwrap()
        .unwrap_or(READ_IMAGE_BYTE_BUDGET)
}

#[derive(Clone, Debug, Default)]
pub struct CompressImageOptions {
    pub max_edge: Option<u32>,
    pub byte_budget: Option<usize>,
    pub max_decode_bytes: Option<usize>,
}
#[derive(Clone, Debug, PartialEq)]
pub struct CompressImageResult {
    pub data: Vec<u8>,
    pub mime_type: String,
    pub width: i64,
    pub height: i64,
    pub original_width: i64,
    pub original_height: i64,
    pub changed: bool,
    pub original_byte_length: usize,
    pub final_byte_length: usize,
}
#[derive(Clone, Debug, PartialEq)]
pub struct CompressBase64Result {
    pub base64: String,
    pub mime_type: String,
    pub width: i64,
    pub height: i64,
    pub original_width: i64,
    pub original_height: i64,
    pub changed: bool,
    pub original_byte_length: usize,
    pub final_byte_length: usize,
}

// Original: compressImageForModel(). Decode and encoding failures are deliberately best-effort passthroughs.
pub fn compress_image_for_model(
    bytes: &[u8],
    mime_type: &str,
    options: &CompressImageOptions,
) -> CompressImageResult {
    let normalized = normalize_image_mime(mime_type);
    let dims = super::sniff_image_dimensions(bytes);
    let passthrough = || CompressImageResult {
        data: bytes.into(),
        mime_type: mime_type.into(),
        width: dims.as_ref().map_or(0, |d| d.width),
        height: dims.as_ref().map_or(0, |d| d.height),
        original_width: dims.as_ref().map_or(0, |d| d.width),
        original_height: dims.as_ref().map_or(0, |d| d.height),
        changed: false,
        original_byte_length: bytes.len(),
        final_byte_length: bytes.len(),
    };
    if bytes.is_empty()
        || !matches!(
            normalized.as_str(),
            "image/png" | "image/jpeg" | "image/webp"
        )
        || is_animated_webp(bytes)
    {
        return passthrough();
    }
    let max_edge = options.max_edge.unwrap_or_else(resolve_max_image_edge_px);
    let budget = options.byte_budget.unwrap_or(IMAGE_BYTE_BUDGET);
    let max_decode = options.max_decode_bytes.unwrap_or(MAX_IMAGE_DECODE_BYTES);
    let longest = dims.as_ref().map_or(0, |d| d.width.max(d.height));
    if bytes.len() <= budget && (longest == 0 || longest <= max_edge as i64) {
        return passthrough();
    }
    if bytes.len() > max_decode
        || dims.as_ref().is_some_and(|d| {
            d.width <= 0
                || d.height <= 0
                || (d.width as u64).saturating_mul(d.height as u64) > MAX_DECODE_PIXELS
        })
    {
        return passthrough();
    }
    let Ok(image) =
        ImageReader::with_format(Cursor::new(bytes), format_for_mime(&normalized)).decode()
    else {
        return passthrough();
    };
    let (ow, oh) = (image.width(), image.height());
    let resized = fit_within_edge(image, max_edge);
    let candidates = encode_candidates(&resized, normalized == "image/jpeg");
    let Some((data, output_mime)) = candidates.into_iter().min_by_key(|(data, _)| data.len())
    else {
        return passthrough();
    };
    if data.len() >= bytes.len() && resized.width() == ow && resized.height() == oh {
        return passthrough();
    }
    CompressImageResult {
        final_byte_length: data.len(),
        data,
        mime_type: output_mime.into(),
        width: resized.width() as i64,
        height: resized.height() as i64,
        original_width: ow as i64,
        original_height: oh as i64,
        changed: true,
        original_byte_length: bytes.len(),
    }
}

// Original: compressBase64ForModel().
pub fn compress_base64_for_model(
    base64: &str,
    mime_type: &str,
    options: &CompressImageOptions,
) -> CompressBase64Result {
    let approx = base64.len().saturating_mul(3) / 4;
    let max = options.max_decode_bytes.unwrap_or(MAX_IMAGE_DECODE_BYTES);
    if approx > max {
        return CompressBase64Result {
            base64: base64.into(),
            mime_type: mime_type.into(),
            width: 0,
            height: 0,
            original_width: 0,
            original_height: 0,
            changed: false,
            original_byte_length: approx,
            final_byte_length: approx,
        };
    }
    let Ok(bytes) = STANDARD.decode(base64) else {
        return CompressBase64Result {
            base64: base64.into(),
            mime_type: mime_type.into(),
            width: 0,
            height: 0,
            original_width: 0,
            original_height: 0,
            changed: false,
            original_byte_length: 0,
            final_byte_length: 0,
        };
    };
    let result = compress_image_for_model(&bytes, mime_type, options);
    CompressBase64Result {
        base64: if result.changed {
            STANDARD.encode(&result.data)
        } else {
            base64.into()
        },
        mime_type: if result.changed {
            result.mime_type.clone()
        } else {
            mime_type.into()
        },
        width: result.width,
        height: result.height,
        original_width: result.original_width,
        original_height: result.original_height,
        changed: result.changed,
        original_byte_length: result.original_byte_length,
        final_byte_length: result.final_byte_length,
    }
}

fn format_for_mime(mime: &str) -> ImageFormat {
    match mime {
        "image/png" => ImageFormat::Png,
        "image/jpeg" => ImageFormat::Jpeg,
        _ => ImageFormat::WebP,
    }
}
fn fit_within_edge(image: DynamicImage, edge: u32) -> DynamicImage {
    let longest = image.width().max(image.height());
    if longest <= edge {
        image
    } else {
        image.resize(
            (image.width() as u64 * edge as u64 / longest as u64).max(1) as u32,
            (image.height() as u64 * edge as u64 / longest as u64).max(1) as u32,
            FilterType::Lanczos3,
        )
    }
}
fn encode_candidates(image: &DynamicImage, jpeg_only: bool) -> Vec<(Vec<u8>, &'static str)> {
    let mut out = Vec::new();
    if !jpeg_only {
        let mut png = Cursor::new(Vec::new());
        if image.write_to(&mut png, ImageFormat::Png).is_ok() {
            out.push((png.into_inner(), "image/png"));
        }
    }
    for quality in [80, 60, 40, 20] {
        let mut jpeg = Vec::new();
        if JpegEncoder::new_with_quality(&mut jpeg, quality)
            .encode_image(image)
            .is_ok()
        {
            out.push((jpeg, "image/jpeg"));
        }
    }
    out
}
fn is_animated_webp(bytes: &[u8]) -> bool {
    bytes
        .windows(4)
        .any(|chunk| chunk == b"ANIM" || chunk == b"ANMF")
}

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
    #[test]
    fn compresses_oversized_dimensions_and_leaves_small_images_unchanged() {
        let image = DynamicImage::new_rgb8(4, 2);
        let mut encoded = Cursor::new(Vec::new());
        image.write_to(&mut encoded, ImageFormat::Png).unwrap();
        let bytes = encoded.into_inner();
        let unchanged =
            compress_image_for_model(&bytes, "image/png", &CompressImageOptions::default());
        assert!(!unchanged.changed);
        let compressed = compress_image_for_model(
            &bytes,
            "image/png",
            &CompressImageOptions {
                max_edge: Some(2),
                ..Default::default()
            },
        );
        assert!(compressed.changed);
        assert_eq!((compressed.width, compressed.height), (2, 1));
        assert_eq!(compressed.original_byte_length, bytes.len());
    }
}
