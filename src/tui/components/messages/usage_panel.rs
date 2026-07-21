use std::any::Any;

pub use crate::oauth::managed_usage::{
    BoosterWalletInfo, ParsedManagedUsage as ManagedUsageReport, UsageRow as ManagedUsageRow,
};

use crate::{
    sdk::types::{SessionUsage, TokenUsage},
    tui::{
        components::{
            Component, ComponentRole,
            render::{truncate_to_width, visible_width},
        },
        theme::{ColorToken, current_theme},
    },
    utils::usage::usage_format::{
        RatioSeverity, format_token_count, ratio_severity, render_progress_bar, safe_usage_ratio,
        usage_percent,
    },
};

const LEFT_MARGIN: usize = 2;
const SIDE_PADDING: usize = 1;
const BOX_OVERHEAD: usize = LEFT_MARGIN + 2 + 2 * SIDE_PADDING;

type LineBuilder = dyn Fn() -> Vec<String> + Send;

#[derive(Debug, Clone, Copy)]
pub struct UsageReportOptions<'a> {
    pub session_usage: Option<&'a SessionUsage>,
    pub session_usage_error: Option<&'a str>,
    pub context_usage: f64,
    pub context_tokens: u64,
    pub max_context_tokens: u64,
    pub managed_usage: Option<&'a ManagedUsageReport>,
    pub managed_usage_error: Option<&'a str>,
}

fn usage_input_total(usage: &TokenUsage) -> u64 {
    usage
        .input_other
        .saturating_add(usage.input_cache_read)
        .saturating_add(usage.input_cache_creation)
}

fn build_session_usage_section(usage: Option<&SessionUsage>, error: Option<&str>) -> Vec<String> {
    let theme = current_theme();
    if let Some(error) = error {
        return vec![theme.fg(ColorToken::Error, &format!("  {error}"))];
    }
    let Some(entries) = usage.and_then(|usage| usage.by_model.as_ref()) else {
        return vec![theme.fg(ColorToken::TextDim, "  No token usage recorded yet.")];
    };
    if entries.is_empty() {
        return vec![theme.fg(ColorToken::TextDim, "  No token usage recorded yet.")];
    }

    let mut lines = Vec::new();
    let mut total_input = 0_u64;
    let mut total_output = 0_u64;
    for (model, usage) in entries {
        let input = usage_input_total(usage);
        let output = usage.output;
        total_input = total_input.saturating_add(input);
        total_output = total_output.saturating_add(output);
        lines.push(format!(
            "  {}  input {}  output {}  total {}",
            theme.fg(ColorToken::TextDim, model),
            theme.fg(ColorToken::Text, &format_token_count(input as f64)),
            theme.fg(ColorToken::Text, &format_token_count(output as f64)),
            theme.fg(
                ColorToken::Text,
                &format_token_count(input.saturating_add(output) as f64)
            )
        ));
    }
    if entries.len() > 1 {
        lines.push(format!(
            "  {}  input {}  output {}  total {}",
            theme.fg(ColorToken::TextDim, "total"),
            theme.fg(ColorToken::Text, &format_token_count(total_input as f64)),
            theme.fg(ColorToken::Text, &format_token_count(total_output as f64)),
            theme.fg(
                ColorToken::Text,
                &format_token_count(total_input.saturating_add(total_output) as f64)
            )
        ));
    }
    lines
}

fn severity_color(severity: RatioSeverity) -> ColorToken {
    match severity {
        RatioSeverity::Danger => ColorToken::Error,
        RatioSeverity::Warn => ColorToken::Warning,
        RatioSeverity::Ok => ColorToken::Success,
    }
}

fn build_managed_usage_section(
    usage: Option<&ManagedUsageReport>,
    error: Option<&str>,
) -> Vec<String> {
    let theme = current_theme();
    if let Some(error) = error {
        return vec![
            theme.bold_fg(ColorToken::Primary, "Plan usage"),
            theme.fg(ColorToken::Error, &format!("  {error}")),
        ];
    }
    let Some(usage) = usage else {
        return Vec::new();
    };
    if usage.summary.is_none() && usage.limits.is_empty() {
        return vec![
            theme.bold_fg(ColorToken::Primary, "Plan usage"),
            theme.fg(ColorToken::TextDim, "  No usage data available."),
        ];
    }
    let rows = usage
        .summary
        .iter()
        .chain(usage.limits.iter())
        .collect::<Vec<_>>();
    let used_ratio = |row: &ManagedUsageRow| {
        if row.limit > 0.0 {
            (row.used / row.limit).clamp(0.0, 1.0)
        } else {
            0.0
        }
    };
    let label_width = rows
        .iter()
        .map(|row| row.label.encode_utf16().count())
        .max()
        .unwrap_or(0)
        .max(10);
    let percentages = rows
        .iter()
        .map(|row| format!("{:.0}% used", used_ratio(row) * 100.0))
        .collect::<Vec<_>>();
    let percentage_width = percentages.iter().map(String::len).max().unwrap_or(0);
    let mut lines = vec![theme.bold_fg(ColorToken::Primary, "Plan usage")];
    for (row, percentage) in rows.into_iter().zip(percentages) {
        let ratio = used_ratio(row);
        let bar = render_progress_bar(ratio, 20, '█', '░');
        let label_padding = label_width.saturating_sub(row.label.encode_utf16().count());
        let label = format!("{}{}", row.label, " ".repeat(label_padding));
        let percentage = format!("{percentage:<percentage_width$}");
        let reset = row.reset_hint.as_deref().map_or_else(String::new, |hint| {
            format!("  {}", theme.fg(ColorToken::TextDim, hint))
        });
        lines.push(format!(
            "  {}  {}  {}{reset}",
            theme.fg(ColorToken::TextDim, &label),
            theme.fg(severity_color(ratio_severity(ratio)), &bar),
            theme.fg(ColorToken::Text, &percentage)
        ));
    }
    lines
}

pub fn build_managed_usage_report_lines(
    usage: Option<&ManagedUsageReport>,
    error: Option<&str>,
) -> Vec<String> {
    build_managed_usage_section(usage, error)
}

fn currency_symbol(currency: &str) -> &'static str {
    if currency.eq_ignore_ascii_case("CNY") {
        "¥"
    } else if currency.eq_ignore_ascii_case("USD") {
        "$"
    } else {
        ""
    }
}

fn format_currency_parts(cents: f64, currency: &str) -> (String, String) {
    let symbol = currency_symbol(currency);
    let formatted = format!("{:.2}", cents / 100.0);
    if symbol.is_empty() {
        (String::new(), format!("{formatted} {currency}"))
    } else {
        (symbol.to_owned(), formatted)
    }
}

pub fn build_extra_usage_section(extra: Option<&BoosterWalletInfo>) -> Vec<String> {
    let Some(extra) = extra else {
        return Vec::new();
    };
    let theme = current_theme();
    let has_monthly_limit =
        extra.monthly_charge_limit_enabled && extra.monthly_charge_limit_cents > 0.0;
    let balance = format_currency_parts(extra.balance_cents, &extra.currency);
    let used = format_currency_parts(extra.monthly_used_cents, &extra.currency);
    let mut rows = Vec::new();
    let mut bar_line = None;
    if has_monthly_limit {
        let ratio = (extra.monthly_used_cents / extra.monthly_charge_limit_cents).clamp(0.0, 1.0);
        let bar = render_progress_bar(ratio, 20, '█', '░');
        bar_line = Some(format!(
            "  {}",
            theme.fg(severity_color(ratio_severity(ratio)), &bar)
        ));
        rows.push(("Used this month", used));
        rows.push((
            "Monthly limit",
            format_currency_parts(extra.monthly_charge_limit_cents, &extra.currency),
        ));
        rows.push(("Balance", balance));
    } else {
        rows.push(("Used this month", used));
        rows.push(("Monthly limit", (String::new(), "Unlimited".to_owned())));
        rows.push(("Balance", balance));
    }
    let label_width = rows.iter().map(|(label, _)| label.len()).max().unwrap_or(0);
    let number_width = rows
        .iter()
        .filter(|(_, (symbol, _))| !symbol.is_empty())
        .map(|(_, (_, number))| visible_width(number))
        .max()
        .unwrap_or(0);
    let mut lines = vec![theme.bold_fg(ColorToken::Primary, "Extra Usage")];
    lines.extend(bar_line);
    for (label, (symbol, number)) in rows {
        let label = format!("{label:<label_width$}");
        let cell = if symbol.is_empty() {
            number
        } else {
            format!("{symbol}{number:>number_width$}")
        };
        lines.push(format!(
            "  {}  {}",
            theme.fg(ColorToken::TextDim, &label),
            theme.fg(ColorToken::Text, &cell)
        ));
    }
    lines
}

/// Original: usage-panel.ts buildUsageReportLines()
pub fn build_usage_report_lines(options: UsageReportOptions<'_>) -> Vec<String> {
    let theme = current_theme();
    let mut lines = vec![theme.bold_fg(ColorToken::Primary, "Session usage")];
    lines.extend(build_session_usage_section(
        options.session_usage,
        options.session_usage_error,
    ));
    if options.max_context_tokens > 0 {
        let ratio = safe_usage_ratio(options.context_usage);
        let bar = render_progress_bar(ratio, 20, '█', '░');
        let percentage = usage_percent(
            options.context_tokens as f64,
            options.max_context_tokens as f64,
        );
        let percentage = format!("{percentage}%");
        lines.push(String::new());
        lines.push(theme.bold_fg(ColorToken::Primary, "Context window"));
        lines.push(format!(
            "  {}  {}  {}",
            theme.fg(severity_color(ratio_severity(ratio)), &bar),
            theme.fg(ColorToken::Text, &format!("{percentage:>6}")),
            theme.fg(
                ColorToken::TextDim,
                &format!(
                    "({} / {})",
                    format_token_count(options.context_tokens as f64),
                    format_token_count(options.max_context_tokens as f64)
                )
            )
        ));
    }
    let managed =
        build_managed_usage_report_lines(options.managed_usage, options.managed_usage_error);
    if !managed.is_empty() {
        lines.push(String::new());
        lines.extend(managed);
    }
    let extra = build_extra_usage_section(
        options
            .managed_usage
            .and_then(|usage| usage.extra_usage.as_ref()),
    );
    if !extra.is_empty() {
        lines.push(String::new());
        lines.extend(extra);
    }
    lines
}

pub struct UsagePanelComponent {
    build_lines: Box<LineBuilder>,
    border_token: ColorToken,
    title: String,
    lines: Vec<String>,
}

impl UsagePanelComponent {
    pub fn new(
        build_lines: impl Fn() -> Vec<String> + Send + 'static,
        border_token: ColorToken,
        title: impl Into<String>,
    ) -> Self {
        let build_lines: Box<LineBuilder> = Box::new(build_lines);
        let lines = build_lines();
        Self {
            build_lines,
            border_token,
            title: title.into(),
            lines,
        }
    }

    pub fn usage(build_lines: impl Fn() -> Vec<String> + Send + 'static) -> Self {
        Self::new(build_lines, ColorToken::Primary, " Usage ")
    }
}

impl Component for UsagePanelComponent {
    /// Original: usage-panel.ts UsagePanelComponent.render()
    fn render(&mut self, width: usize) -> Vec<String> {
        if width == 0 {
            return vec![String::new()];
        }
        if width < BOX_OVERHEAD + 1 {
            let mut output = vec![truncate_to_width(self.title.trim(), width, "…", false)];
            output.extend(
                self.lines
                    .iter()
                    .map(|line| truncate_to_width(line, width, "…", false)),
            );
            return output;
        }

        let available_interior = width - BOX_OVERHEAD;
        let longest_line = self
            .lines
            .iter()
            .map(|line| visible_width(line))
            .max()
            .unwrap_or(0);
        let content_width = available_interior
            .min(longest_line.max(visible_width(&self.title)))
            .max(1);
        let horizontal_length = content_width + 2 * SIDE_PADDING;
        let title = truncate_to_width(&self.title, horizontal_length, "…", false);
        let trailing_dashes = horizontal_length.saturating_sub(visible_width(&title));
        let theme = current_theme();
        let paint = |text: &str| theme.fg(self.border_token, text);
        let indent = " ".repeat(LEFT_MARGIN);
        let mut output = vec![format!(
            "{indent}{}{}{}{}",
            paint("╭"),
            paint(&title),
            paint(&"─".repeat(trailing_dashes)),
            paint("╮")
        )];
        for line in &self.lines {
            let clipped = if visible_width(line) > content_width {
                truncate_to_width(line, content_width, "…", false)
            } else {
                line.clone()
            };
            let padding = content_width.saturating_sub(visible_width(&clipped));
            output.push(format!(
                "{indent}{} {clipped}{} {}",
                paint("│"),
                " ".repeat(padding),
                paint("│")
            ));
        }
        output.push(format!(
            "{indent}{}",
            paint(&format!("╰{}╯", "─".repeat(horizontal_length)))
        ));
        output
            .into_iter()
            .map(|line| truncate_to_width(&line, width, "…", false))
            .collect()
    }

    fn invalidate(&mut self) {
        self.lines = (self.build_lines)();
    }

    fn role(&self) -> ComponentRole {
        ComponentRole::Other
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    use super::*;

    fn strip_sgr(text: &str) -> String {
        let mut output = String::new();
        let mut escape = false;
        for character in text.chars() {
            if character == '\u{1b}' {
                escape = true;
            } else if escape && character == 'm' {
                escape = false;
            } else if !escape {
                output.push(character);
            }
        }
        output
    }

    #[test]
    fn wraps_lines_in_a_titled_bordered_panel() {
        let mut component = UsagePanelComponent::usage(|| vec!["Session usage".to_owned()]);
        let output = component
            .render(80)
            .iter()
            .map(|line| strip_sgr(line))
            .collect::<Vec<_>>();
        assert!(output[0].contains(" Usage "));
        assert!(output[1].contains("Session usage"));
        assert!(output.last().is_some_and(|line| line.contains('╰')));
    }

    #[test]
    fn truncates_long_lines_and_handles_every_narrow_width() {
        let mut component = UsagePanelComponent::usage(|| {
            vec![format!("error: {}", "x".repeat(200)), "second".to_owned()]
        });
        for width in [60, 39, 24, 20, 10, 4, 1, 0] {
            assert!(
                component
                    .render(width)
                    .iter()
                    .all(|line| visible_width(line) <= width)
            );
        }
    }

    #[test]
    fn invalidate_rebuilds_cached_body_lines() {
        let count = Arc::new(AtomicUsize::new(0));
        let captured = Arc::clone(&count);
        let mut component = UsagePanelComponent::usage(move || {
            vec![format!(
                "build={}",
                captured.fetch_add(1, Ordering::Relaxed)
            )]
        });
        assert!(
            component
                .render(80)
                .iter()
                .any(|line| strip_sgr(line).contains("build=0"))
        );
        component.invalidate();
        assert!(
            component
                .render(80)
                .iter()
                .any(|line| strip_sgr(line).contains("build=1"))
        );
        assert_eq!(count.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn formats_session_context_and_managed_usage_sections() {
        let usage = SessionUsage {
            by_model: Some(BTreeMap::from([(
                "kimi".to_owned(),
                TokenUsage {
                    input: 0,
                    input_cache_read: 500,
                    input_cache_creation: 500,
                    input_other: 1_000,
                    output: 250,
                },
            )])),
            current_turn: None,
            total: None,
        };
        let managed = ManagedUsageReport {
            summary: Some(ManagedUsageRow {
                label: "daily".to_owned(),
                used: 20.0,
                limit: 100.0,
                reset_hint: Some("resets tomorrow".to_owned()),
            }),
            limits: Vec::new(),
            extra_usage: None,
        };
        let plain = build_usage_report_lines(UsageReportOptions {
            session_usage: Some(&usage),
            session_usage_error: None,
            context_usage: 0.25,
            context_tokens: 2_500,
            max_context_tokens: 10_000,
            managed_usage: Some(&managed),
            managed_usage_error: None,
        })
        .iter()
        .map(|line| strip_sgr(line))
        .collect::<Vec<_>>();
        assert!(plain.contains(&"Session usage".to_owned()));
        assert!(plain.contains(&"  kimi  input 2k  output 250  total 2.2k".to_owned()));
        assert!(plain.contains(&"Context window".to_owned()));
        assert!(plain.join("\n").contains("25%"));
        assert!(plain.contains(&"Plan usage".to_owned()));
        assert!(plain.join("\n").contains("20% used"));
        assert!(plain.join("\n").contains("resets tomorrow"));
    }

    #[test]
    fn formats_extra_usage_limits_unlimited_and_currency_alignment() {
        let limited = BoosterWalletInfo {
            balance_cents: 15_901.0,
            total_cents: 300_000.0,
            monthly_charge_limit_enabled: true,
            monthly_charge_limit_cents: 300_000.0,
            monthly_used_cents: 24_099.0,
            currency: "CNY".to_owned(),
        };
        let limited = build_extra_usage_section(Some(&limited))
            .iter()
            .map(|line| strip_sgr(line))
            .collect::<Vec<_>>();
        assert!(limited.contains(&"Extra Usage".to_owned()));
        assert!(limited.join("\n").contains('░'));
        let currency_rows = limited
            .iter()
            .filter(|line| line.contains('¥'))
            .collect::<Vec<_>>();
        assert_eq!(currency_rows.len(), 3);
        assert_eq!(
            currency_rows
                .iter()
                .map(|line| line.find('¥'))
                .collect::<std::collections::BTreeSet<_>>()
                .len(),
            1
        );
        assert_eq!(
            currency_rows
                .iter()
                .map(|line| line.len())
                .collect::<std::collections::BTreeSet<_>>()
                .len(),
            1
        );

        let unlimited = BoosterWalletInfo {
            monthly_charge_limit_enabled: false,
            monthly_charge_limit_cents: 0.0,
            ..limited_wallet()
        };
        let unlimited = build_extra_usage_section(Some(&unlimited))
            .iter()
            .map(|line| strip_sgr(line))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(unlimited.contains("Unlimited"));
        assert!(!unlimited.contains('░'));
        assert!(!unlimited.contains('█'));
        assert!(build_extra_usage_section(None).is_empty());
    }

    fn limited_wallet() -> BoosterWalletInfo {
        BoosterWalletInfo {
            balance_cents: 18_208.0,
            total_cents: 40_000.0,
            monthly_charge_limit_enabled: true,
            monthly_charge_limit_cents: 40_000.0,
            monthly_used_cents: 21_792.0,
            currency: "CNY".to_owned(),
        }
    }
}
