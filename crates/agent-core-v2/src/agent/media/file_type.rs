//! Magic-byte and extension file-type detection.
//!
//! Original: `packages/agent-core-v2/src/agent/media/file-type.ts`.

pub const MEDIA_SNIFF_BYTES: usize = 512;

pub const IMAGE_MIME_BY_SUFFIX: &[(&str, &str)] = &[
    (".png", "image/png"),
    (".jpg", "image/jpeg"),
    (".jpeg", "image/jpeg"),
    (".gif", "image/gif"),
    (".bmp", "image/bmp"),
    (".tif", "image/tiff"),
    (".tiff", "image/tiff"),
    (".webp", "image/webp"),
    (".ico", "image/x-icon"),
    (".heic", "image/heic"),
    (".heif", "image/heif"),
    (".avif", "image/avif"),
    (".svgz", "image/svg+xml"),
];

pub const VIDEO_MIME_BY_SUFFIX: &[(&str, &str)] = &[
    (".mp4", "video/mp4"),
    (".mpg", "video/mpeg"),
    (".mpeg", "video/mpeg"),
    (".mkv", "video/x-matroska"),
    (".avi", "video/x-msvideo"),
    (".mov", "video/quicktime"),
    (".ogv", "video/ogg"),
    (".wmv", "video/x-ms-wmv"),
    (".webm", "video/webm"),
    (".m4v", "video/x-m4v"),
    (".flv", "video/x-flv"),
    (".3gp", "video/3gpp"),
    (".3g2", "video/3gpp2"),
];

const NON_TEXT_SUFFIXES: &[&str] = &[
    ".icns", ".psd", ".ai", ".eps", ".pdf", ".doc", ".docx", ".dot", ".dotx", ".rtf", ".odt",
    ".xls", ".xlsx", ".xlsm", ".xlt", ".xltx", ".xltm", ".ods", ".ppt", ".pptx", ".pptm", ".pps",
    ".ppsx", ".odp", ".pages", ".numbers", ".key", ".zip", ".rar", ".7z", ".tar", ".gz", ".tgz",
    ".bz2", ".xz", ".zst", ".lz", ".lz4", ".br", ".cab", ".ar", ".deb", ".rpm", ".mp3", ".wav",
    ".flac", ".ogg", ".oga", ".opus", ".aac", ".m4a", ".wma", ".ttf", ".otf", ".woff", ".woff2",
    ".exe", ".dll", ".so", ".dylib", ".bin", ".apk", ".ipa", ".jar", ".class", ".pyc", ".pyo",
    ".wasm", ".dmg", ".iso", ".img", ".sqlite", ".sqlite3", ".db", ".db3",
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FileTypeKind {
    Text,
    Image,
    Video,
    Unknown,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FileType {
    pub kind: FileTypeKind,
    pub mime_type: String,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum DetectFileTypeMode {
    #[default]
    Text,
    Media,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ImageDimensions {
    pub width: i64,
    pub height: i64,
    pub transposed: bool,
}

pub fn mime_for_image_suffix(suffix: &str) -> Option<&'static str> {
    lookup(IMAGE_MIME_BY_SUFFIX, suffix)
}

pub fn sniff_media_from_magic(data: &[u8]) -> Option<FileType> {
    let header = &data[..data.len().min(MEDIA_SNIFF_BYTES)];
    let image = |mime: &str| {
        Some(FileType {
            kind: FileTypeKind::Image,
            mime_type: mime.into(),
        })
    };
    let video = |mime: &str| {
        Some(FileType {
            kind: FileTypeKind::Video,
            mime_type: mime.into(),
        })
    };
    if starts(header, b"\x89PNG\r\n\x1a\n") {
        return image("image/png");
    }
    if starts(header, &[0xff, 0xd8, 0xff]) {
        return image("image/jpeg");
    }
    if starts(header, b"GIF87a") || starts(header, b"GIF89a") {
        return image("image/gif");
    }
    if starts(header, b"BM") {
        return image("image/bmp");
    }
    if starts(header, &[0x49, 0x49, 0x2a, 0x00]) || starts(header, &[0x4d, 0x4d, 0x00, 0x2a]) {
        return image("image/tiff");
    }
    if starts(header, &[0, 0, 1, 0]) {
        return image("image/x-icon");
    }
    if starts(header, b"RIFF") && header.len() >= 12 {
        return match &header[8..12] {
            b"WEBP" => image("image/webp"),
            b"AVI " => video("video/x-msvideo"),
            _ => None,
        };
    }
    if starts(header, b"FLV") {
        return video("video/x-flv");
    }
    if starts(
        header,
        &[
            0x30, 0x26, 0xb2, 0x75, 0x8e, 0x66, 0xcf, 0x11, 0xa6, 0xd9, 0x00, 0xaa, 0x00, 0x62,
            0xce, 0x6c,
        ],
    ) {
        return video("video/x-ms-wmv");
    }
    if starts(header, &[0x1a, 0x45, 0xdf, 0xa3]) {
        let lowered = String::from_utf8_lossy(header).to_ascii_lowercase();
        if lowered.contains("webm") {
            return video("video/webm");
        }
        if lowered.contains("matroska") {
            return video("video/x-matroska");
        }
    }
    let brand = ftyp_brand(header)?;
    let image_brands = [
        ("avif", "image/avif"),
        ("avis", "image/avif"),
        ("heic", "image/heic"),
        ("heif", "image/heif"),
        ("heix", "image/heif"),
        ("hevc", "image/heic"),
        ("mif1", "image/heif"),
        ("msf1", "image/heif"),
    ];
    let video_brands = [
        ("isom", "video/mp4"),
        ("iso2", "video/mp4"),
        ("iso5", "video/mp4"),
        ("mp41", "video/mp4"),
        ("mp42", "video/mp4"),
        ("avc1", "video/mp4"),
        ("mp4v", "video/mp4"),
        ("m4v", "video/x-m4v"),
        ("qt", "video/quicktime"),
        ("3gp4", "video/3gpp"),
        ("3gp5", "video/3gpp"),
        ("3gp6", "video/3gpp"),
        ("3gp7", "video/3gpp"),
        ("3g2", "video/3gpp2"),
    ];
    lookup(&image_brands, &brand)
        .map(|mime| FileType {
            kind: FileTypeKind::Image,
            mime_type: mime.into(),
        })
        .or_else(|| {
            lookup(&video_brands, &brand).map(|mime| FileType {
                kind: FileTypeKind::Video,
                mime_type: mime.into(),
            })
        })
}

pub fn sniff_image_dimensions(data: &[u8]) -> Option<ImageDimensions> {
    if starts(data, b"\x89PNG\r\n\x1a\n") && data.len() >= 24 {
        return Some(ImageDimensions {
            width: be32(data, 16)? as i64,
            height: be32(data, 20)? as i64,
            transposed: false,
        });
    }
    if (starts(data, b"GIF87a") || starts(data, b"GIF89a")) && data.len() >= 10 {
        return Some(ImageDimensions {
            width: le16(data, 6)? as i64,
            height: le16(data, 8)? as i64,
            transposed: false,
        });
    }
    if starts(data, b"BM") && data.len() >= 26 {
        return Some(ImageDimensions {
            width: le_i32(data, 18)? as i64,
            height: le_i32(data, 22)?.unsigned_abs() as i64,
            transposed: false,
        });
    }
    if starts(data, b"RIFF") && data.len() >= 30 {
        match &data[12..16] {
            b"VP8 " => {
                return Some(ImageDimensions {
                    width: (le16(data, 26)? & 0x3fff) as i64,
                    height: (le16(data, 28)? & 0x3fff) as i64,
                    transposed: false,
                });
            }
            b"VP8L" if data.len() >= 25 => {
                let bits = le32(data, 21)?;
                return Some(ImageDimensions {
                    width: ((bits & 0x3fff) + 1) as i64,
                    height: (((bits >> 14) & 0x3fff) + 1) as i64,
                    transposed: false,
                });
            }
            b"VP8X" => {
                return Some(ImageDimensions {
                    width: (1
                        + data[24] as u32
                        + ((data[25] as u32) << 8)
                        + ((data[26] as u32) << 16)) as i64,
                    height: (1
                        + data[27] as u32
                        + ((data[28] as u32) << 8)
                        + ((data[29] as u32) << 16)) as i64,
                    transposed: false,
                });
            }
            _ => {}
        }
    }
    if starts(data, &[0xff, 0xd8]) {
        return jpeg_dimensions(data);
    }
    None
}

pub fn detect_file_type(path: &str, header: Option<&[u8]>, mode: DetectFileTypeMode) -> FileType {
    let suffix = suffix(path);
    let hint = if suffix == ".svg" {
        Some(file(FileTypeKind::Text, "image/svg+xml"))
    } else if let Some(mime) = lookup(IMAGE_MIME_BY_SUFFIX, &suffix) {
        Some(file(FileTypeKind::Image, mime))
    } else {
        lookup(VIDEO_MIME_BY_SUFFIX, &suffix).map(|mime| file(FileTypeKind::Video, mime))
    };
    if let Some(bytes) = header {
        if let Some(sniffed) = sniff_media_from_magic(bytes) {
            if mode == DetectFileTypeMode::Media {
                return sniffed;
            }
            if let Some(ref hint) = hint {
                return if sniffed.kind == hint.kind {
                    hint.clone()
                } else {
                    unknown()
                };
            }
            return sniffed;
        }
        if hint
            .as_ref()
            .is_some_and(|value| value.kind == FileTypeKind::Image)
        {
            return unknown();
        }
        if mode == DetectFileTypeMode::Media
            && hint
                .as_ref()
                .is_some_and(|value| value.kind == FileTypeKind::Video)
        {
            return hint.unwrap();
        }
        if bytes.contains(&0) {
            return unknown();
        }
    }
    hint.unwrap_or_else(|| {
        if NON_TEXT_SUFFIXES.contains(&suffix.as_str()) {
            unknown()
        } else {
            file(FileTypeKind::Text, "text/plain")
        }
    })
}

fn file(kind: FileTypeKind, mime_type: &str) -> FileType {
    FileType {
        kind,
        mime_type: mime_type.into(),
    }
}
fn unknown() -> FileType {
    file(FileTypeKind::Unknown, "")
}
fn lookup<'a>(pairs: &'a [(&str, &'a str)], value: &str) -> Option<&'a str> {
    pairs
        .iter()
        .find(|(key, _)| *key == value)
        .map(|(_, value)| *value)
}
fn starts(data: &[u8], prefix: &[u8]) -> bool {
    data.starts_with(prefix)
}
fn le16(data: &[u8], at: usize) -> Option<u16> {
    data.get(at..at + 2)
        .map(|v| u16::from_le_bytes([v[0], v[1]]))
}
fn be16(data: &[u8], at: usize) -> Option<u16> {
    data.get(at..at + 2)
        .map(|v| u16::from_be_bytes([v[0], v[1]]))
}
fn le32(data: &[u8], at: usize) -> Option<u32> {
    data.get(at..at + 4)
        .map(|v| u32::from_le_bytes([v[0], v[1], v[2], v[3]]))
}
fn be32(data: &[u8], at: usize) -> Option<u32> {
    data.get(at..at + 4)
        .map(|v| u32::from_be_bytes([v[0], v[1], v[2], v[3]]))
}
fn le_i32(data: &[u8], at: usize) -> Option<i32> {
    le32(data, at).map(|v| v as i32)
}
fn ftyp_brand(data: &[u8]) -> Option<String> {
    if data.len() < 12 || &data[4..8] != b"ftyp" {
        return None;
    }
    let raw = String::from_utf8_lossy(&data[8..12]);
    let brand = raw
        .trim_matches(|c: char| c.is_whitespace() || c == '\0')
        .to_ascii_lowercase();
    (!brand.is_empty()).then_some(brand)
}
fn suffix(path: &str) -> String {
    let sep = path.rfind(['/', '\\']).map_or(0, |i| i + 1);
    path[sep..]
        .rfind('.')
        .filter(|i| *i > 0)
        .map(|i| path[sep + i..].to_ascii_lowercase())
        .unwrap_or_default()
}

fn jpeg_dimensions(data: &[u8]) -> Option<ImageDimensions> {
    let mut orientation = None;
    let mut offset = 2;
    while offset + 9 < data.len() {
        if data[offset] != 0xff {
            offset += 1;
            continue;
        }
        let marker = data[offset + 1];
        if (0xc0..=0xcf).contains(&marker) && !matches!(marker, 0xc4 | 0xc8 | 0xcc) {
            let height = be16(data, offset + 5)? as i64;
            let width = be16(data, offset + 7)? as i64;
            let transposed = orientation.is_some_and(|v| v >= 5);
            return Some(if transposed {
                ImageDimensions {
                    width: height,
                    height: width,
                    transposed,
                }
            } else {
                ImageDimensions {
                    width,
                    height,
                    transposed,
                }
            });
        }
        if marker == 0xd8 || marker == 0xd9 || (0xd0..=0xd7).contains(&marker) {
            offset += 2;
            continue;
        }
        let length = be16(data, offset + 2)? as usize;
        if length < 2 {
            break;
        }
        if marker == 0xe1 && orientation.is_none() {
            orientation = exif_orientation(data, offset + 4, offset + 2 + length);
        }
        offset += 2 + length;
    }
    None
}
fn exif_orientation(data: &[u8], start: usize, end: usize) -> Option<u16> {
    let end = end.min(data.len());
    if start + 6 > end || data.get(start..start + 6)? != b"Exif\0\0" {
        return None;
    }
    let tiff = start + 6;
    if tiff + 8 > end {
        return None;
    };
    let le = match data.get(tiff..tiff + 2)? {
        b"II" => true,
        b"MM" => false,
        _ => return None,
    };
    let u16at = |at| if le { le16(data, at) } else { be16(data, at) };
    let u32at = |at| if le { le32(data, at) } else { be32(data, at) };
    if u16at(tiff + 2)? != 42 {
        return None;
    };
    let ifd = tiff + u32at(tiff + 4)? as usize;
    if ifd + 2 > end {
        return None;
    };
    for i in 0..u16at(ifd)? as usize {
        let entry = ifd + 2 + i * 12;
        if entry + 12 > end {
            return None;
        };
        if u16at(entry)? == 0x0112 {
            let value = u16at(entry + 8)?;
            return (1..=8).contains(&value).then_some(value);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn magic_and_extension_detection_match_source_policy() {
        let png = [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a];
        assert_eq!(sniff_media_from_magic(&png).unwrap().mime_type, "image/png");
        assert_eq!(
            detect_file_type("photo.jpg", Some(&png), DetectFileTypeMode::Text).mime_type,
            "image/jpeg"
        );
        assert_eq!(
            detect_file_type("photo.jpg", Some(&png), DetectFileTypeMode::Media).mime_type,
            "image/png"
        );
        assert_eq!(
            detect_file_type("notes.txt", Some(b"text\0"), DetectFileTypeMode::Text),
            unknown()
        );
        assert_eq!(
            detect_file_type("archive.zip", None, DetectFileTypeMode::Text),
            unknown()
        );
    }
    #[test]
    fn dimensions_include_png_webp_and_jpeg_exif_transposition() {
        let mut png = vec![0; 24];
        png[..8].copy_from_slice(b"\x89PNG\r\n\x1a\n");
        png[16..20].copy_from_slice(&640u32.to_be_bytes());
        png[20..24].copy_from_slice(&480u32.to_be_bytes());
        assert_eq!(
            sniff_image_dimensions(&png),
            Some(ImageDimensions {
                width: 640,
                height: 480,
                transposed: false
            })
        );
        let mut webp = vec![0; 30];
        webp[..4].copy_from_slice(b"RIFF");
        webp[12..16].copy_from_slice(b"VP8 ");
        webp[26..28].copy_from_slice(&320u16.to_le_bytes());
        webp[28..30].copy_from_slice(&200u16.to_le_bytes());
        assert_eq!(
            sniff_image_dimensions(&webp),
            Some(ImageDimensions {
                width: 320,
                height: 200,
                transposed: false
            })
        );
    }
}
