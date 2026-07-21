/// Semantic severity used to style a usage ratio.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RatioSeverity {
    Ok,
    Warn,
    Danger,
}

// Original:
//   apps/kimi-code/src/utils/usage/usage-format.ts
//   formatTokenCount()
pub fn format_token_count(count: f64) -> String {
    if !count.is_finite() || count < 0.0 {
        return "0".to_owned();
    }
    if count >= 1024.0 * 1024.0 {
        return format!("{}M", trim_decimal(count / (1024.0 * 1024.0)));
    }
    if count >= 1024.0 {
        let thousands = count / 1024.0;
        let value = if thousands >= 100.0 {
            format!("{:.0}", thousands)
        } else {
            trim_decimal(thousands)
        };
        return format!("{value}k");
    }
    if count == 0.0 {
        "0".to_owned()
    } else {
        count.to_string()
    }
}

fn trim_decimal(value: f64) -> String {
    let formatted = format!("{value:.1}");
    formatted
        .strip_suffix(".0")
        .unwrap_or(&formatted)
        .to_owned()
}

// Original:
//   apps/kimi-code/src/utils/usage/usage-format.ts
//   usagePercent()
//
// Returning `f64` retains JavaScript's observable `NaN` result when `used` is
// non-finite and `max` itself is valid.
pub fn usage_percent(used: f64, max: f64) -> f64 {
    if !max.is_finite() || max <= 0.0 {
        return 0.0;
    }
    let percent = (used / max * 100.0).ceil();
    if percent.is_nan() {
        f64::NAN
    } else {
        percent.clamp(0.0, 100.0)
    }
}

// Original:
//   apps/kimi-code/src/utils/usage/usage-format.ts
//   usagePercentFromRatio()
pub fn usage_percent_from_ratio(ratio: f64) -> u8 {
    (safe_usage_ratio(ratio) * 100.0).ceil().clamp(0.0, 100.0) as u8
}

// Original:
//   apps/kimi-code/src/utils/usage/usage-format.ts
//   renderProgressBar()
//
// Rust has no default arguments, so callers pass the width and glyphs
// explicitly. `render_default_progress_bar` supplies the TypeScript defaults.
pub fn render_progress_bar(ratio: f64, width: usize, filled: char, empty: char) -> String {
    let filled_count = (safe_usage_ratio(ratio) * width as f64).round() as usize;
    let empty_count = width.saturating_sub(filled_count);
    filled.to_string().repeat(filled_count) + &empty.to_string().repeat(empty_count)
}

pub fn render_default_progress_bar(ratio: f64) -> String {
    render_progress_bar(ratio, 20, '█', '░')
}

// Original:
//   apps/kimi-code/src/utils/usage/usage-format.ts
//   safeUsageRatio()
pub fn safe_usage_ratio(ratio: f64) -> f64 {
    if ratio.is_finite() {
        ratio.clamp(0.0, 1.0)
    } else {
        0.0
    }
}

// Original:
//   apps/kimi-code/src/utils/usage/usage-format.ts
//   ratioSeverity()
pub fn ratio_severity(ratio: f64) -> RatioSeverity {
    if ratio >= 0.85 {
        RatioSeverity::Danger
    } else if ratio >= 0.5 {
        RatioSeverity::Warn
    } else {
        RatioSeverity::Ok
    }
}

#[cfg(test)]
mod tests {
    use super::{
        RatioSeverity, format_token_count, ratio_severity, render_progress_bar, safe_usage_ratio,
        usage_percent, usage_percent_from_ratio,
    };

    #[test]
    fn formats_token_counts_in_binary_units() {
        assert_eq!(format_token_count(0.0), "0");
        assert_eq!(format_token_count(1.0), "1");
        assert_eq!(format_token_count(999.0), "999");
        assert_eq!(format_token_count(1_000.0), "1000");
        assert_eq!(format_token_count(1_024.0), "1k");
        assert_eq!(format_token_count(1_536.0), "1.5k");
        assert_eq!(format_token_count(2_048.0), "2k");
        assert_eq!(format_token_count(50_552.0), "49.4k");
        assert_eq!(format_token_count(262_144.0), "256k");
        assert_eq!(format_token_count(102_400.0), "100k");
        assert_eq!(format_token_count(999_999.0), "977k");
        assert_eq!(format_token_count(1_048_576.0), "1M");
        assert_eq!(format_token_count(1_572_864.0), "1.5M");
        assert_eq!(format_token_count(10_485_760.0), "10M");
    }

    #[test]
    fn invalid_token_counts_are_zero() {
        assert_eq!(format_token_count(-1.0), "0");
        assert_eq!(format_token_count(f64::NAN), "0");
        assert_eq!(format_token_count(f64::INFINITY), "0");
        assert_eq!(format_token_count(-0.0), "0");
    }

    #[test]
    fn calculates_usage_percent_with_original_clamping() {
        assert_eq!(usage_percent(0.0, 1_000.0), 0.0);
        assert_eq!(usage_percent(4.0, 10_000.0), 1.0);
        assert_eq!(usage_percent(427.0, 1_000.0), 43.0);
        assert_eq!(usage_percent(992.0, 1_000.0), 100.0);
        assert_eq!(usage_percent(1_000.0, 1_000.0), 100.0);
        assert_eq!(usage_percent(1_200.0, 1_000.0), 100.0);
        assert_eq!(usage_percent(500.0, 0.0), 0.0);
        assert_eq!(usage_percent(500.0, -1.0), 0.0);
        assert_eq!(usage_percent(500.0, f64::NAN), 0.0);
        assert!(usage_percent(f64::NAN, 1_000.0).is_nan());
    }

    #[test]
    fn calculates_percent_from_a_safe_ratio() {
        assert_eq!(usage_percent_from_ratio(f64::NAN), 0);
        assert_eq!(usage_percent_from_ratio(0.0), 0);
        assert_eq!(usage_percent_from_ratio(0.004), 1);
        assert_eq!(usage_percent_from_ratio(0.427), 43);
        assert_eq!(usage_percent_from_ratio(1.5), 100);
    }

    #[test]
    fn renders_progress_bars() {
        assert_eq!(render_progress_bar(0.0, 10, '█', '░'), "░".repeat(10));
        assert_eq!(render_progress_bar(1.0, 10, '█', '░'), "█".repeat(10));
        assert_eq!(
            render_progress_bar(0.5, 10, '█', '░'),
            "█".repeat(5) + &"░".repeat(5)
        );
        assert_eq!(render_progress_bar(-1.0, 8, '█', '░'), "░".repeat(8));
        assert_eq!(render_progress_bar(2.0, 8, '█', '░'), "█".repeat(8));
        assert_eq!(render_progress_bar(f64::NAN, 6, '█', '░'), "░".repeat(6));
    }

    #[test]
    fn clamps_safe_usage_ratios() {
        assert_eq!(safe_usage_ratio(f64::NAN), 0.0);
        assert_eq!(safe_usage_ratio(-1.0), 0.0);
        assert_eq!(safe_usage_ratio(0.427), 0.427);
        assert_eq!(safe_usage_ratio(1.5), 1.0);
    }

    #[test]
    fn classifies_ratio_severity() {
        assert_eq!(ratio_severity(0.0), RatioSeverity::Ok);
        assert_eq!(ratio_severity(0.49), RatioSeverity::Ok);
        assert_eq!(ratio_severity(0.5), RatioSeverity::Warn);
        assert_eq!(ratio_severity(0.7), RatioSeverity::Warn);
        assert_eq!(ratio_severity(0.849), RatioSeverity::Warn);
        assert_eq!(ratio_severity(0.85), RatioSeverity::Danger);
        assert_eq!(ratio_severity(1.0), RatioSeverity::Danger);
    }
}
