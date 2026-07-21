//! Transcript-side rendering of pasted image attachments.

use base64::Engine as _;

use crate::tui::{
    components::render::truncate_to_width,
    theme::{ColorToken, current_theme},
    utils::image_attachment_store::ImageAttachment,
};

const MAX_IMAGE_ROWS: usize = 12;
const MAX_IMAGE_WIDTH: usize = 40;
const MIN_INLINE_RENDER_WIDTH: usize = MAX_IMAGE_WIDTH + 2;
const KITTY_CHUNK_BYTES: usize = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InlineImageProtocol {
    Kitty,
    ITerm2,
}

/// Detects the same common terminal families supported by the original TUI.
pub fn detect_inline_image_protocol() -> Option<InlineImageProtocol> {
    let term = std::env::var("TERM").unwrap_or_default().to_lowercase();
    if std::env::var_os("KITTY_WINDOW_ID").is_some() || term.contains("kitty") {
        return Some(InlineImageProtocol::Kitty);
    }

    let term_program = std::env::var("TERM_PROGRAM")
        .unwrap_or_default()
        .to_lowercase();
    let lc_terminal = std::env::var("LC_TERMINAL")
        .unwrap_or_default()
        .to_lowercase();
    (term_program.contains("iterm") || lc_terminal.contains("iterm"))
        .then_some(InlineImageProtocol::ITerm2)
}

/// Width-aware image thumbnail with a text fallback for unsupported terminals.
///
/// Original:
/// `src/tui/components/media/image-thumbnail.ts`, `ImageThumbnail`.
#[derive(Debug, Clone)]
pub struct ImageThumbnail {
    attachment: ImageAttachment,
    last_render_width: usize,
    last_built_width: Option<usize>,
    last_built_protocol: Option<Option<InlineImageProtocol>>,
    built_lines: Vec<String>,
    protocol_override: Option<InlineImageProtocol>,
    has_protocol_override: bool,
}

impl ImageThumbnail {
    pub fn new(attachment: ImageAttachment) -> Self {
        Self::build_initial(attachment, None, false)
    }

    /// Creates a deterministic thumbnail for callers that already negotiated
    /// terminal capabilities. Passing `None` forces the text fallback.
    pub fn with_protocol(
        attachment: ImageAttachment,
        protocol: Option<InlineImageProtocol>,
    ) -> Self {
        Self::build_initial(attachment, protocol, true)
    }

    fn build_initial(
        attachment: ImageAttachment,
        protocol_override: Option<InlineImageProtocol>,
        has_protocol_override: bool,
    ) -> Self {
        let mut thumbnail = Self {
            attachment,
            last_render_width: 80,
            last_built_width: None,
            last_built_protocol: None,
            built_lines: Vec::new(),
            protocol_override,
            has_protocol_override,
        };
        let protocol = thumbnail.protocol();
        thumbnail.build_children(80, protocol);
        thumbnail
    }

    fn protocol(&self) -> Option<InlineImageProtocol> {
        if self.has_protocol_override {
            self.protocol_override
        } else {
            detect_inline_image_protocol()
        }
    }

    fn fallback(&self, width: usize) -> Vec<String> {
        vec![truncate_to_width(
            &current_theme().fg(ColorToken::Accent, &self.attachment.placeholder),
            width,
            "…",
            false,
        )]
    }

    // Original: ImageThumbnail.buildChildren().
    fn build_children(&mut self, width: usize, protocol: Option<InlineImageProtocol>) {
        self.built_lines = match protocol {
            None => self.fallback(width),
            Some(protocol) => self.inline_lines(width, protocol),
        };
        self.last_built_width = Some(width);
        self.last_built_protocol = Some(protocol);
    }

    fn inline_lines(&self, width: usize, protocol: InlineImageProtocol) -> Vec<String> {
        let max_columns = MAX_IMAGE_WIDTH.min(width.saturating_sub(2)).max(1);
        let (columns, rows) = scaled_cell_size(
            self.attachment.width,
            self.attachment.height,
            max_columns,
            MAX_IMAGE_ROWS,
        );
        let encoded = base64::engine::general_purpose::STANDARD.encode(&self.attachment.bytes);
        let command = match protocol {
            InlineImageProtocol::Kitty => kitty_image(&encoded, columns, rows),
            InlineImageProtocol::ITerm2 => {
                iterm2_image(&encoded, &self.attachment.placeholder, columns, rows)
            }
        };

        let mut lines = Vec::with_capacity(rows);
        lines.push(command);
        lines.resize(rows, String::new());
        lines
    }

    // Original: ImageThumbnail.render().
    pub fn render(&mut self, width: usize) -> Vec<String> {
        self.last_render_width = width;
        if width < MIN_INLINE_RENDER_WIDTH {
            return self.fallback(width);
        }

        let protocol = self.protocol();
        if self.last_built_width != Some(width) || self.last_built_protocol != Some(protocol) {
            self.build_children(width, protocol);
        }
        self.built_lines.clone()
    }

    // Original: ImageThumbnail.invalidate().
    pub fn invalidate(&mut self) {
        let protocol = self.protocol();
        self.build_children(self.last_render_width, protocol);
    }
}

fn scaled_cell_size(
    pixel_width: u32,
    pixel_height: u32,
    max_columns: usize,
    max_rows: usize,
) -> (usize, usize) {
    if pixel_width == 0 || pixel_height == 0 {
        return (max_columns.max(1), 1);
    }

    // Terminal cells are approximately twice as tall as they are wide.
    let natural_rows = (u128::from(pixel_height) * max_columns as u128)
        .div_ceil(u128::from(pixel_width) * 2) as usize;
    if natural_rows <= max_rows {
        return (max_columns, natural_rows.max(1));
    }

    let columns = (u128::from(pixel_width) * max_rows as u128 * 2)
        .div_ceil(u128::from(pixel_height)) as usize;
    (columns.clamp(1, max_columns), max_rows.max(1))
}

fn kitty_image(encoded: &str, columns: usize, rows: usize) -> String {
    let mut chunks = encoded.as_bytes().chunks(KITTY_CHUNK_BYTES).peekable();
    let mut output = String::new();
    if chunks.peek().is_none() {
        return format!("\x1b_Gf=100,a=T,t=d,q=2,c={columns},r={rows},m=0;\x1b\\");
    }

    for (index, chunk) in chunks.enumerate() {
        let more = usize::from((index + 1) * KITTY_CHUNK_BYTES < encoded.len());
        let payload = std::str::from_utf8(chunk).expect("base64 is ASCII");
        if index == 0 {
            output.push_str(&format!(
                "\x1b_Gf=100,a=T,t=d,q=2,c={columns},r={rows},m={more};{payload}\x1b\\"
            ));
        } else {
            output.push_str(&format!("\x1b_Gm={more};{payload}\x1b\\"));
        }
    }
    output
}

fn iterm2_image(encoded: &str, filename: &str, columns: usize, rows: usize) -> String {
    let encoded_name = base64::engine::general_purpose::STANDARD.encode(filename.as_bytes());
    format!(
        "\x1b]1337;File=name={encoded_name};inline=1;width={columns};height={rows};preserveAspectRatio=1:{encoded}\x07"
    )
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::tui::components::render::visible_width;

    use super::*;

    fn image() -> ImageAttachment {
        ImageAttachment {
            id: 1,
            bytes: Arc::from([137, 80, 78, 71]),
            mime: "image/png".to_owned(),
            width: 800,
            height: 600,
            original: None,
            placeholder: "[image #1 (800脳600)]".to_owned(),
        }
    }

    #[test]
    fn keeps_fallback_output_within_narrow_widths() {
        let mut thumbnail = ImageThumbnail::with_protocol(image(), None);
        for width in [39, 20, 3, 1, 0] {
            assert!(
                thumbnail
                    .render(width)
                    .iter()
                    .all(|line| visible_width(line) <= width)
            );
        }
    }

    #[test]
    fn does_not_rebuild_same_width_and_protocol() {
        let mut thumbnail =
            ImageThumbnail::with_protocol(image(), Some(InlineImageProtocol::Kitty));
        let first = thumbnail.render(80);
        let built_width = thumbnail.last_built_width;
        let second = thumbnail.render(80);

        assert_eq!(first, second);
        assert_eq!(thumbnail.last_built_width, built_width);
    }

    #[test]
    fn emits_supported_terminal_protocols_with_capped_dimensions() {
        let mut kitty = ImageThumbnail::with_protocol(image(), Some(InlineImageProtocol::Kitty));
        let kitty_lines = kitty.render(80);
        assert_eq!(kitty_lines.len(), 12);
        assert!(kitty_lines[0].starts_with("\x1b_Gf=100,a=T,t=d,q=2,c=32,r=12"));
        assert!(kitty_lines[0].contains("iVBORw=="));

        let mut iterm = ImageThumbnail::with_protocol(image(), Some(InlineImageProtocol::ITerm2));
        let iterm_lines = iterm.render(80);
        assert_eq!(iterm_lines.len(), 12);
        assert!(iterm_lines[0].contains("width=32;height=12"));
        assert!(iterm_lines[0].contains("preserveAspectRatio=1:iVBORw=="));
    }

    #[test]
    fn narrow_render_uses_fallback_even_when_images_are_supported() {
        let mut thumbnail =
            ImageThumbnail::with_protocol(image(), Some(InlineImageProtocol::Kitty));
        let lines = thumbnail.render(39);
        assert_eq!(lines.len(), 1);
        assert_eq!(
            visible_width(&lines[0]),
            visible_width("[image #1 (800脳600)]")
        );
        assert!(!lines[0].contains("\x1b_G"));
    }

    #[test]
    fn kitty_payload_is_chunked_for_large_attachments() {
        let encoded = "a".repeat(KITTY_CHUNK_BYTES + 4);
        let command = kitty_image(&encoded, 40, 12);
        assert!(command.contains("m=1;"));
        assert!(command.contains("\x1b_Gm=0;aaaa\x1b\\"));
    }
}
