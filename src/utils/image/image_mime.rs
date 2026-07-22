#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SupportedImageMime {
    Png,
    Jpeg,
    Gif,
    Webp,
}

impl SupportedImageMime {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Png => "image/png",
            Self::Jpeg => "image/jpeg",
            Self::Gif => "image/gif",
            Self::Webp => "image/webp",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImageMeta {
    pub mime: SupportedImageMime,
    pub width: u32,
    pub height: u32,
}

/// Detects an accepted image format and reads its canvas dimensions.
///
/// Original:
///   apps/kimi-code/src/utils/image/image-mime.ts
///   parseImageMeta()
pub fn parse_image_meta(bytes: &[u8]) -> Option<ImageMeta> {
    if is_png(bytes) {
        parse_png(bytes)
    } else if is_jpeg(bytes) {
        parse_jpeg(bytes)
    } else if is_gif(bytes) {
        parse_gif(bytes)
    } else if is_webp(bytes) {
        parse_webp(bytes)
    } else {
        None
    }
}

fn is_png(bytes: &[u8]) -> bool {
    bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a])
}

fn parse_png(bytes: &[u8]) -> Option<ImageMeta> {
    let width = read_u32_be(bytes, 16)?;
    let height = read_u32_be(bytes, 20)?;
    nonzero_meta(SupportedImageMime::Png, width, height)
}

fn is_jpeg(bytes: &[u8]) -> bool {
    bytes.starts_with(&[0xff, 0xd8, 0xff])
}

fn parse_jpeg(bytes: &[u8]) -> Option<ImageMeta> {
    let mut index = 2;
    while index < bytes.len() {
        if bytes[index] != 0xff {
            index += 1;
            continue;
        }
        while bytes.get(index) == Some(&0xff) {
            index += 1;
        }
        let marker = *bytes.get(index)?;
        index += 1;
        if matches!(marker, 0xd8 | 0xd9) {
            continue;
        }
        let segment_length = usize::from(read_u16_be(bytes, index)?);
        if is_sof_marker(marker) {
            let height = u32::from(read_u16_be(bytes, index + 3)?);
            let width = u32::from(read_u16_be(bytes, index + 5)?);
            return nonzero_meta(SupportedImageMime::Jpeg, width, height);
        }
        index = index.checked_add(segment_length)?;
    }
    None
}

fn is_sof_marker(marker: u8) -> bool {
    (0xc0..=0xcf).contains(&marker) && !matches!(marker, 0xc4 | 0xc8 | 0xcc)
}

fn is_gif(bytes: &[u8]) -> bool {
    bytes.len() >= 6
        && &bytes[..4] == b"GIF8"
        && matches!(bytes[4], b'7' | b'9')
        && bytes[5] == b'a'
}

fn parse_gif(bytes: &[u8]) -> Option<ImageMeta> {
    let width = u32::from(read_u16_le(bytes, 6)?);
    let height = u32::from(read_u16_le(bytes, 8)?);
    nonzero_meta(SupportedImageMime::Gif, width, height)
}

fn is_webp(bytes: &[u8]) -> bool {
    bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP"
}

fn parse_webp(bytes: &[u8]) -> Option<ImageMeta> {
    if bytes.len() < 30 {
        return None;
    }
    match &bytes[12..16] {
        b"VP8 " => {
            let width = u32::from(read_u16_le(bytes, 26)? & 0x3fff);
            let height = u32::from(read_u16_le(bytes, 28)? & 0x3fff);
            nonzero_meta(SupportedImageMime::Webp, width, height)
        }
        b"VP8L" => {
            if bytes[20] != 0x2f {
                return None;
            }
            let byte_1 = u32::from(bytes[21]);
            let byte_2 = u32::from(bytes[22]);
            let byte_3 = u32::from(bytes[23]);
            let byte_4 = u32::from(bytes[24]);
            let width = 1 + (((byte_2 & 0x3f) << 8) | byte_1);
            let height = 1 + (((byte_4 & 0x0f) << 10) | (byte_3 << 2) | ((byte_2 & 0xc0) >> 6));
            nonzero_meta(SupportedImageMime::Webp, width, height)
        }
        b"VP8X" => {
            let width = 1 + read_u24_le(bytes, 24)?;
            let height = 1 + read_u24_le(bytes, 27)?;
            nonzero_meta(SupportedImageMime::Webp, width, height)
        }
        _ => None,
    }
}

fn nonzero_meta(mime: SupportedImageMime, width: u32, height: u32) -> Option<ImageMeta> {
    (width > 0 && height > 0).then_some(ImageMeta {
        mime,
        width,
        height,
    })
}

fn read_u16_be(bytes: &[u8], offset: usize) -> Option<u16> {
    Some(u16::from_be_bytes([
        *bytes.get(offset)?,
        *bytes.get(offset + 1)?,
    ]))
}

fn read_u16_le(bytes: &[u8], offset: usize) -> Option<u16> {
    Some(u16::from_le_bytes([
        *bytes.get(offset)?,
        *bytes.get(offset + 1)?,
    ]))
}

fn read_u24_le(bytes: &[u8], offset: usize) -> Option<u32> {
    Some(
        u32::from(*bytes.get(offset)?)
            | (u32::from(*bytes.get(offset + 1)?) << 8)
            | (u32::from(*bytes.get(offset + 2)?) << 16),
    )
}

fn read_u32_be(bytes: &[u8], offset: usize) -> Option<u32> {
    Some(u32::from_be_bytes([
        *bytes.get(offset)?,
        *bytes.get(offset + 1)?,
        *bytes.get(offset + 2)?,
        *bytes.get(offset + 3)?,
    ]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_png_gif_and_rejects_zero_or_truncated_dimensions() {
        let mut png = vec![0; 24];
        png[..8].copy_from_slice(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]);
        png[16..20].copy_from_slice(&640_u32.to_be_bytes());
        png[20..24].copy_from_slice(&480_u32.to_be_bytes());
        assert_eq!(
            parse_image_meta(&png),
            Some(ImageMeta {
                mime: SupportedImageMime::Png,
                width: 640,
                height: 480
            })
        );
        let gif = [b'G', b'I', b'F', b'8', b'9', b'a', 0x20, 0x03, 0x58, 0x02];
        assert_eq!(
            parse_image_meta(&gif).map(|meta| (meta.width, meta.height)),
            Some((800, 600))
        );
        png[16..20].fill(0);
        assert_eq!(parse_image_meta(&png), None);
        assert_eq!(parse_image_meta(&png[..10]), None);
    }

    #[test]
    fn scans_jpeg_segments_for_start_of_frame() {
        let jpeg = [
            0xff, 0xd8, 0xff, 0xe0, 0x00, 0x04, 0xaa, 0xbb, 0xff, 0xc0, 0x00, 0x08, 0x08, 0x01,
            0x2c, 0x02, 0x80, 0x03,
        ];
        assert_eq!(
            parse_image_meta(&jpeg),
            Some(ImageMeta {
                mime: SupportedImageMime::Jpeg,
                width: 640,
                height: 300
            })
        );
        assert_eq!(parse_image_meta(&jpeg[..14]), None);
    }

    #[test]
    fn parses_all_three_webp_dimension_layouts() {
        let mut vp8 = vec![0; 30];
        vp8[..4].copy_from_slice(b"RIFF");
        vp8[8..12].copy_from_slice(b"WEBP");
        vp8[12..16].copy_from_slice(b"VP8 ");
        vp8[26..28].copy_from_slice(&640_u16.to_le_bytes());
        vp8[28..30].copy_from_slice(&480_u16.to_le_bytes());
        assert_eq!(
            parse_image_meta(&vp8).map(|meta| (meta.width, meta.height)),
            Some((640, 480))
        );

        let mut vp8l = vp8.clone();
        vp8l[12..16].copy_from_slice(b"VP8L");
        vp8l[20] = 0x2f;
        let width_minus_one = 319_u32;
        let height_minus_one = 239_u32;
        vp8l[21] = width_minus_one as u8;
        vp8l[22] = ((width_minus_one >> 8) as u8 & 0x3f) | ((height_minus_one as u8 & 0x03) << 6);
        vp8l[23] = (height_minus_one >> 2) as u8;
        vp8l[24] = (height_minus_one >> 10) as u8;
        assert_eq!(
            parse_image_meta(&vp8l).map(|meta| (meta.width, meta.height)),
            Some((320, 240))
        );

        let mut vp8x = vp8;
        vp8x[12..16].copy_from_slice(b"VP8X");
        vp8x[24..27].copy_from_slice(&[0xff, 0x03, 0x00]);
        vp8x[27..30].copy_from_slice(&[0xff, 0x01, 0x00]);
        assert_eq!(
            parse_image_meta(&vp8x).map(|meta| (meta.width, meta.height)),
            Some((1024, 512))
        );
    }

    #[test]
    fn rejects_unsupported_signatures_and_invalid_webp_chunks() {
        assert_eq!(parse_image_meta(b"BM bitmap"), None);
        let mut webp = vec![0; 30];
        webp[..4].copy_from_slice(b"RIFF");
        webp[8..12].copy_from_slice(b"WEBP");
        webp[12..16].copy_from_slice(b"NOPE");
        assert_eq!(parse_image_meta(&webp), None);
    }
}
