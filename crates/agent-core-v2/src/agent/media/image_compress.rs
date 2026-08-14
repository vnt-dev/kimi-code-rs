//! Image-ingestion format gate.
//!
//! Original: `packages/agent-core-v2/src/agent/media/image-compress.ts`.
//! The format gate, compression ladder, caption helpers, content-part
//! integration, and crop path are translated here.

use parking_lot::Mutex;
use std::sync::{Arc, OnceLock};
use std::{fmt, io::Cursor, time::Instant};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use futures_util::future::BoxFuture;
use image::{
    DynamicImage, ImageFormat, ImageReader, codecs::jpeg::JpegEncoder, imageops::FilterType,
};

use crate::{
    app::telemetry::{TelemetryProperties, TelemetryServiceHandle},
    kosong::contract::message::{ContentPart, MediaUrl},
};

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
const JPEG_QUALITY_STEPS: [u8; 4] = [80, 60, 40, 20];
const FALLBACK_EDGES_PX: [u32; 6] = [2000, 1000, 768, 512, 384, 256];
const PNG_RESCALE_FLOOR_PX: u32 = 1000;

static CONFIGURED_MAX_EDGE: OnceLock<Mutex<Option<u32>>> = OnceLock::new();
static CONFIGURED_READ_BUDGET: OnceLock<Mutex<Option<usize>>> = OnceLock::new();

pub fn set_configured_max_image_edge_px(value: Option<u64>) {
    *CONFIGURED_MAX_EDGE.get_or_init(|| Mutex::new(None)).lock() = value
        .filter(|v| *v > 0)
        .map(|v| v.min(u32::MAX as u64) as u32);
}
pub fn resolve_max_image_edge_px() -> u32 {
    CONFIGURED_MAX_EDGE
        .get_or_init(|| Mutex::new(None))
        .lock()
        .unwrap_or(MAX_IMAGE_EDGE_PX)
}
pub fn set_configured_read_image_byte_budget(value: Option<u64>) {
    *CONFIGURED_READ_BUDGET
        .get_or_init(|| Mutex::new(None))
        .lock() = value
        .filter(|v| *v > 0)
        .map(|v| v.min(usize::MAX as u64) as usize);
}
pub fn resolve_read_image_byte_budget() -> usize {
    CONFIGURED_READ_BUDGET
        .get_or_init(|| Mutex::new(None))
        .lock()
        .unwrap_or(READ_IMAGE_BYTE_BUDGET)
}

#[derive(Clone)]
pub struct ImageCompressionTelemetry {
    pub client: TelemetryServiceHandle,
    pub source: String,
}

#[derive(Clone, Default)]
pub struct CompressImageOptions {
    pub max_edge: Option<u32>,
    pub byte_budget: Option<usize>,
    pub max_decode_bytes: Option<usize>,
    pub telemetry: Option<ImageCompressionTelemetry>,
}

impl fmt::Debug for CompressImageOptions {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CompressImageOptions")
            .field("max_edge", &self.max_edge)
            .field("byte_budget", &self.byte_budget)
            .field("max_decode_bytes", &self.max_decode_bytes)
            .field(
                "telemetry",
                &self.telemetry.as_ref().map(|_| "<configured>"),
            )
            .finish()
    }
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
    let started_at = Instant::now();
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
    let finish = |outcome: &str, result: CompressImageResult| {
        report_compress_event(
            options.telemetry.as_ref(),
            outcome,
            started_at,
            &normalized,
            dims.as_ref()
                .is_some_and(|dimensions| dimensions.transposed),
            &result,
        );
        result
    };
    if bytes.is_empty()
        || !matches!(
            normalized.as_str(),
            "image/png" | "image/jpeg" | "image/webp"
        )
        || is_animated_webp(bytes)
    {
        return finish("passthrough_unsupported", passthrough());
    }
    let max_edge = options.max_edge.unwrap_or_else(resolve_max_image_edge_px);
    let budget = options.byte_budget.unwrap_or(IMAGE_BYTE_BUDGET);
    let max_decode = options.max_decode_bytes.unwrap_or(MAX_IMAGE_DECODE_BYTES);
    let longest = dims.as_ref().map_or(0, |d| d.width.max(d.height));
    if bytes.len() <= budget && (longest == 0 || longest <= max_edge as i64) {
        return finish("passthrough_fast", passthrough());
    }
    if bytes.len() > max_decode
        || dims
            .as_ref()
            .is_some_and(|d| (d.width as u64).saturating_mul(d.height as u64) > MAX_DECODE_PIXELS)
    {
        return finish("passthrough_guard", passthrough());
    }
    let Ok(image) =
        ImageReader::with_format(Cursor::new(bytes), format_for_mime(&normalized)).decode()
    else {
        return finish("passthrough_error", passthrough());
    };
    let (ow, oh) = (image.width(), image.height());
    let mut image = image;
    fit_within_edge(&mut image, max_edge);
    let Some(encoded) = encode_within_budget(
        image,
        normalized == "image/jpeg",
        budget,
        &FALLBACK_EDGES_PX,
    ) else {
        return finish("passthrough_error", passthrough());
    };
    if encoded.data.len() >= bytes.len() && encoded.width == ow && encoded.height == oh {
        return finish("passthrough_unhelpful", passthrough());
    }
    finish(
        "compressed",
        CompressImageResult {
            final_byte_length: encoded.data.len(),
            data: encoded.data,
            mime_type: encoded.mime_type.into(),
            width: encoded.width as i64,
            height: encoded.height as i64,
            original_width: ow as i64,
            original_height: oh as i64,
            changed: true,
            original_byte_length: bytes.len(),
        },
    )
}

fn report_compress_event(
    telemetry: Option<&ImageCompressionTelemetry>,
    outcome: &str,
    started_at: Instant,
    input_mime: &str,
    exif_transposed: bool,
    result: &CompressImageResult,
) {
    let Some(telemetry) = telemetry else {
        return;
    };
    let properties = TelemetryProperties::from([
        ("source".into(), Some(serde_json::json!(telemetry.source))),
        ("outcome".into(), Some(serde_json::json!(outcome))),
        ("input_mime".into(), Some(serde_json::json!(input_mime))),
        (
            "output_mime".into(),
            Some(serde_json::json!(normalize_image_mime(&result.mime_type))),
        ),
        (
            "original_bytes".into(),
            Some(serde_json::json!(result.original_byte_length)),
        ),
        (
            "final_bytes".into(),
            Some(serde_json::json!(result.final_byte_length)),
        ),
        (
            "original_width".into(),
            Some(serde_json::json!(result.original_width)),
        ),
        (
            "original_height".into(),
            Some(serde_json::json!(result.original_height)),
        ),
        ("final_width".into(), Some(serde_json::json!(result.width))),
        (
            "final_height".into(),
            Some(serde_json::json!(result.height)),
        ),
        (
            "exif_transposed".into(),
            Some(serde_json::json!(exif_transposed)),
        ),
        (
            "duration_ms".into(),
            Some(serde_json::json!(started_at.elapsed().as_millis())),
        ),
    ]);
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        telemetry.client.track("image_compress", Some(&properties));
    }));
}

// Original: compressBase64ForModel().
pub fn compress_base64_for_model(
    base64: &str,
    mime_type: &str,
    options: &CompressImageOptions,
) -> CompressBase64Result {
    let started_at = Instant::now();
    let approx = base64.len().saturating_mul(3) / 4;
    let max = options.max_decode_bytes.unwrap_or(MAX_IMAGE_DECODE_BYTES);
    if approx > max {
        let result = CompressBase64Result {
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
        report_compress_event(
            options.telemetry.as_ref(),
            "passthrough_guard",
            started_at,
            &normalize_image_mime(mime_type),
            false,
            &compress_result_view(&result),
        );
        return result;
    }
    let Ok(bytes) = STANDARD.decode(base64) else {
        let result = CompressBase64Result {
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
        report_compress_event(
            options.telemetry.as_ref(),
            "passthrough_error",
            started_at,
            &normalize_image_mime(mime_type),
            false,
            &compress_result_view(&result),
        );
        return result;
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

fn compress_result_view(result: &CompressBase64Result) -> CompressImageResult {
    CompressImageResult {
        data: Vec::new(),
        mime_type: result.mime_type.clone(),
        width: result.width,
        height: result.height,
        original_width: result.original_width,
        original_height: result.original_height,
        changed: result.changed,
        original_byte_length: result.original_byte_length,
        final_byte_length: result.final_byte_length,
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ImageCropRegion {
    pub x: i64,
    pub y: i64,
    pub width: i64,
    pub height: i64,
}
#[derive(Clone, Debug, Default)]
pub struct CropImageOptions {
    pub compress: CompressImageOptions,
    pub skip_resize: bool,
}
#[derive(Clone, Debug, PartialEq)]
pub struct CropImageSuccess {
    pub data: Vec<u8>,
    pub mime_type: String,
    pub width: i64,
    pub height: i64,
    pub original_width: i64,
    pub original_height: i64,
    pub region: ImageCropRegion,
    pub resized: bool,
    pub original_byte_length: usize,
    pub final_byte_length: usize,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CropImageFailure {
    pub error: String,
}
#[derive(Clone, Debug, PartialEq)]
pub enum CropImageOutcome {
    Success(CropImageSuccess),
    Failure(CropImageFailure),
}

// Original: cropImageForModel(). The returned region is in original-pixel
// coordinates even when the encoded crop is downscaled.
pub fn crop_image_for_model(
    bytes: &[u8],
    mime_type: &str,
    region: ImageCropRegion,
    options: &CropImageOptions,
) -> CropImageOutcome {
    let started_at = Instant::now();
    let fail = |error_kind, error| {
        report_crop_event(
            options.compress.telemetry.as_ref(),
            false,
            Some(error_kind),
            started_at,
            None,
        );
        CropImageOutcome::Failure(CropImageFailure { error })
    };
    let normalized = normalize_image_mime(mime_type);
    if bytes.is_empty() {
        return fail("empty", "The image is empty.".into());
    }
    if !matches!(
        normalized.as_str(),
        "image/png" | "image/jpeg" | "image/webp"
    ) {
        return fail(
            "unsupported_format",
            format!("Cropping is only supported for PNG, JPEG, and WebP images; got {mime_type}."),
        );
    }
    if normalized == "image/webp" && is_animated_webp(bytes) {
        return fail(
            "unsupported_format",
            "Cropping is not supported for animated WebP images.".into(),
        );
    }
    if let Some(dimensions) = super::sniff_image_dimensions(bytes)
        && (dimensions.width as u64).saturating_mul(dimensions.height as u64) > MAX_DECODE_PIXELS
    {
        return fail(
            "too_large",
            format!(
                "The image ({}x{} pixels) is too large to decode for cropping.",
                dimensions.width, dimensions.height
            ),
        );
    }
    let max_decode = options
        .compress
        .max_decode_bytes
        .unwrap_or(MAX_IMAGE_DECODE_BYTES);
    if bytes.len() > max_decode {
        return fail(
            "too_large",
            "The image is too large to decode for cropping.".into(),
        );
    }
    let Ok(image) =
        ImageReader::with_format(Cursor::new(bytes), format_for_mime(&normalized)).decode()
    else {
        return fail(
            "decode_failed",
            "Failed to decode the image for cropping: unsupported or corrupt image data.".into(),
        );
    };
    let (original_width, original_height) = (image.width(), image.height());
    let (x, y) = (region.x, region.y);
    if x < 0
        || y < 0
        || x >= i64::from(original_width)
        || y >= i64::from(original_height)
        || region.width < 1
        || region.height < 1
    {
        return fail(
            "out_of_bounds",
            format!(
                "Region (x={}, y={}, width={}, height={}) lies outside the {}x{} image.",
                region.x, region.y, region.width, region.height, original_width, original_height
            ),
        );
    }
    let (x, y) = (x as u32, y as u32);
    let (w, h) = (
        (region.width as u32).min(original_width - x),
        (region.height as u32).min(original_height - y),
    );
    let applied = ImageCropRegion {
        x: i64::from(x),
        y: i64::from(y),
        width: i64::from(w),
        height: i64::from(h),
    };
    let cropped = image.crop_imm(x, y, w, h);
    let jpeg_only = normalized == "image/jpeg";
    let budget = options.compress.byte_budget.unwrap_or(IMAGE_BYTE_BUDGET);
    let encoded = if options.skip_resize {
        encode_skip_resize(&cropped, jpeg_only)
    } else {
        let mut resized = cropped;
        fit_within_edge(
            &mut resized,
            options
                .compress
                .max_edge
                .unwrap_or_else(resolve_max_image_edge_px),
        );
        encode_within_budget(resized, jpeg_only, budget, &FALLBACK_EDGES_PX).map(|encoded| {
            (
                encoded.data,
                encoded.mime_type,
                encoded.width,
                encoded.height,
            )
        })
    };
    let Some((data, output_mime, width, height)) = encoded else {
        return fail(
            "decode_failed",
            "Failed to decode the image for cropping: image encoding failed.".into(),
        );
    };
    if options.skip_resize && data.len() > budget {
        return fail(
            "budget",
            format!(
                "The cropped region encodes to {} bytes ({}), over the {}-byte ({}) per-image limit. Choose a smaller region, or allow downscaling.",
                data.len(),
                format_byte_size(data.len() as u64),
                budget,
                format_byte_size(budget as u64),
            ),
        );
    }
    let result = CropImageSuccess {
        final_byte_length: data.len(),
        data,
        mime_type: output_mime.into(),
        width: width as i64,
        height: height as i64,
        original_width: original_width as i64,
        original_height: original_height as i64,
        region: applied,
        resized: width != w || height != h,
        original_byte_length: bytes.len(),
    };
    report_crop_event(
        options.compress.telemetry.as_ref(),
        true,
        None,
        started_at,
        Some(&result),
    );
    CropImageOutcome::Success(result)
}

fn report_crop_event(
    telemetry: Option<&ImageCompressionTelemetry>,
    ok: bool,
    error_kind: Option<&str>,
    started_at: Instant,
    result: Option<&CropImageSuccess>,
) {
    let Some(telemetry) = telemetry else {
        return;
    };
    let original_pixels = result
        .map(|value| value.original_width.saturating_mul(value.original_height))
        .unwrap_or_default();
    let region_area_ratio = result.filter(|_| original_pixels != 0).map(|value| {
        value.region.width as f64 * value.region.height as f64 / original_pixels as f64
    });
    let properties = TelemetryProperties::from([
        ("source".into(), Some(serde_json::json!(telemetry.source))),
        ("ok".into(), Some(serde_json::json!(ok))),
        (
            "error_kind".into(),
            error_kind.map(|value| serde_json::json!(value)),
        ),
        (
            "resized".into(),
            result.map(|value| serde_json::json!(value.resized)),
        ),
        (
            "original_width".into(),
            result.map(|value| serde_json::json!(value.original_width)),
        ),
        (
            "original_height".into(),
            result.map(|value| serde_json::json!(value.original_height)),
        ),
        (
            "region_area_ratio".into(),
            region_area_ratio.map(|value| serde_json::json!(value)),
        ),
        (
            "final_bytes".into(),
            result.map(|value| serde_json::json!(value.final_byte_length)),
        ),
        (
            "duration_ms".into(),
            Some(serde_json::json!(started_at.elapsed().as_millis())),
        ),
    ]);
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        telemetry.client.track("image_crop", Some(&properties));
    }));
}

fn encode_skip_resize(
    image: &DynamicImage,
    jpeg_only: bool,
) -> Option<(Vec<u8>, &'static str, u32, u32)> {
    if jpeg_only {
        let mut data = Vec::new();
        JpegEncoder::new_with_quality(&mut data, 90)
            .encode_image(image)
            .ok()?;
        Some((data, "image/jpeg", image.width(), image.height()))
    } else {
        let mut data = Cursor::new(Vec::new());
        image.write_to(&mut data, ImageFormat::Png).ok()?;
        Some((
            data.into_inner(),
            "image/png",
            image.width(),
            image.height(),
        ))
    }
}

fn format_for_mime(mime: &str) -> ImageFormat {
    match mime {
        "image/png" => ImageFormat::Png,
        "image/jpeg" => ImageFormat::Jpeg,
        _ => ImageFormat::WebP,
    }
}
// Original: fitWithinEdge(). The source modifies the same Jimp image between
// ladder steps, so each fallback edge is relative to the preceding resize.
fn fit_within_edge(image: &mut DynamicImage, edge: u32) -> bool {
    let longest = image.width().max(image.height());
    if longest <= edge {
        return false;
    }
    let factor = edge as f64 / longest as f64;
    let width = (image.width() as f64 * factor).round().max(1.0) as u32;
    let height = (image.height() as f64 * factor).round().max(1.0) as u32;
    *image = image.resize(width, height, FilterType::Lanczos3);
    true
}

struct EncodedImage {
    data: Vec<u8>,
    mime_type: &'static str,
    width: u32,
    height: u32,
}

fn encode_png(image: &DynamicImage) -> Option<Vec<u8>> {
    let mut data = Cursor::new(Vec::new());
    image.write_to(&mut data, ImageFormat::Png).ok()?;
    Some(data.into_inner())
}

fn encode_jpeg(image: &DynamicImage, quality: u8) -> Option<Vec<u8>> {
    let mut jpeg = Vec::new();
    JpegEncoder::new_with_quality(&mut jpeg, quality)
        .encode_image(image)
        .ok()?;
    Some(jpeg)
}

fn consider_encoded(
    smallest: &mut Option<EncodedImage>,
    data: Vec<u8>,
    mime_type: &'static str,
    image: &DynamicImage,
) -> EncodedImage {
    let candidate = EncodedImage {
        data,
        mime_type,
        width: image.width(),
        height: image.height(),
    };
    if smallest
        .as_ref()
        .is_none_or(|current| candidate.data.len() < current.data.len())
    {
        *smallest = Some(EncodedImage {
            data: candidate.data.clone(),
            mime_type,
            width: candidate.width,
            height: candidate.height,
        });
    }
    candidate
}

fn jpeg_ladder(
    image: &DynamicImage,
    byte_budget: usize,
    smallest: &mut Option<EncodedImage>,
) -> Option<EncodedImage> {
    for quality in JPEG_QUALITY_STEPS {
        let jpeg = encode_jpeg(image, quality)?;
        let candidate = consider_encoded(smallest, jpeg, "image/jpeg", image);
        if candidate.data.len() <= byte_budget {
            return Some(candidate);
        }
    }
    None
}

// Original: encodeWithinBudget(). As in the source, if no candidate meets the
// byte budget it returns the smallest candidate rather than failing.
fn encode_within_budget(
    mut image: DynamicImage,
    jpeg_only: bool,
    byte_budget: usize,
    fallback_edges: &[u32],
) -> Option<EncodedImage> {
    let mut smallest = None;

    if !jpeg_only {
        let png = encode_png(&image)?;
        let candidate = consider_encoded(&mut smallest, png, "image/png", &image);
        if candidate.data.len() <= byte_budget {
            return Some(candidate);
        }

        for &edge in fallback_edges {
            if edge < PNG_RESCALE_FLOOR_PX {
                break;
            }
            if !fit_within_edge(&mut image, edge) {
                continue;
            }
            let png = encode_png(&image)?;
            let candidate = consider_encoded(&mut smallest, png, "image/png", &image);
            if candidate.data.len() <= byte_budget {
                return Some(candidate);
            }
        }

        if let Some(encoded) = jpeg_ladder(&image, byte_budget, &mut smallest) {
            return Some(encoded);
        }
        for &edge in fallback_edges {
            if edge >= PNG_RESCALE_FLOOR_PX || !fit_within_edge(&mut image, edge) {
                continue;
            }
            if let Some(encoded) = jpeg_ladder(&image, byte_budget, &mut smallest) {
                return Some(encoded);
            }
        }
        return smallest;
    }

    if let Some(encoded) = jpeg_ladder(&image, byte_budget, &mut smallest) {
        return Some(encoded);
    }
    for &edge in fallback_edges {
        if !fit_within_edge(&mut image, edge) {
            continue;
        }
        if let Some(encoded) = jpeg_ladder(&image, byte_budget, &mut smallest) {
            return Some(encoded);
        }
    }
    smallest
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

pub type PersistOriginal =
    Arc<dyn Fn(Vec<u8>, String) -> BoxFuture<'static, Option<String>> + Send + Sync>;

#[derive(Clone, Default)]
pub struct CompressAnnotateOptions {
    pub persist_original: Option<PersistOriginal>,
}
#[derive(Clone, Default)]
pub struct CompressImageContentPartsOptions {
    pub compress: CompressImageOptions,
    pub annotate: Option<CompressAnnotateOptions>,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CompressedContentParts {
    pub parts: Vec<ContentPart>,
    pub captions: Vec<String>,
}

// Original: compressImageContentParts(). Formatting is gated before any
// base64 decode; persistence failures never prevent an already-compressed image from being sent.
pub async fn compress_image_content_parts(
    parts: &[ContentPart],
    options: &CompressImageContentPartsOptions,
) -> CompressedContentParts {
    let mut output = Vec::with_capacity(parts.len());
    let mut captions = Vec::new();
    for part in gate_image_format_parts(parts) {
        let ContentPart::ImageUrl { image_url } = &part else {
            output.push(part);
            continue;
        };
        let Some(parsed) = parse_image_data_url(&image_url.url) else {
            output.push(part);
            continue;
        };
        // Full decode + re-encode ladder can take seconds; run it off the
        // async executor so it never stalls other tool work.
        let result = tokio::task::spawn_blocking({
            let base64 = parsed.base64.clone();
            let mime_type = parsed.mime_type.clone();
            let compress = options.compress.clone();
            move || compress_base64_for_model(&base64, &mime_type, &compress)
        })
        .await
        .expect("compress_base64_for_model panicked");
        if !result.changed {
            output.push(part);
            continue;
        }
        if let Some(annotate) = &options.annotate {
            let original_path = match (&annotate.persist_original, STANDARD.decode(&parsed.base64))
            {
                (Some(persist), Ok(bytes)) => persist(bytes, parsed.mime_type.clone()).await,
                _ => None,
            };
            captions.push(build_image_compression_caption(
                &ImageCompressionCaptionInput {
                    original: ImageVariantDescription {
                        width: result.original_width,
                        height: result.original_height,
                        byte_length: result.original_byte_length,
                        mime_type: parsed.mime_type,
                    },
                    final_variant: ImageVariantDescription {
                        width: result.width,
                        height: result.height,
                        byte_length: result.final_byte_length,
                        mime_type: result.mime_type.clone(),
                    },
                    original_path,
                },
            ));
        }
        output.push(ContentPart::ImageUrl {
            image_url: MediaUrl {
                url: format!("data:{};base64,{}", result.mime_type, result.base64),
                id: image_url.id.clone(),
            },
        });
    }
    CompressedContentParts {
        parts: output,
        captions,
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ImageVariantDescription {
    pub width: i64,
    pub height: i64,
    pub byte_length: usize,
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
pub fn format_byte_size(bytes: u64) -> String {
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    if bytes < 1024 * 1024 {
        return format!("{} KB", (bytes as f64 / 1024.0).round());
    }
    format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
}

fn describe_image_variant(variant: &ImageVariantDescription) -> String {
    let size = format!(
        "{} ({})",
        variant.mime_type,
        format_byte_size(variant.byte_length as u64)
    );
    if variant.width > 0 && variant.height > 0 {
        format!("{}x{} {size}", variant.width, variant.height)
    } else {
        size
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::app::telemetry::{TelemetryAppender, TelemetryService, TelemetryServiceContract};

    use super::*;

    struct RecordingAppender(Arc<Mutex<Vec<(String, TelemetryProperties)>>>);

    impl TelemetryAppender for RecordingAppender {
        fn track(&self, event: &str, properties: Option<&TelemetryProperties>) {
            self.0
                .lock()
                .push((event.into(), properties.cloned().unwrap_or_default()));
        }
    }

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
                width: 4000,
                height: 3000,
                byte_length: (3.75 * 1024.0 * 1024.0) as usize,
                mime_type: "image/png".into(),
            },
            final_variant: ImageVariantDescription {
                width: 2000,
                height: 1500,
                byte_length: 1280,
                mime_type: "image/jpeg".into(),
            },
            original_path: Some("/tmp/original.png".into()),
        });
        let extracted = extract_image_compression_captions(&format!("before{caption}after"));
        assert_eq!(extracted.text, "beforeafter");
        assert_eq!(extracted.captions.len(), 1);
        assert!(extracted.captions[0].contains("4000x3000 image/png (3.8 MB)"));
        assert_eq!(format_byte_size(1023), "1023 B");
        assert_eq!(format_byte_size(1536), "2 KB");
    }
    #[test]
    fn compresses_oversized_dimensions_and_leaves_small_images_unchanged() {
        let bitmap = DynamicImage::new_rgb8(4, 2);
        let mut encoded = Cursor::new(Vec::new());
        bitmap.write_to(&mut encoded, ImageFormat::Png).unwrap();
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

    #[test]
    fn compression_reports_the_source_telemetry_payload() {
        let records = Arc::new(Mutex::new(Vec::new()));
        let telemetry = Arc::new(TelemetryService::new());
        telemetry.set_appender(Arc::new(RecordingAppender(Arc::clone(&records))));
        let bitmap = DynamicImage::new_rgb8(4, 2);
        let mut encoded = Cursor::new(Vec::new());
        bitmap.write_to(&mut encoded, ImageFormat::Png).unwrap();
        let result = compress_image_for_model(
            &encoded.into_inner(),
            "image/png",
            &CompressImageOptions {
                max_edge: Some(2),
                telemetry: Some(ImageCompressionTelemetry {
                    client: TelemetryServiceHandle(telemetry),
                    source: "mcp_tool_result".into(),
                }),
                ..Default::default()
            },
        );
        assert!(result.changed);
        let records = records.lock();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].0, "image_compress");
        assert_eq!(
            records[0].1["source"],
            Some(serde_json::json!("mcp_tool_result"))
        );
        assert_eq!(
            records[0].1["outcome"],
            Some(serde_json::json!("compressed"))
        );
        assert_eq!(records[0].1["original_width"], Some(serde_json::json!(4)));
        assert_eq!(records[0].1["final_width"], Some(serde_json::json!(2)));
    }

    #[test]
    fn compression_uses_source_rounding_and_progressive_fallback_edges() {
        let bitmap = DynamicImage::new_rgb8(4, 3);
        let mut encoded = Cursor::new(Vec::new());
        bitmap.write_to(&mut encoded, ImageFormat::Png).unwrap();
        let rounded = compress_image_for_model(
            &encoded.into_inner(),
            "image/png",
            &CompressImageOptions {
                max_edge: Some(2),
                ..Default::default()
            },
        );
        assert_eq!((rounded.width, rounded.height), (2, 2));

        let bitmap = DynamicImage::new_rgb8(600, 400);
        let mut encoded = Cursor::new(Vec::new());
        bitmap.write_to(&mut encoded, ImageFormat::Png).unwrap();
        let fallback = compress_image_for_model(
            &encoded.into_inner(),
            "image/png",
            &CompressImageOptions {
                byte_budget: Some(1),
                ..Default::default()
            },
        );
        assert!(fallback.changed);
        assert_eq!((fallback.width, fallback.height), (256, 171));
    }
    #[tokio::test]
    async fn compresses_gated_content_parts_and_emits_annotations_only_when_requested() {
        let bitmap = DynamicImage::new_rgb8(4, 2);
        let mut encoded = Cursor::new(Vec::new());
        bitmap.write_to(&mut encoded, ImageFormat::Png).unwrap();
        let url = format!(
            "data:image/png;base64,{}",
            STANDARD.encode(encoded.into_inner())
        );
        let result = compress_image_content_parts(
            &[image(&url)],
            &CompressImageContentPartsOptions {
                compress: CompressImageOptions {
                    max_edge: Some(2),
                    ..Default::default()
                },
                annotate: Some(CompressAnnotateOptions::default()),
            },
        )
        .await;
        assert_eq!(result.captions.len(), 1);
        assert!(
            matches!(&result.parts[..], [ContentPart::ImageUrl { image_url }] if image_url.url.starts_with("data:image/"))
        );
    }
    #[test]
    fn crops_regions_and_reports_invalid_coordinates() {
        let bitmap = DynamicImage::new_rgb8(4, 3);
        let mut encoded = Cursor::new(Vec::new());
        bitmap.write_to(&mut encoded, ImageFormat::Png).unwrap();
        let bytes = encoded.into_inner();
        let outcome = crop_image_for_model(
            &bytes,
            "image/png",
            ImageCropRegion {
                x: 1,
                y: 1,
                width: 8,
                height: 8,
            },
            &CropImageOptions {
                skip_resize: true,
                ..Default::default()
            },
        );
        assert!(matches!(
            outcome,
            CropImageOutcome::Success(CropImageSuccess {
                width: 3,
                height: 2,
                resized: false,
                ..
            })
        ));
        let downscaled_over_budget = crop_image_for_model(
            &bytes,
            "image/png",
            ImageCropRegion {
                x: 0,
                y: 0,
                width: 4,
                height: 3,
            },
            &CropImageOptions {
                compress: CompressImageOptions {
                    byte_budget: Some(1),
                    ..Default::default()
                },
                ..Default::default()
            },
        );
        assert!(matches!(
            downscaled_over_budget,
            CropImageOutcome::Success(_)
        ));
        let invalid = crop_image_for_model(
            &bytes,
            "image/png",
            ImageCropRegion {
                x: 10,
                y: 0,
                width: 1,
                height: 1,
            },
            &CropImageOptions::default(),
        );
        assert!(
            matches!(invalid, CropImageOutcome::Failure(CropImageFailure { error }) if error.contains("outside"))
        );
    }

    #[test]
    fn crop_reports_success_and_failure_telemetry_like_the_source() {
        let records = Arc::new(Mutex::new(Vec::new()));
        let telemetry = Arc::new(TelemetryService::new());
        telemetry.set_appender(Arc::new(RecordingAppender(Arc::clone(&records))));
        let telemetry = Some(ImageCompressionTelemetry {
            client: TelemetryServiceHandle(telemetry),
            source: "read_media".into(),
        });
        let bitmap = DynamicImage::new_rgb8(4, 2);
        let mut encoded = Cursor::new(Vec::new());
        bitmap.write_to(&mut encoded, ImageFormat::Png).unwrap();
        let bytes = encoded.into_inner();

        let success = crop_image_for_model(
            &bytes,
            "image/png",
            ImageCropRegion {
                x: 1,
                y: 0,
                width: 2,
                height: 2,
            },
            &CropImageOptions {
                compress: CompressImageOptions {
                    telemetry: telemetry.clone(),
                    ..Default::default()
                },
                ..Default::default()
            },
        );
        assert!(matches!(success, CropImageOutcome::Success(_)));

        let failure = crop_image_for_model(
            &bytes,
            "image/png",
            ImageCropRegion {
                x: -1,
                y: 0,
                width: 1,
                height: 1,
            },
            &CropImageOptions {
                compress: CompressImageOptions {
                    telemetry,
                    ..Default::default()
                },
                ..Default::default()
            },
        );
        assert!(matches!(failure, CropImageOutcome::Failure(_)));

        let records = records.lock();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].0, "image_crop");
        assert_eq!(
            records[0].1["source"],
            Some(serde_json::json!("read_media"))
        );
        assert_eq!(records[0].1["ok"], Some(serde_json::json!(true)));
        assert_eq!(
            records[0].1["region_area_ratio"],
            Some(serde_json::json!(0.5))
        );
        assert_eq!(records[1].0, "image_crop");
        assert_eq!(records[1].1["ok"], Some(serde_json::json!(false)));
        assert_eq!(
            records[1].1["error_kind"],
            Some(serde_json::json!("out_of_bounds"))
        );
    }
}
