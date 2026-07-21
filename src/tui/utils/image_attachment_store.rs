use std::{collections::HashMap, sync::Arc};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageAttachmentOriginal {
    pub path: Option<String>,
    pub width: u32,
    pub height: u32,
    pub byte_length: usize,
    pub mime: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageAttachment {
    pub id: u64,
    pub bytes: Arc<[u8]>,
    pub mime: String,
    pub width: u32,
    pub height: u32,
    pub original: Option<ImageAttachmentOriginal>,
    pub placeholder: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoAttachment {
    pub id: u64,
    pub mime: String,
    pub filename: String,
    pub source_path: String,
    pub label: String,
    pub placeholder: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MediaAttachment {
    Image(ImageAttachment),
    Video(VideoAttachment),
}

impl MediaAttachment {
    pub fn id(&self) -> u64 {
        match self {
            Self::Image(attachment) => attachment.id,
            Self::Video(attachment) => attachment.id,
        }
    }
}

/// Per-TUI registry for media pasted into the editor.
///
/// Original:
///   apps/kimi-code/src/tui/utils/image-attachment-store.ts
///   ImageAttachmentStore
#[derive(Debug, Clone)]
pub struct ImageAttachmentStore {
    next_id: u64,
    by_id: HashMap<u64, MediaAttachment>,
}

impl Default for ImageAttachmentStore {
    fn default() -> Self {
        Self::new()
    }
}

impl ImageAttachmentStore {
    pub fn new() -> Self {
        Self {
            next_id: 1,
            by_id: HashMap::new(),
        }
    }

    pub fn add_image(
        &mut self,
        bytes: impl Into<Arc<[u8]>>,
        mime: impl Into<String>,
        width: u32,
        height: u32,
        original: Option<ImageAttachmentOriginal>,
    ) -> ImageAttachment {
        let id = self.take_next_id();
        let attachment = ImageAttachment {
            id,
            bytes: bytes.into(),
            mime: mime.into(),
            width,
            height,
            original,
            placeholder: format_placeholder(id, width, height),
        };
        self.by_id
            .insert(id, MediaAttachment::Image(attachment.clone()));
        attachment
    }

    pub fn add_video(
        &mut self,
        mime: impl Into<String>,
        source_path: impl Into<String>,
        filename: Option<&str>,
    ) -> VideoAttachment {
        let id = self.take_next_id();
        let mime = mime.into();
        let source_path = source_path.into();
        let normalized_filename = basename_like(
            filename
                .filter(|filename| !filename.is_empty())
                .unwrap_or(&source_path),
        );
        let label_source = if normalized_filename.is_empty() {
            &mime
        } else {
            &normalized_filename
        };
        let label = sanitize_video_label(label_source);
        let attachment = VideoAttachment {
            id,
            mime,
            filename: normalized_filename,
            source_path,
            placeholder: format_video_placeholder(id, &label),
            label,
        };
        self.by_id
            .insert(id, MediaAttachment::Video(attachment.clone()));
        attachment
    }

    pub fn get(&self, id: u64) -> Option<&MediaAttachment> {
        self.by_id.get(&id)
    }

    pub fn clear(&mut self) {
        self.by_id.clear();
        self.next_id = 1;
    }

    pub fn remove(&mut self, id: u64) {
        self.by_id.remove(&id);
    }

    pub fn remove_many(&mut self, ids: impl IntoIterator<Item = u64>) {
        for id in ids {
            self.by_id.remove(&id);
        }
    }

    pub fn len(&self) -> usize {
        self.by_id.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_id.is_empty()
    }

    fn take_next_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        id
    }
}

pub fn format_placeholder(id: u64, width: u32, height: u32) -> String {
    format!("[image #{id} ({width}×{height})]")
}

pub fn format_video_placeholder(id: u64, label: &str) -> String {
    format!("[video #{id} {}]", sanitize_video_label(label))
}

fn sanitize_video_label(raw: &str) -> String {
    let label = raw
        .chars()
        .map(|character| {
            let code = u32::from(character);
            if code < 0x20 || code == 0x7f || matches!(character, '[' | ']') {
                '_'
            } else {
                character
            }
        })
        .collect::<String>();
    let label = label.trim();
    if label.is_empty() {
        "video".to_owned()
    } else {
        label.to_owned()
    }
}

fn basename_like(raw: &str) -> String {
    raw.split(['/', '\\'])
        .rfind(|part| !part.is_empty())
        .unwrap_or(raw)
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assigns_monotonic_ids_across_media_kinds() {
        let mut store = ImageAttachmentStore::new();
        let image = store.add_image(vec![1], "image/png", 10, 20, None);
        let video = store.add_video("video/quicktime", "/tmp/sample.mov", None);

        assert_eq!(image.id, 1);
        assert_eq!(video.id, 2);
    }

    #[test]
    fn builds_canonical_placeholders_and_sanitizes_video_labels() {
        assert_eq!(format_placeholder(1, 640, 480), "[image #1 (640×480)]");
        assert_eq!(
            format_video_placeholder(2, "bad[name]\u{0}.mov"),
            "[video #2 bad_name__.mov]"
        );
        assert_eq!(format_video_placeholder(3, "\n[]"), "[video #3 ___]");
        assert_eq!(format_video_placeholder(4, "   "), "[video #4 video]");
    }

    #[test]
    fn derives_video_filename_from_slash_or_backslash_paths() {
        let mut store = ImageAttachmentStore::new();
        let slash = store.add_video("video/mp4", "/tmp/clips/sample.mp4", None);
        let backslash = store.add_video("video/mp4", r"C:\clips\demo.mp4", None);
        let explicit = store.add_video("video/mp4", "/tmp/ignored.mp4", Some("nested/custom.mov"));

        assert_eq!(slash.filename, "sample.mp4");
        assert_eq!(slash.source_path, "/tmp/clips/sample.mp4");
        assert_eq!(backslash.filename, "demo.mp4");
        assert_eq!(explicit.filename, "custom.mov");
    }

    #[test]
    fn stores_shared_image_bytes_and_original_metadata() {
        let mut store = ImageAttachmentStore::new();
        let bytes = Arc::<[u8]>::from([9, 8, 7]);
        let original = ImageAttachmentOriginal {
            path: Some("/tmp/original.png".to_owned()),
            width: 200,
            height: 400,
            byte_length: 3,
            mime: "image/png".to_owned(),
        };
        let image = store.add_image(bytes.clone(), "image/jpeg", 100, 200, Some(original));

        let stored = store.get(image.id);
        assert!(matches!(
            stored,
            Some(MediaAttachment::Image(value)) if Arc::ptr_eq(&value.bytes, &bytes)
        ));
        assert_eq!(image.mime, "image/jpeg");
    }

    #[test]
    fn clear_resets_ids_and_remove_does_not() {
        let mut store = ImageAttachmentStore::new();
        let first = store.add_image(Vec::new(), "image/png", 10, 10, None);
        let second = store.add_image(Vec::new(), "image/png", 10, 10, None);
        store.remove(first.id);
        assert_eq!(store.len(), 1);
        assert!(store.get(second.id).is_some());
        assert_eq!(store.add_image(Vec::new(), "image/png", 10, 10, None).id, 3);

        store.clear();
        assert!(store.is_empty());
        assert_eq!(store.add_image(Vec::new(), "image/png", 10, 10, None).id, 1);
    }

    #[test]
    fn removes_many_attachments() {
        let mut store = ImageAttachmentStore::new();
        let first = store.add_image(vec![1], "image/png", 10, 10, None);
        let second = store.add_image(vec![2], "image/png", 10, 10, None);
        let third = store.add_image(vec![3], "image/png", 10, 10, None);
        store.remove_many([first.id, third.id]);

        assert_eq!(store.len(), 1);
        assert!(store.get(second.id).is_some());
    }
}
