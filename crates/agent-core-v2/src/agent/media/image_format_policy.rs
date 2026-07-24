//! Provider-accepted image-format policy and data-URL helpers.
//!
//! Original: `packages/agent-core-v2/src/agent/media/image-format-policy.ts`.

use base64::{
    Engine as _,
    engine::general_purpose::{STANDARD, STANDARD_NO_PAD, URL_SAFE, URL_SAFE_NO_PAD},
};

use super::{mime_for_image_suffix, sniff_media_from_magic};

pub const MODEL_ACCEPTED_IMAGE_MIMES: &[&str] =
    &["image/png", "image/jpeg", "image/gif", "image/webp"];
const ACCEPTED_FORMATS_TEXT: &str = "PNG, JPEG, GIF, and WebP";
const BASE64_SNIFF_CHARS: usize = 48;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ParsedImageDataUrl {
    pub mime_type: String,
    pub base64: String,
}

pub fn normalize_image_mime(mime_type: &str) -> String {
    let lower = mime_type.trim().to_ascii_lowercase();
    let base = lower
        .split_once(';')
        .map_or(lower.as_str(), |(base, _)| base)
        .trim();
    if base == "image/jpg" {
        "image/jpeg".into()
    } else {
        base.into()
    }
}

// Original: decodeBase64Prefix(). Node's Buffer decoder is best-effort; each
// standard and unpadded alphabet is attempted before falling back to empty.
pub fn decode_base64_prefix(base64: &str) -> Vec<u8> {
    let prefix = &base64[..base64.len().min(BASE64_SNIFF_CHARS)];
    [
        STANDARD.decode(prefix),
        STANDARD_NO_PAD.decode(prefix),
        URL_SAFE.decode(prefix),
        URL_SAFE_NO_PAD.decode(prefix),
    ]
    .into_iter()
    .find_map(Result::ok)
    .unwrap_or_default()
}

pub fn resolve_effective_image_mime(declared_mime: &str, header: &[u8]) -> String {
    sniff_media_from_magic(header).map_or_else(|| declared_mime.into(), |value| value.mime_type)
}

pub fn unsupported_image_mime_from_url(url: &str) -> Option<String> {
    let path = url.split_once('?').map_or(url, |(path, _)| path);
    let path = path.split_once('#').map_or(path, |(path, _)| path);
    let dot = path.rfind('.')?;
    let ext = path[dot..].to_ascii_lowercase();
    let mime = if ext == ".svg" {
        Some("image/svg+xml")
    } else {
        mime_for_image_suffix(&ext)
    }?;
    (!is_model_accepted_image_mime(mime)).then(|| mime.into())
}

pub fn parse_image_data_url(url: &str) -> Option<ParsedImageDataUrl> {
    let rest = url.strip_prefix("data:").or_else(|| {
        url.get(..5)
            .filter(|prefix| prefix.eq_ignore_ascii_case("data:"))
            .map(|_| &url[5..])
    })?;
    let comma = rest.find(',')?;
    let (header, base64) = rest.split_at(comma);
    let mut parts = header.split(';');
    let mime_type = parts.next()?.trim();
    if mime_type.is_empty() || !parts.any(|part| part.eq_ignore_ascii_case("base64")) {
        return None;
    }
    Some(ParsedImageDataUrl {
        mime_type: mime_type.into(),
        base64: base64[1..].into(),
    })
}

pub fn is_data_url(url: &str) -> bool {
    url.get(..5)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("data:"))
}
pub fn is_model_accepted_image_mime(mime_type: &str) -> bool {
    MODEL_ACCEPTED_IMAGE_MIMES.contains(&normalize_image_mime(mime_type).as_str())
}

pub fn build_image_conversion_guidance(path: &str, mime_type: &str, os_kind: &str) -> String {
    let converted = match path.rfind(['.', '/', '\\']) {
        Some(index) if path.as_bytes()[index] == b'.' => format!("{}.jpg", &path[..index]),
        _ => format!("{path}.jpg"),
    };
    format!(
        "\"{path}\" is an {mime_type} image, which the provider does not accept. Convert it to JPEG first, then read the converted file. {}",
        image_conversion_command(path, &converted, os_kind, &normalize_image_mime(mime_type))
    )
}

fn image_conversion_command(path: &str, converted: &str, os_kind: &str, mime_type: &str) -> String {
    let magick = format!("magick \"{path}\" \"{converted}\"");
    let decoder = matches!(mime_type, "image/heic" | "image/heif")
        .then_some(("heif-convert", "libheif-examples"));
    match os_kind {
        "macOS" => format!("On macOS: sips -s format jpeg \"{path}\" --out \"{converted}\""),
        "Linux" => decoder.map_or_else(|| format!("On Linux, with ImageMagick: {magick}"), |(command, package)| format!("On Linux: {command} \"{path}\" \"{converted}\" (package {package}), or with ImageMagick: {magick}")),
        "Windows" => format!("On Windows, with ImageMagick: {magick} (install it first if missing: winget install ImageMagick.ImageMagick)"),
        _ => format!("Options: sips -s format jpeg \"{path}\" --out \"{converted}\" (macOS){} , or {magick}", decoder.map_or(String::new(), |(command, package)| format!(", {command} \"{path}\" \"{converted}\" (Linux, package {package})")).replace(" ,", ",")),
    }
}

pub fn build_unsupported_image_notice(mime_type: &str, name: Option<&str>) -> String {
    let what = name.filter(|value| !value.is_empty()).map_or_else(
        || format!("unsupported image format {mime_type}"),
        |value| format!("\"{value}\" uses unsupported image format {mime_type}"),
    );
    format!(
        "[Image omitted: {what}. Model providers accept only {ACCEPTED_FORMATS_TEXT} — convert it to PNG or JPEG and try again.]"
    )
}
pub fn build_malformed_image_notice(url: &str) -> String {
    let shown = if url.len() > 80 {
        format!("{}…", &url[..80])
    } else {
        url.into()
    };
    format!(
        "[Image omitted: \"{shown}\" is not a valid data URL (its header or payload could not be parsed). Re-encode the image as PNG or JPEG and try again.]"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn normalizes_parses_and_resolves_image_mime() {
        assert_eq!(normalize_image_mime(" Image/JPG; charset=x "), "image/jpeg");
        assert_eq!(
            parse_image_data_url("DATA:image/png;foo=x;base64,aGVsbG8=").unwrap(),
            ParsedImageDataUrl {
                mime_type: "image/png".into(),
                base64: "aGVsbG8=".into()
            }
        );
        assert!(parse_image_data_url("data:image/png,abc").is_none());
        assert_eq!(
            resolve_effective_image_mime("image/jpeg", b"\x89PNG\r\n\x1a\n"),
            "image/png"
        );
    }
    #[test]
    fn rejects_unsupported_urls_and_preserves_exact_guidance() {
        assert_eq!(
            unsupported_image_mime_from_url("https://x/a.HEIC?q=1#x").as_deref(),
            Some("image/heic")
        );
        assert_eq!(unsupported_image_mime_from_url("x.jpg"), None);
        assert!(
            build_image_conversion_guidance("a.heic", "image/heic", "Linux")
                .contains("heif-convert \"a.heic\" \"a.jpg\" (package libheif-examples)")
        );
        assert_eq!(
            build_unsupported_image_notice("image/avif", Some("a.avif")),
            "[Image omitted: \"a.avif\" uses unsupported image format image/avif. Model providers accept only PNG, JPEG, GIF, and WebP — convert it to PNG or JPEG and try again.]"
        );
    }
}
