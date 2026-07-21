use std::any::Any;

use crate::tui::{
    components::{
        Component, ComponentRole, Spacer, Text,
        media::image_thumbnail::{ImageThumbnail, InlineImageProtocol},
        render::{truncate_to_width, visible_width},
    },
    theme::{ColorToken, current_theme},
    utils::{image_attachment_store::ImageAttachment, render_cache::is_render_cache_enabled},
};

const USER_MESSAGE_BULLET: &str = "✦ ";

/// User transcript row with optional inline image thumbnails.
///
/// Original:
/// `src/tui/components/messages/user-message.ts`, `UserMessageComponent`.
pub struct UserMessageComponent {
    text: String,
    bullet: Option<String>,
    spacer_component: Spacer,
    image_thumbnails: Vec<ImageThumbnail>,
    render_cache: Option<(usize, Vec<String>)>,
}

impl UserMessageComponent {
    pub fn new(
        text: impl Into<String>,
        images: Vec<ImageAttachment>,
        bullet: Option<String>,
    ) -> Self {
        Self {
            text: text.into(),
            bullet,
            spacer_component: Spacer::new(1),
            image_thumbnails: images.into_iter().map(ImageThumbnail::new).collect(),
            render_cache: None,
        }
    }

    /// Deterministic constructor for an already-negotiated image protocol.
    pub fn with_image_protocol(
        text: impl Into<String>,
        images: Vec<ImageAttachment>,
        bullet: Option<String>,
        protocol: Option<InlineImageProtocol>,
    ) -> Self {
        Self {
            text: text.into(),
            bullet,
            spacer_component: Spacer::new(1),
            image_thumbnails: images
                .into_iter()
                .map(|image| ImageThumbnail::with_protocol(image, protocol))
                .collect(),
            render_cache: None,
        }
    }

    fn mark_render_dirty(&mut self) {
        self.render_cache = None;
    }
}

impl Component for UserMessageComponent {
    // Original: UserMessageComponent.render().
    fn render(&mut self, width: usize) -> Vec<String> {
        if width == 0 {
            return vec![String::new()];
        }
        if is_render_cache_enabled()
            && let Some((cached_width, lines)) = &self.render_cache
            && *cached_width == width
        {
            return lines.clone();
        }

        let marker = self.bullet.as_deref().unwrap_or(USER_MESSAGE_BULLET);
        let bullet = if marker.is_empty() {
            String::new()
        } else {
            current_theme().bold_fg(ColorToken::RoleUser, marker)
        };
        let bullet_width = visible_width(&bullet);
        let content_width = width.saturating_sub(bullet_width).max(1);
        let mut lines = self.spacer_component.render(width);

        let colored_text = current_theme().bold_fg(ColorToken::RoleUser, &self.text);
        let mut text_component = Text::new(colored_text, 0, 0);
        for (index, text_line) in text_component.render(content_width).into_iter().enumerate() {
            let prefix = if index == 0 {
                bullet.clone()
            } else {
                " ".repeat(bullet_width)
            };
            lines.push(format!("{prefix}{text_line}"));
        }

        for thumbnail in &mut self.image_thumbnails {
            for image_line in thumbnail.render(content_width) {
                lines.push(format!("{}{image_line}", " ".repeat(bullet_width)));
            }
        }

        let rendered = lines
            .into_iter()
            .map(|line| {
                if is_image_line(&line) {
                    line
                } else {
                    truncate_to_width(&line, width, "…", false)
                }
            })
            .collect::<Vec<_>>();
        if is_render_cache_enabled() {
            self.render_cache = Some((width, rendered.clone()));
        }
        rendered
    }

    // Original: UserMessageComponent.invalidate().
    fn invalidate(&mut self) {
        self.mark_render_dirty();
        for thumbnail in &mut self.image_thumbnails {
            thumbnail.invalidate();
        }
    }

    fn role(&self) -> ComponentRole {
        ComponentRole::UserMessage
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

fn is_image_line(line: &str) -> bool {
    line.contains("\x1b_G") || line.contains("\x1b]1337;File=")
}

/// Invisible replay-only turn boundary.
///
/// Original: `ReplayTurnBoundaryComponent` in `user-message.ts`.
#[derive(Debug, Default, Clone, Copy)]
pub struct ReplayTurnBoundaryComponent;

impl Component for ReplayTurnBoundaryComponent {
    fn render(&mut self, _width: usize) -> Vec<String> {
        Vec::new()
    }

    fn invalidate(&mut self) {}

    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use regex::Regex;
    use std::sync::LazyLock;

    use super::*;

    fn strip(text: &str) -> String {
        static SGR: LazyLock<Regex> =
            LazyLock::new(|| Regex::new("\\x1b\\[[0-9;]*m").expect("valid SGR regex"));
        SGR.replace_all(text, "").into_owned()
    }

    fn image() -> ImageAttachment {
        ImageAttachment {
            id: 1,
            bytes: Arc::from([
                0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0, 0, 0, 0x0d, 0x49, 0x48, 0x44,
                0x52,
            ]),
            mime: "image/png".to_owned(),
            width: 2000,
            height: 1302,
            original: None,
            placeholder: "[image #1 (2000脳1302)]".to_owned(),
        }
    }

    #[test]
    fn renders_video_placeholders_as_plain_text() {
        let mut component = UserMessageComponent::with_image_protocol(
            "please inspect [video #1 sample.mov]",
            Vec::new(),
            None,
            None,
        );
        let output = strip(&component.render(80).join("\n"));
        assert!(output.contains("[video #1 sample.mov]"));
        assert!(!output.contains("\x1b_G"));
        assert!(!output.contains("\x1b]1337;File="));
    }

    #[test]
    fn keeps_text_lines_within_narrow_widths() {
        let mut component = UserMessageComponent::with_image_protocol(
            "please inspect the attached output",
            Vec::new(),
            None,
            None,
        );
        for width in [1, 2, 4, 10, 39] {
            assert!(
                component
                    .render(width)
                    .iter()
                    .all(|line| visible_width(line) <= width)
            );
        }
    }

    #[test]
    fn does_not_truncate_inline_image_sequences() {
        let mut component = UserMessageComponent::with_image_protocol(
            "",
            vec![image()],
            None,
            Some(InlineImageProtocol::Kitty),
        );
        let lines = component.render(80);
        let image_line = lines
            .iter()
            .find(|line| line.contains("\x1b_G"))
            .expect("Kitty image line");
        assert!(!image_line.contains("\x1b[0m"));
        assert!(!image_line.contains('…'));
        assert!(image_line.contains("\x1b\\"));
    }

    #[test]
    fn empty_bullet_places_text_at_the_leading_column() {
        let mut with_bullet =
            UserMessageComponent::with_image_protocol("hello", Vec::new(), None, None);
        assert!(strip(&with_bullet.render(80).join("\n")).contains("✦ "));

        let mut without_bullet = UserMessageComponent::with_image_protocol(
            "$ ls",
            Vec::new(),
            Some(String::new()),
            None,
        );
        let lines = without_bullet
            .render(80)
            .into_iter()
            .map(|line| strip(&line))
            .collect::<Vec<_>>();
        let content = lines
            .iter()
            .find(|line| line.contains("$ ls"))
            .expect("content line");
        assert!(content.starts_with("$ ls"));
        assert!(!lines.join("\n").contains('✦'));
    }

    #[test]
    fn invalidates_images_and_replay_boundary_stays_invisible() {
        let mut component =
            UserMessageComponent::with_image_protocol("hello", vec![image()], None, None);
        let first = component.render(80);
        assert_eq!(first, component.render(80));
        component.invalidate();
        assert!(component.render_cache.is_none());

        let mut boundary = ReplayTurnBoundaryComponent;
        assert!(boundary.render(80).is_empty());
    }
}
