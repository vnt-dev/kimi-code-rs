use std::sync::OnceLock;

use regex::Regex;

use super::colors::ResolvedTheme;

fn osc11_response_pattern() -> Option<&'static Regex> {
    static PATTERN: OnceLock<Option<Regex>> = OnceLock::new();
    PATTERN
        .get_or_init(|| {
            Regex::new(
                r"(?i)\x1b?\]11;rgb:([0-9a-f]{1,4})/([0-9a-f]{1,4})/([0-9a-f]{1,4})(?:\x07|\x1b\\)",
            )
            .ok()
        })
        .as_ref()
}

/// Original:
///   apps/kimi-code/src/tui/theme/terminal-background.ts
///   parseOsc11BackgroundTheme()
pub fn parse_osc11_background_theme(data: &str) -> Option<ResolvedTheme> {
    let captures = osc11_response_pattern()?.captures(data)?;
    Some(theme_from_hex_channels(
        captures.get(1)?.as_str(),
        captures.get(2)?.as_str(),
        captures.get(3)?.as_str(),
    ))
}

/// Original:
///   apps/kimi-code/src/tui/theme/terminal-background.ts
///   themeFromHexChannels()
pub fn theme_from_hex_channels(red: &str, green: &str, blue: &str) -> ResolvedTheme {
    let red = normalize_channel(red);
    let green = normalize_channel(green);
    let blue = normalize_channel(blue);
    let luminance = 0.2126 * red + 0.7152 * green + 0.0722 * blue;
    if luminance > 0.5 {
        ResolvedTheme::Light
    } else {
        ResolvedTheme::Dark
    }
}

fn normalize_channel(hex: &str) -> f64 {
    let bit_count = hex.len().saturating_mul(4);
    let maximum = 2_f64.powi(i32::try_from(bit_count).unwrap_or(i32::MAX)) - 1.0;
    u64::from_str_radix(hex, 16)
        .ok()
        .filter(|_| maximum > 0.0 && maximum.is_finite())
        .map(|value| value as f64 / maximum)
        .unwrap_or(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_bel_and_string_terminated_osc11_responses() {
        assert_eq!(
            parse_osc11_background_theme("prefix\u{1b}]11;rgb:00/00/00\u{7}suffix"),
            Some(ResolvedTheme::Dark)
        );
        assert_eq!(
            parse_osc11_background_theme("]11;rgb:ffff/ffff/ffff\u{1b}\\"),
            Some(ResolvedTheme::Light)
        );
    }

    #[test]
    fn requires_a_complete_response_and_valid_channel_widths() {
        for value in [
            "\u{1b}]11;rgb:ff/ff/ff",
            "\u{1b}]11;rgb:/ff/ff\u{7}",
            "\u{1b}]11;rgb:fffff/ff/ff\u{7}",
            "\u{1b}]10;rgb:ff/ff/ff\u{7}",
        ] {
            assert_eq!(parse_osc11_background_theme(value), None);
        }
    }

    #[test]
    fn normalizes_each_supported_channel_precision() {
        for white in ["f", "ff", "fff", "ffff"] {
            assert_eq!(
                theme_from_hex_channels(white, white, white),
                ResolvedTheme::Light
            );
        }
        for black in ["0", "00", "000", "0000"] {
            assert_eq!(
                theme_from_hex_channels(black, black, black),
                ResolvedTheme::Dark
            );
        }
    }

    #[test]
    fn luminance_threshold_weights_green_like_the_original() {
        assert_eq!(
            theme_from_hex_channels("00", "ff", "00"),
            ResolvedTheme::Light
        );
        assert_eq!(
            theme_from_hex_channels("ff", "00", "ff"),
            ResolvedTheme::Dark
        );
        assert_eq!(
            theme_from_hex_channels("invalid", "invalid", "invalid"),
            ResolvedTheme::Dark
        );
    }
}
