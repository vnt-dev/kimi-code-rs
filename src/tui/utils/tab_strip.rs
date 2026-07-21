use unicode_width::UnicodeWidthStr;

use crate::tui::theme::colors::ColorPalette;

pub struct RenderTabStripOptions<'a> {
    pub labels: &'a [String],
    pub active_index: usize,
    pub width: usize,
    pub colors: &'a ColorPalette,
}

fn rgb_escape(hex: &str, background: bool) -> Option<String> {
    let digits = hex.strip_prefix('#')?;
    if digits.len() != 6 {
        return None;
    }
    let value = u32::from_str_radix(digits, 16).ok()?;
    let red = (value >> 16) & 0xff;
    let green = (value >> 8) & 0xff;
    let blue = value & 0xff;
    let layer = if background { 48 } else { 38 };
    Some(format!("\x1b[{layer};2;{red};{green};{blue}m"))
}

fn style_tab(label: &str, is_active: bool, colors: &ColorPalette) -> String {
    let cell = format!(" {label} ");
    let foreground = rgb_escape(
        if is_active {
            &colors.text
        } else {
            &colors.text_muted
        },
        false,
    )
    .unwrap_or_default();
    if !is_active {
        return format!("{foreground}{cell}\x1b[39m");
    }

    let background = rgb_escape(&colors.primary, true).unwrap_or_default();
    format!("{background}{foreground}\x1b[1m{cell}\x1b[22m\x1b[39m\x1b[49m")
}

fn style_marker(marker: &str, colors: &ColorPalette) -> String {
    let foreground = rgb_escape(&colors.text_muted, false).unwrap_or_default();
    format!("{foreground}{marker}\x1b[39m")
}

fn window_fits(start: usize, end: usize, content_width: usize, count: usize, width: usize) -> bool {
    let need_left = start > 0;
    let need_right = end < count;
    let frame_width = if need_left { 2 } else { 1 } + if need_right { 2 } else { 0 };
    let separators = end.saturating_sub(start).saturating_sub(1);
    content_width
        .saturating_add(separators)
        .saturating_add(frame_width)
        <= width
}

/// Original:
///   apps/kimi-code/src/tui/utils/tab-strip.ts
///   renderTabStrip()
pub fn render_tab_strip(options: &RenderTabStripOptions<'_>) -> String {
    let segments = options
        .labels
        .iter()
        .enumerate()
        .map(|(index, label)| style_tab(label, index == options.active_index, options.colors))
        .collect::<Vec<_>>();
    let segment_widths = options
        .labels
        .iter()
        .map(|label| UnicodeWidthStr::width(label.as_str()) + 2)
        .collect::<Vec<_>>();
    let total_segment_width = segment_widths.iter().sum::<usize>();
    let full_separator_width = segments.len().saturating_sub(1);
    if 1usize
        .saturating_add(total_segment_width)
        .saturating_add(full_separator_width)
        <= options.width
    {
        return format!(" {}", segments.join(" "));
    }
    if segments.is_empty() {
        return " ".to_owned();
    }

    // Valid callers always provide an in-range index. Clamping only protects
    // rendering from malformed state while retaining the original styling
    // decision above for the supplied index.
    let active_index = options.active_index.min(segments.len() - 1);
    let mut start = active_index;
    let mut end = active_index + 1;
    let mut content_width = segment_widths[active_index];

    loop {
        let left_width = start.checked_sub(1).map(|index| segment_widths[index]);
        let right_width = (end < segments.len()).then(|| segment_widths[end]);
        let expand_left_first = match (left_width, right_width) {
            (None, None) => break,
            (Some(_), None) => true,
            (None, Some(_)) => false,
            (Some(left), Some(right)) => left <= right,
        };

        let try_left = |start: usize, end: usize, content_width: usize| {
            left_width.filter(|left| {
                window_fits(
                    start - 1,
                    end,
                    content_width.saturating_add(*left),
                    segments.len(),
                    options.width,
                )
            })
        };
        let try_right = |start: usize, end: usize, content_width: usize| {
            right_width.filter(|right| {
                window_fits(
                    start,
                    end + 1,
                    content_width.saturating_add(*right),
                    segments.len(),
                    options.width,
                )
            })
        };

        if expand_left_first {
            if let Some(left) = try_left(start, end, content_width) {
                content_width += left;
                start -= 1;
            } else if let Some(right) = try_right(start, end, content_width) {
                content_width += right;
                end += 1;
            } else {
                break;
            }
        } else if let Some(right) = try_right(start, end, content_width) {
            content_width += right;
            end += 1;
        } else if let Some(left) = try_left(start, end, content_width) {
            content_width += left;
            start -= 1;
        } else {
            break;
        }
    }

    let mut strip = if start > 0 {
        style_marker("< ", options.colors)
    } else {
        " ".to_owned()
    };
    strip.push_str(&segments[start..end].join(" "));
    if end < segments.len() {
        strip.push_str(&style_marker(" >", options.colors));
    }
    strip
}

#[cfg(test)]
mod tests {
    use regex::Regex;

    use super::*;
    use crate::tui::theme::colors::dark_colors;

    fn strip_ansi(text: &str) -> String {
        Regex::new(r"\x1b\[[0-9;]*m")
            .map(|pattern| pattern.replace_all(text, "").into_owned())
            .unwrap_or_else(|_| text.to_owned())
    }

    fn render(labels: &[&str], width: usize, active_index: usize) -> String {
        let labels = labels
            .iter()
            .map(|label| (*label).to_owned())
            .collect::<Vec<_>>();
        strip_ansi(&render_tab_strip(&RenderTabStripOptions {
            labels: &labels,
            active_index,
            width,
            colors: &dark_colors(),
        }))
    }

    #[test]
    fn shows_full_strip_when_it_exactly_fits() {
        let output = render(&["Installed", "Official", "Third-party", "Custom"], 46, 0);

        assert!(output.contains("Installed"));
        assert!(output.contains("Custom"));
        assert!(!output.contains('<'));
        assert!(!output.contains('>'));
        assert!(output.ends_with(" Custom "));
    }

    #[test]
    fn scrolls_when_one_column_narrower_than_full_fit() {
        let output = render(&["Installed", "Official", "Third-party", "Custom"], 45, 0);

        assert!(output.contains('>'));
        assert!(!output.contains("Custom"));
    }

    #[test]
    fn keeps_active_tab_visible_and_frames_both_sides() {
        let output = render(&["One", "Two", "Three", "Four"], 12, 2);

        assert!(output.contains("Three"));
        assert!(output.contains('<'));
        assert!(output.contains('>'));
    }

    #[test]
    fn uses_display_width_for_wide_labels() {
        let output = render(&["安装", "Official"], 18, 0);
        assert_eq!(output, "  安装   Official ");
    }

    #[test]
    fn empty_strip_matches_original_leading_space() {
        assert_eq!(render(&[], 0, 0), " ");
    }
}
