use std::{
    error::Error,
    fmt::{self, Display, Formatter},
    path::{Path, PathBuf},
    sync::OnceLock,
};

use base64::{Engine, engine::general_purpose::STANDARD};
use regex::Regex;

use crate::{
    sdk::types::{MediaUrl, PromptPart},
    utils::paths::{HomeDirectoryUnavailable, get_cache_dir},
};

use super::image_attachment_store::{
    ImageAttachment, ImageAttachmentOriginal, ImageAttachmentStore, MediaAttachment,
    VideoAttachment,
};

#[derive(Debug)]
pub enum MediaExtractionError {
    HomeDirectoryUnavailable(HomeDirectoryUnavailable),
    Io(std::io::Error),
}

impl Display for MediaExtractionError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::HomeDirectoryUnavailable(error) => Display::fmt(error, formatter),
            Self::Io(error) => Display::fmt(error, formatter),
        }
    }
}

impl Error for MediaExtractionError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::HomeDirectoryUnavailable(error) => Some(error),
            Self::Io(error) => Some(error),
        }
    }
}

impl From<HomeDirectoryUnavailable> for MediaExtractionError {
    fn from(error: HomeDirectoryUnavailable) -> Self {
        Self::HomeDirectoryUnavailable(error)
    }
}

impl From<std::io::Error> for MediaExtractionError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtractionResult {
    pub parts: Vec<PromptPart>,
    pub has_media: bool,
    pub image_attachment_ids: Vec<u64>,
    pub video_attachment_ids: Vec<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaTagRewriteResult {
    pub text: String,
    pub has_media: bool,
    pub image_attachment_ids: Vec<u64>,
    pub video_attachment_ids: Vec<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaReferenceStyle {
    Tag,
    Plain,
}

fn placeholder_pattern() -> Option<&'static Regex> {
    static PATTERN: OnceLock<Option<Regex>> = OnceLock::new();
    PATTERN
        .get_or_init(|| Regex::new(r"\[(image|video) #(\d+) (?:(\(\d+×\d+\))|([^\]]+))\]").ok())
        .as_ref()
}

/// Original:
///   apps/kimi-code/src/tui/utils/image-placeholder.ts
///   extractMediaAttachments()
pub async fn extract_media_attachments(
    text: &str,
    store: &ImageAttachmentStore,
) -> Result<ExtractionResult, MediaExtractionError> {
    extract_media_attachments_internal(text, store, None).await
}

pub async fn extract_media_attachments_with_cache_dir(
    text: &str,
    store: &ImageAttachmentStore,
    cache_dir: &Path,
) -> Result<ExtractionResult, MediaExtractionError> {
    extract_media_attachments_internal(text, store, Some(cache_dir)).await
}

async fn extract_media_attachments_internal(
    text: &str,
    store: &ImageAttachmentStore,
    cache_dir: Option<&Path>,
) -> Result<ExtractionResult, MediaExtractionError> {
    let mut parts = Vec::new();
    let mut image_attachment_ids = Vec::new();
    let mut video_attachment_ids = Vec::new();
    let mut cursor = 0;
    let mut has_media = false;

    let Some(pattern) = placeholder_pattern() else {
        return Ok(ExtractionResult {
            parts,
            has_media,
            image_attachment_ids,
            video_attachment_ids,
        });
    };
    for captures in pattern.captures_iter(text) {
        let Some(complete) = captures.get(0) else {
            continue;
        };
        let Some(kind) = captures.get(1).map(|value| value.as_str()) else {
            continue;
        };
        let Some(id) = captures
            .get(2)
            .and_then(|value| value.as_str().parse::<u64>().ok())
        else {
            continue;
        };
        let Some(attachment) = store.get(id) else {
            continue;
        };
        let kind_matches = matches!(
            (kind, attachment),
            ("image", MediaAttachment::Image(_)) | ("video", MediaAttachment::Video(_))
        );
        if !kind_matches {
            continue;
        }

        push_text(&mut parts, &text[cursor..complete.start()]);
        match attachment {
            MediaAttachment::Video(attachment) => {
                let cache_path = materialize_video_to_cache(attachment, cache_dir, false).await?;
                push_text(&mut parts, &format_media_tag("video", &cache_path));
                video_attachment_ids.push(id);
            }
            MediaAttachment::Image(attachment) => {
                if attachment.original.is_some() {
                    push_text(&mut parts, &caption_for_compressed_image(attachment));
                }
                parts.push(image_part_for_attachment(attachment));
                image_attachment_ids.push(id);
            }
        }
        has_media = true;
        cursor = complete.end();
    }
    push_text(&mut parts, &text[cursor..]);

    Ok(ExtractionResult {
        parts: if has_media { parts } else { Vec::new() },
        has_media,
        image_attachment_ids,
        video_attachment_ids,
    })
}

/// Original:
///   apps/kimi-code/src/tui/utils/image-placeholder.ts
///   rewriteMediaPlaceholders()
pub async fn rewrite_media_placeholders(
    text: &str,
    store: &ImageAttachmentStore,
    style: MediaReferenceStyle,
) -> Result<MediaTagRewriteResult, MediaExtractionError> {
    rewrite_media_placeholders_internal(text, store, style, None).await
}

pub async fn rewrite_media_placeholders_with_cache_dir(
    text: &str,
    store: &ImageAttachmentStore,
    style: MediaReferenceStyle,
    cache_dir: &Path,
) -> Result<MediaTagRewriteResult, MediaExtractionError> {
    rewrite_media_placeholders_internal(text, store, style, Some(cache_dir)).await
}

async fn rewrite_media_placeholders_internal(
    text: &str,
    store: &ImageAttachmentStore,
    style: MediaReferenceStyle,
    cache_dir: Option<&Path>,
) -> Result<MediaTagRewriteResult, MediaExtractionError> {
    let mut image_attachment_ids = Vec::new();
    let mut video_attachment_ids = Vec::new();
    let mut cursor = 0;
    let mut output = String::new();

    if let Some(pattern) = placeholder_pattern() {
        for captures in pattern.captures_iter(text) {
            let Some(complete) = captures.get(0) else {
                continue;
            };
            let Some(kind) = captures.get(1).map(|value| value.as_str()) else {
                continue;
            };
            let Some(id) = captures
                .get(2)
                .and_then(|value| value.as_str().parse::<u64>().ok())
            else {
                continue;
            };
            let Some(attachment) = store.get(id) else {
                continue;
            };
            let kind_matches = matches!(
                (kind, attachment),
                ("image", MediaAttachment::Image(_)) | ("video", MediaAttachment::Video(_))
            );
            if !kind_matches {
                continue;
            }

            output.push_str(&text[cursor..complete.start()]);
            match attachment {
                MediaAttachment::Video(attachment) => {
                    let path = materialize_video_to_cache(
                        attachment,
                        cache_dir,
                        style == MediaReferenceStyle::Plain,
                    )
                    .await?;
                    output.push_str(&format_media_reference_or_tag("video", &path, style));
                    video_attachment_ids.push(id);
                }
                MediaAttachment::Image(attachment) => {
                    let path = materialize_image_to_cache(attachment, cache_dir).await?;
                    output.push_str(&format_media_reference_or_tag("image", &path, style));
                    image_attachment_ids.push(id);
                }
            }
            cursor = complete.end();
        }
    }

    let has_media = !image_attachment_ids.is_empty() || !video_attachment_ids.is_empty();
    if has_media {
        output.push_str(&text[cursor..]);
    } else {
        output = text.to_owned();
    }
    Ok(MediaTagRewriteResult {
        text: output,
        has_media,
        image_attachment_ids,
        video_attachment_ids,
    })
}

fn push_text(parts: &mut Vec<PromptPart>, segment: &str) {
    if segment.is_empty() || segment.trim().is_empty() {
        return;
    }
    if let Some(PromptPart::Text { text }) = parts.last_mut() {
        text.push_str(segment);
    } else {
        parts.push(PromptPart::Text {
            text: segment.to_owned(),
        });
    }
}

fn image_part_for_attachment(attachment: &ImageAttachment) -> PromptPart {
    let base64 = STANDARD.encode(&attachment.bytes);
    PromptPart::ImageUrl {
        image_url: MediaUrl {
            url: format!("data:{};base64,{base64}", attachment.mime),
            id: None,
        },
    }
}

async fn materialize_video_to_cache(
    attachment: &VideoAttachment,
    cache_dir: Option<&Path>,
    escape_proof_name: bool,
) -> Result<PathBuf, MediaExtractionError> {
    let cache_dir = resolve_cache_dir(cache_dir)?;
    tokio::fs::create_dir_all(&cache_dir).await?;
    let label = if escape_proof_name {
        attachment.label.replace(['<', '>', '&', '"'], "_")
    } else {
        attachment.label.clone()
    };
    let target = cache_dir.join(format!("{}-{label}", uuid::Uuid::new_v4()));
    tokio::fs::copy(&attachment.source_path, &target).await?;
    Ok(target)
}

async fn materialize_image_to_cache(
    attachment: &ImageAttachment,
    cache_dir: Option<&Path>,
) -> Result<PathBuf, MediaExtractionError> {
    let cache_dir = resolve_cache_dir(cache_dir)?;
    tokio::fs::create_dir_all(&cache_dir).await?;
    let extension = match attachment.mime.trim().to_lowercase().as_str() {
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/gif" => "gif",
        "image/webp" => "webp",
        "image/bmp" => "bmp",
        "image/tiff" => "tif",
        _ => "img",
    };
    let target = cache_dir.join(format!("{}.{extension}", uuid::Uuid::new_v4()));
    tokio::fs::write(&target, &attachment.bytes).await?;
    Ok(target)
}

fn resolve_cache_dir(cache_dir: Option<&Path>) -> Result<PathBuf, MediaExtractionError> {
    cache_dir
        .map(Path::to_path_buf)
        .map(Ok)
        .unwrap_or_else(|| get_cache_dir().map_err(Into::into))
}

fn caption_for_compressed_image(attachment: &ImageAttachment) -> String {
    let Some(original) = attachment.original.as_ref() else {
        return String::new();
    };
    build_image_compression_caption(
        original,
        attachment.width,
        attachment.height,
        attachment.bytes.len(),
        &attachment.mime,
    )
}

fn build_image_compression_caption(
    original: &ImageAttachmentOriginal,
    final_width: u32,
    final_height: u32,
    final_byte_length: usize,
    final_mime: &str,
) -> String {
    let mut sentences = vec![
        format!(
            "Image compressed to fit model limits: original {} -> sent {}.",
            describe_image_variant(
                original.width,
                original.height,
                original.byte_length,
                &original.mime,
            ),
            describe_image_variant(final_width, final_height, final_byte_length, final_mime),
        ),
        "Fine detail may be lost.".to_owned(),
    ];
    if let Some(path) = original.path.as_deref().filter(|path| !path.is_empty()) {
        sentences.push(format!(
            "The uncompressed original is saved at \"{path}\"; if you need fine detail (e.g. small text), call ReadMediaFile on that path with the region parameter (original-pixel coordinates) to view a crop at full fidelity."
        ));
    } else {
        sentences.push("The uncompressed original was not preserved.".to_owned());
    }
    format!("<system>{}</system>", sentences.join(" "))
}

fn describe_image_variant(width: u32, height: u32, bytes: usize, mime: &str) -> String {
    let size = format!("{mime} ({})", format_byte_size(bytes));
    if width > 0 && height > 0 {
        format!("{width}x{height} {size}")
    } else {
        size
    }
}

fn format_byte_size(bytes: usize) -> String {
    if bytes < 1_024 {
        format!("{bytes} B")
    } else if bytes < 1_024 * 1_024 {
        format!("{} KB", bytes.saturating_add(512) / 1_024)
    } else {
        format!("{:.1} MB", bytes as f64 / (1_024.0 * 1_024.0))
    }
}

fn format_media_reference_or_tag(
    kind: &'static str,
    path: &Path,
    style: MediaReferenceStyle,
) -> String {
    match style {
        MediaReferenceStyle::Tag => format_media_tag(kind, path),
        MediaReferenceStyle::Plain => format!(
            "Attached {kind} file: {} (open it with ReadMediaFile)",
            path.to_string_lossy()
        ),
    }
}

fn format_media_tag(tag: &str, path: &Path) -> String {
    format!(
        "<{tag} path=\"{}\"></{tag}>",
        escape_attribute(&path.to_string_lossy())
    )
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

    fn temp_dir(prefix: &str) -> PathBuf {
        std::env::temp_dir().join(format!("{prefix}-{}", uuid::Uuid::new_v4()))
    }

    async fn clean(path: &Path) {
        if path.starts_with(std::env::temp_dir()) {
            let _ = tokio::fs::remove_dir_all(path).await;
        }
    }

    #[tokio::test]
    async fn plain_and_unresolved_text_stay_on_the_plain_path() {
        let store = ImageAttachmentStore::new();
        for text in ["hello world", "try [image #999 (1×1)] now"] {
            let result =
                extract_media_attachments_with_cache_dir(text, &store, &temp_dir("unused-cache"))
                    .await;
            assert!(matches!(result, Ok(value) if !value.has_media && value.parts.is_empty()));
        }
    }

    #[tokio::test]
    async fn extracts_images_in_order_with_data_urls() {
        let mut store = ImageAttachmentStore::new();
        let first = store.add_image(vec![1], "image/png", 10, 10, None);
        let second = store.add_image(vec![2], "image/png", 20, 20, None);
        let text = format!(
            "first {} then {} end",
            first.placeholder, second.placeholder
        );
        let result =
            extract_media_attachments_with_cache_dir(&text, &store, &temp_dir("unused-cache"))
                .await
                .unwrap_or_else(|error| panic!("extract failed: {error}"));

        assert_eq!(result.image_attachment_ids, [1, 2]);
        assert_eq!(
            result.parts,
            [
                PromptPart::Text {
                    text: "first ".to_owned()
                },
                PromptPart::ImageUrl {
                    image_url: MediaUrl {
                        url: "data:image/png;base64,AQ==".to_owned(),
                        id: None
                    }
                },
                PromptPart::Text {
                    text: " then ".to_owned()
                },
                PromptPart::ImageUrl {
                    image_url: MediaUrl {
                        url: "data:image/png;base64,Ag==".to_owned(),
                        id: None
                    }
                },
                PromptPart::Text {
                    text: " end".to_owned()
                }
            ]
        );
    }

    #[tokio::test]
    async fn copies_videos_to_cache_and_escapes_tag_paths() {
        let source_dir = temp_dir("kimi-media-source");
        let cache_dir = temp_dir("kimi-media-cache").join("a&b");
        tokio::fs::create_dir_all(&source_dir)
            .await
            .unwrap_or_else(|error| panic!("mkdir failed: {error}"));
        let source = source_dir.join("clip.mov");
        tokio::fs::write(&source, b"video-bytes")
            .await
            .unwrap_or_else(|error| panic!("write failed: {error}"));
        let mut store = ImageAttachmentStore::new();
        let video = store.add_video(
            "video/quicktime",
            source.to_string_lossy(),
            Some("clip&one.mov"),
        );

        let result =
            extract_media_attachments_with_cache_dir(&video.placeholder, &store, &cache_dir)
                .await
                .unwrap_or_else(|error| panic!("extract failed: {error}"));
        let PromptPart::Text { text } = &result.parts[0] else {
            panic!("expected a text video tag")
        };
        assert!(text.contains("a&amp;b"));
        assert!(text.contains("clip&amp;one.mov"));
        let copied = tokio::fs::read_dir(&cache_dir)
            .await
            .unwrap_or_else(|error| panic!("read dir failed: {error}"));
        drop(copied);
        assert_eq!(result.video_attachment_ids, [1]);
        clean(&source_dir).await;
        if let Some(parent) = cache_dir.parent() {
            clean(parent).await;
        }
    }

    #[tokio::test]
    async fn prepends_compression_caption_before_image() {
        let mut store = ImageAttachmentStore::new();
        let image = store.add_image(
            vec![1, 2, 3],
            "image/png",
            2_000,
            2_000,
            Some(ImageAttachmentOriginal {
                path: Some("/tmp/original.png".to_owned()),
                width: 2_600,
                height: 2_600,
                byte_length: 123_456,
                mime: "image/png".to_owned(),
            }),
        );
        let result = extract_media_attachments_with_cache_dir(
            &image.placeholder,
            &store,
            &temp_dir("unused-cache"),
        )
        .await
        .unwrap_or_else(|error| panic!("extract failed: {error}"));

        assert_eq!(result.parts.len(), 2);
        assert!(matches!(
            &result.parts[0],
            PromptPart::Text { text }
                if text.contains("Image compressed")
                    && text.contains("2600x2600")
                    && text.contains("/tmp/original.png")
        ));
    }

    #[test]
    fn compression_byte_sizes_match_javascript_rounding() {
        assert_eq!(format_byte_size(640), "640 B");
        assert_eq!(format_byte_size(2_560), "3 KB");
        assert_eq!(format_byte_size(3_984_589), "3.8 MB");
    }

    #[tokio::test]
    async fn rewrites_images_and_videos_with_verbatim_surrounding_text() {
        let source_dir = temp_dir("kimi-media-source");
        let cache_dir = temp_dir("kimi-media-cache");
        tokio::fs::create_dir_all(&source_dir)
            .await
            .unwrap_or_else(|error| panic!("mkdir failed: {error}"));
        let source = source_dir.join("clip.mov");
        tokio::fs::write(&source, b"video")
            .await
            .unwrap_or_else(|error| panic!("write failed: {error}"));
        let mut store = ImageAttachmentStore::new();
        let image = store.add_image(vec![0x89, 0x50], "image/png", 10, 10, None);
        let video = store.add_video("video/quicktime", source.to_string_lossy(), None);
        let text = format!(
            "first {}   then {} end",
            image.placeholder, video.placeholder
        );

        let result = rewrite_media_placeholders_with_cache_dir(
            &text,
            &store,
            MediaReferenceStyle::Tag,
            &cache_dir,
        )
        .await
        .unwrap_or_else(|error| panic!("rewrite failed: {error}"));
        assert!(result.text.starts_with("first <image path="));
        assert!(result.text.contains(">   then <video path="));
        assert!(result.text.ends_with("></video> end"));
        assert_eq!(result.image_attachment_ids, [1]);
        assert_eq!(result.video_attachment_ids, [2]);
        clean(&source_dir).await;
        clean(&cache_dir).await;
    }

    #[tokio::test]
    async fn plain_references_are_xml_escape_proof() {
        let source_dir = temp_dir("kimi-media-source");
        let cache_dir = temp_dir("kimi-media-cache");
        tokio::fs::create_dir_all(&source_dir)
            .await
            .unwrap_or_else(|error| panic!("mkdir failed: {error}"));
        let source = source_dir.join("clip.mov");
        tokio::fs::write(&source, b"video")
            .await
            .unwrap_or_else(|error| panic!("write failed: {error}"));
        let mut store = ImageAttachmentStore::new();
        let video = store.add_video(
            "video/quicktime",
            source.to_string_lossy(),
            Some("clip<1>&\".mov"),
        );

        let result = rewrite_media_placeholders_with_cache_dir(
            &video.placeholder,
            &store,
            MediaReferenceStyle::Plain,
            &cache_dir,
        )
        .await
        .unwrap_or_else(|error| panic!("rewrite failed: {error}"));
        assert!(!result.text.chars().any(|value| "<>&\"".contains(value)));
        assert!(result.text.starts_with("Attached video file: "));
        clean(&source_dir).await;
        clean(&cache_dir).await;
    }
}
