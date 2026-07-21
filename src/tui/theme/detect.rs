use super::colors::ResolvedTheme;

/// COLORFGBG is `fg;bg` (and sometimes `fg;default;bg`). The last token is
/// the ANSI 16-color background index.
///
/// Original:
///   apps/kimi-code/src/tui/theme/detect.ts
///   parseColorFgBg()
pub fn parse_color_fg_bg(value: Option<&str>) -> Option<ResolvedTheme> {
    let value = value.filter(|value| !value.is_empty())?;
    let background = parse_javascript_integer(value.rsplit(';').next()?)?;
    if matches!(background, 0 | 1 | 2 | 3 | 4 | 5 | 6 | 8) {
        Some(ResolvedTheme::Dark)
    } else {
        Some(ResolvedTheme::Light)
    }
}

fn parse_javascript_integer(value: &str) -> Option<i64> {
    let value = value.trim_start();
    let (sign, digits) = match value.as_bytes().first() {
        Some(b'+') => (1_i64, &value[1..]),
        Some(b'-') => (-1_i64, &value[1..]),
        _ => (1_i64, value),
    };
    let digit_count = digits
        .as_bytes()
        .iter()
        .take_while(|byte| byte.is_ascii_digit())
        .count();
    if digit_count == 0 {
        return None;
    }
    digits[..digit_count]
        .parse::<i64>()
        .ok()
        .and_then(|number| number.checked_mul(sign))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_last_colorfgbg_token() {
        assert_eq!(parse_color_fg_bg(Some("15;0")), Some(ResolvedTheme::Dark));
        assert_eq!(
            parse_color_fg_bg(Some("15;default;7")),
            Some(ResolvedTheme::Light)
        );
        assert_eq!(parse_color_fg_bg(Some("15;8")), Some(ResolvedTheme::Dark));
        assert_eq!(parse_color_fg_bg(Some("15;15")), Some(ResolvedTheme::Light));
    }

    #[test]
    fn matches_javascript_parse_int_prefix_behavior() {
        assert_eq!(
            parse_color_fg_bg(Some("0; 6suffix")),
            Some(ResolvedTheme::Dark)
        );
        assert_eq!(parse_color_fg_bg(Some("0;-1")), Some(ResolvedTheme::Light));
    }

    #[test]
    fn rejects_missing_empty_and_non_numeric_values() {
        assert_eq!(parse_color_fg_bg(None), None);
        assert_eq!(parse_color_fg_bg(Some("")), None);
        assert_eq!(parse_color_fg_bg(Some("15;default")), None);
        assert_eq!(parse_color_fg_bg(Some("15;  ")), None);
    }
}
