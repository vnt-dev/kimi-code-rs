use std::collections::BTreeMap;

use crate::{
    sdk::{
        model_alias::{ModelAlias, effective_model_alias},
        types::{PermissionMode, SessionStatus, ThinkingEffort},
    },
    tui::{
        components::messages::usage_panel::{
            ManagedUsageReport, build_extra_usage_section, build_managed_usage_report_lines,
        },
        theme::{ColorToken, current_theme},
    },
    utils::usage::usage_format::{
        RatioSeverity, format_token_count, ratio_severity, render_progress_bar, safe_usage_ratio,
        usage_percent,
    },
};

const PRODUCT_NAME: &str = "Kimi Code";

#[derive(Debug, Clone, Copy)]
pub struct StatusReportOptions<'a> {
    pub version: &'a str,
    pub model: &'a str,
    pub work_dir: &'a str,
    pub session_id: &'a str,
    pub session_title: Option<&'a str>,
    pub thinking_effort: &'a ThinkingEffort,
    pub permission_mode: PermissionMode,
    pub plan_mode: bool,
    pub context_usage: f64,
    pub context_tokens: u64,
    pub max_context_tokens: u64,
    pub available_models: &'a BTreeMap<String, ModelAlias>,
    pub status: Option<&'a SessionStatus>,
    pub status_error: Option<&'a str>,
    pub managed_usage: Option<&'a ManagedUsageReport>,
    pub managed_usage_error: Option<&'a str>,
}

struct FieldRow<'a> {
    label: &'static str,
    value: &'a str,
    is_error: bool,
}

fn display_model_name(alias: &str, models: &BTreeMap<String, ModelAlias>) -> String {
    let Some(model) = models.get(alias) else {
        return alias.to_owned();
    };
    let effective = effective_model_alias(model, None);
    effective.display_name.unwrap_or(effective.model)
}

fn format_model_status(options: StatusReportOptions<'_>) -> String {
    let model = options
        .status
        .and_then(|status| status.model.as_deref())
        .unwrap_or(options.model);
    if model.trim().is_empty() {
        return "not set".to_owned();
    }
    let effort = options.status.map_or_else(
        || options.thinking_effort.as_str(),
        |status| status.thinking_effort.as_str(),
    );
    format!(
        "{} (thinking {effort})",
        display_model_name(model, options.available_models)
    )
}

fn permission_name(permission: PermissionMode) -> &'static str {
    match permission {
        PermissionMode::Manual => "manual",
        PermissionMode::Yolo => "yolo",
        PermissionMode::Auto => "auto",
    }
}

fn severity_color(severity: RatioSeverity) -> ColorToken {
    match severity {
        RatioSeverity::Danger => ColorToken::Error,
        RatioSeverity::Warn => ColorToken::Warning,
        RatioSeverity::Ok => ColorToken::Success,
    }
}

// Original:
//   apps/kimi-code/src/tui/components/messages/status-panel.ts
//   buildStatusReportLines()
pub fn build_status_report_lines(options: StatusReportOptions<'_>) -> Vec<String> {
    let theme = current_theme();
    let model = format_model_status(options);
    let permission = options
        .status
        .map_or(options.permission_mode, |status| status.permission);
    let plan_mode = options
        .status
        .map_or(options.plan_mode, |status| status.plan_mode);
    let session_id = if options.session_id.trim().is_empty() {
        "none"
    } else {
        options.session_id
    };
    let permission = permission_name(permission);
    let plan_mode = if plan_mode { "on" } else { "off" };
    let mut rows = vec![
        FieldRow {
            label: "Model",
            value: &model,
            is_error: false,
        },
        FieldRow {
            label: "Directory",
            value: options.work_dir,
            is_error: false,
        },
        FieldRow {
            label: "Permissions",
            value: permission,
            is_error: false,
        },
        FieldRow {
            label: "Plan mode",
            value: plan_mode,
            is_error: false,
        },
        FieldRow {
            label: "Session",
            value: session_id,
            is_error: false,
        },
    ];
    let title = options
        .session_title
        .map(str::trim)
        .filter(|title| !title.is_empty());
    if let Some(title) = title {
        rows.push(FieldRow {
            label: "Title",
            value: title,
            is_error: false,
        });
    }
    if let Some(error) = options.status_error {
        rows.push(FieldRow {
            label: "Warning",
            value: error,
            is_error: true,
        });
    }

    let mut lines = vec![
        format!(
            "{} {}",
            theme.bold_fg(ColorToken::Primary, &format!(">_ {PRODUCT_NAME}")),
            theme.fg(ColorToken::TextDim, &format!("(v{})", options.version))
        ),
        String::new(),
    ];
    let label_width = rows
        .iter()
        .map(|row| row.label.len())
        .max()
        .unwrap_or(0)
        .max(10);
    for row in rows {
        let label = format!("{:<label_width$}", row.label);
        let value = if row.is_error {
            theme.fg(ColorToken::Error, row.value)
        } else {
            theme.fg(ColorToken::Text, row.value)
        };
        lines.push(format!(
            "  {}  {value}",
            theme.fg(ColorToken::TextDim, &label)
        ));
    }

    let (ratio, tokens, max_tokens) = options.status.map_or(
        (
            options.context_usage,
            options.context_tokens,
            options.max_context_tokens,
        ),
        |status| {
            (
                status.context_usage,
                status.context_tokens,
                status.max_context_tokens,
            )
        },
    );
    lines.push(String::new());
    lines.push(theme.bold_fg(ColorToken::Primary, "Context window"));
    if max_tokens > 0 {
        let safe_ratio = safe_usage_ratio(ratio);
        let bar = render_progress_bar(safe_ratio, 20, '█', '░');
        let percentage = format!("{}%", usage_percent(tokens as f64, max_tokens as f64));
        lines.push(format!(
            "  {}  {}  {}",
            theme.fg(severity_color(ratio_severity(safe_ratio)), &bar),
            theme.fg(ColorToken::Text, &format!("{percentage:>6}")),
            theme.fg(
                ColorToken::TextDim,
                &format!(
                    "({} / {})",
                    format_token_count(tokens as f64),
                    format_token_count(max_tokens as f64)
                )
            )
        ));
    } else {
        lines.push(theme.fg(ColorToken::TextDim, "  No context window data available."));
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

#[cfg(test)]
mod tests {
    use crate::{
        sdk::{
            model_alias::{ModelAlias, ModelProtocol},
            types::{PermissionMode, SessionStatus, ThinkingEffort},
        },
        tui::components::messages::usage_panel::{
            BoosterWalletInfo, ManagedUsageReport, ManagedUsageRow,
        },
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

    fn model_alias() -> ModelAlias {
        ModelAlias {
            provider: "managed:kimi-code".to_owned(),
            model: "kimi-k2".to_owned(),
            max_context_size: 10_000,
            max_output_size: None,
            capabilities: None,
            display_name: Some("Kimi K2".to_owned()),
            reasoning_key: None,
            protocol: None::<ModelProtocol>,
            adaptive_thinking: None,
            support_efforts: None,
            default_effort: None,
            beta_api: None,
            overrides: None,
        }
    }

    fn plain(lines: Vec<String>) -> String {
        lines
            .iter()
            .map(|line| strip_sgr(line))
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn formats_runtime_status_context_and_managed_usage() {
        let models = BTreeMap::from([("k2".to_owned(), model_alias())]);
        let status = SessionStatus {
            model: Some("k2".to_owned()),
            thinking_effort: "high".to_owned(),
            permission: PermissionMode::Auto,
            plan_mode: true,
            swarm_mode: None,
            context_tokens: 3_000,
            max_context_tokens: 12_000,
            context_usage: 0.25,
            usage: None,
        };
        let managed = ManagedUsageReport {
            summary: None,
            limits: vec![ManagedUsageRow {
                label: "5h limit".to_owned(),
                used: 8.0,
                limit: 100.0,
                reset_hint: Some("resets in 1h".to_owned()),
            }],
            extra_usage: None,
        };
        let output = plain(build_status_report_lines(StatusReportOptions {
            version: "1.2.3",
            model: "k2",
            work_dir: "/tmp/project",
            session_id: "ses-1",
            session_title: Some("Implement status"),
            thinking_effort: &ThinkingEffort::from("on"),
            permission_mode: PermissionMode::Manual,
            plan_mode: false,
            context_usage: 0.25,
            context_tokens: 2_500,
            max_context_tokens: 10_000,
            available_models: &models,
            status: Some(&status),
            status_error: None,
            managed_usage: Some(&managed),
            managed_usage_error: None,
        }));
        for expected in [
            ">_ Kimi Code (v1.2.3)",
            "Model        Kimi K2 (thinking high)",
            "Directory    /tmp/project",
            "Permissions  auto",
            "Plan mode    on",
            "Session      ses-1",
            "Title        Implement status",
            "Context window",
            "25%",
            "(2.9k / 11.7k)",
            "Plan usage",
            "8% used",
        ] {
            assert!(
                output.contains(expected),
                "missing {expected:?} in {output:?}"
            );
        }
        for excluded in ["Account", "AGENTS.md", "Runtime"] {
            assert!(!output.contains(excluded));
        }
    }

    #[test]
    fn formats_extra_usage_section() {
        let managed = ManagedUsageReport {
            summary: None,
            limits: Vec::new(),
            extra_usage: Some(BoosterWalletInfo {
                balance_cents: 15_000.0,
                total_cents: 20_000.0,
                monthly_charge_limit_enabled: true,
                monthly_charge_limit_cents: 20_000.0,
                monthly_used_cents: 5_000.0,
                currency: "USD".to_owned(),
            }),
        };
        let models = BTreeMap::new();
        let output = plain(build_status_report_lines(StatusReportOptions {
            version: "1.2.3",
            model: "k2",
            work_dir: "/tmp/project",
            session_id: "ses-1",
            session_title: None,
            thinking_effort: &ThinkingEffort::from("off"),
            permission_mode: PermissionMode::Manual,
            plan_mode: false,
            context_usage: 0.0,
            context_tokens: 0,
            max_context_tokens: 0,
            available_models: &models,
            status: None,
            status_error: None,
            managed_usage: Some(&managed),
            managed_usage_error: None,
        }));
        for expected in [
            "Extra Usage",
            "Balance",
            "150.00",
            "Used this month",
            "50.00",
            "Monthly limit",
            "200.00",
        ] {
            assert!(output.contains(expected));
        }
    }

    #[test]
    fn falls_back_to_app_state_and_displays_load_errors() {
        let models = BTreeMap::new();
        let output = plain(build_status_report_lines(StatusReportOptions {
            version: "1.2.3",
            model: "",
            work_dir: "/tmp/project",
            session_id: "",
            session_title: None,
            thinking_effort: &ThinkingEffort::from("off"),
            permission_mode: PermissionMode::Manual,
            plan_mode: false,
            context_usage: 0.0,
            context_tokens: 0,
            max_context_tokens: 0,
            available_models: &models,
            status: None,
            status_error: Some("No active session"),
            managed_usage: None,
            managed_usage_error: None,
        }));
        for expected in [
            "Model        not set",
            "Session      none",
            "Warning      No active session",
            "No context window data available.",
        ] {
            assert!(output.contains(expected));
        }
    }
}
