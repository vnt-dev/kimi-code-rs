use std::{
    any::Any,
    collections::BTreeMap,
    env,
    sync::{
        Arc,
        mpsc::{self, Sender},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use crate::{
    sdk::{
        model_alias::{ModelAlias, effective_model_alias},
        types::{GoalSnapshot, GoalStatus, PermissionMode, ThinkingEffort},
    },
    tui::{
        components::{
            Component, ComponentRole,
            render::{truncate_to_width, visible_width},
        },
        theme::{ColorToken, current_theme},
    },
    utils::{
        git::git_status::{
            FormatGitBadgeOptions, GitStatus, GitStatusCache, format_git_badge_base,
            format_pull_request_badge,
        },
        usage::usage_format::{format_token_count, usage_percent, usage_percent_from_ratio},
    },
};

use super::working_tips::{ALL_TIPS, ToolbarTip, build_weighted_tips};

const MAX_CWD_SEGMENTS: usize = 3;
const GOAL_TIMER_INTERVAL: Duration = Duration::from_secs(1);
const TIP_ROTATE_INTERVAL_MS: u128 = 10_000;
const TIP_SEPARATOR: &str = " | ";

#[derive(Debug, Clone)]
pub struct FooterState {
    pub model: String,
    pub work_dir: String,
    pub permission_mode: PermissionMode,
    pub plan_mode: bool,
    pub swarm_mode: bool,
    pub thinking_effort: ThinkingEffort,
    pub context_usage: f64,
    pub context_tokens: Option<u64>,
    pub max_context_tokens: Option<u64>,
    pub available_models: BTreeMap<String, ModelAlias>,
    pub goal: Option<GoalSnapshot>,
}

pub struct FooterComponent {
    state: FooterState,
    on_refresh: Arc<dyn Fn() + Send + Sync>,
    git_cache: GitStatusCache,
    git_cache_work_dir: String,
    transient_hint: Option<String>,
    goal_snapshot_key: Option<String>,
    goal_observed_at: Instant,
    goal_timer_stop: Option<Sender<()>>,
    goal_timer: Option<JoinHandle<()>>,
    background_bash_task_count: usize,
    background_agent_count: usize,
}

impl FooterComponent {
    pub fn new(state: FooterState, on_refresh: Arc<dyn Fn() + Send + Sync>) -> Self {
        let work_dir = state.work_dir.clone();
        let git_cache = GitStatusCache::new(&work_dir, Some(Arc::clone(&on_refresh)));
        let mut footer = Self {
            state,
            on_refresh,
            git_cache,
            git_cache_work_dir: work_dir,
            transient_hint: None,
            goal_snapshot_key: None,
            goal_observed_at: Instant::now(),
            goal_timer_stop: None,
            goal_timer: None,
            background_bash_task_count: 0,
            background_agent_count: 0,
        };
        footer.sync_goal_clock();
        footer.sync_goal_timer();
        footer
    }

    // Original: FooterComponent.setState()
    pub fn set_state(&mut self, state: FooterState) {
        if state.work_dir != self.git_cache_work_dir {
            self.git_cache_work_dir.clone_from(&state.work_dir);
            self.git_cache =
                GitStatusCache::new(&state.work_dir, Some(Arc::clone(&self.on_refresh)));
        }
        self.state = state;
        self.sync_goal_clock();
        self.sync_goal_timer();
    }

    pub fn set_transient_hint(&mut self, hint: Option<String>) {
        self.transient_hint = hint;
    }

    pub fn transient_hint(&self) -> Option<&str> {
        self.transient_hint.as_deref()
    }

    pub fn set_background_counts(&mut self, bash_tasks: isize, agent_tasks: isize) {
        self.background_bash_task_count = bash_tasks.max(0) as usize;
        self.background_agent_count = agent_tasks.max(0) as usize;
    }

    pub fn dispose(&mut self) {
        self.stop_goal_timer();
    }

    fn render_footer(&mut self, width: usize) -> Vec<String> {
        let width = width.max(1);
        let theme = current_theme();
        let mut left = Vec::new();
        let mut modes = Vec::new();
        match self.state.permission_mode {
            PermissionMode::Auto => modes.push(theme.bold_fg(ColorToken::Warning, "auto")),
            PermissionMode::Yolo => modes.push(theme.bold_fg(ColorToken::Warning, "yolo")),
            PermissionMode::Manual => {}
        }
        if self.state.plan_mode {
            modes.push(theme.bold_fg(ColorToken::Primary, "plan"));
        }
        if self.state.swarm_mode {
            modes.push(theme.bold_fg(ColorToken::Accent, "swarm"));
        }
        if !modes.is_empty() {
            left.push(modes.join(" "));
        }
        if let Some(goal) = format_goal_badge(self.state.goal.as_ref(), self.goal_wall_clock_ms()) {
            left.push(goal);
        }
        if let Some(model) = model_display_name(&self.state) {
            let effective = self
                .state
                .available_models
                .get(&self.state.model)
                .map(|model| effective_model_alias(model, None));
            let effort = self.state.thinking_effort.as_str();
            let has_efforts = effective
                .as_ref()
                .and_then(|model| model.support_efforts.as_ref())
                .is_some_and(|efforts| !efforts.is_empty());
            let suffix = if effort == "off" {
                String::new()
            } else if has_efforts && effort != "on" {
                format!(" thinking: {effort}")
            } else {
                " thinking".to_owned()
            };
            left.push(theme.fg(ColorToken::Text, &format!("{model}{suffix}")));
        }
        push_background_badge(&mut left, self.background_bash_task_count, "task", "tasks");
        push_background_badge(&mut left, self.background_agent_count, "agent", "agents");
        let cwd = shorten_cwd(&self.state.work_dir);
        if !cwd.is_empty() {
            left.push(theme.fg(ColorToken::TextDim, &cwd));
        }
        if let Some(git) = self.git_cache.get_status() {
            left.push(format_footer_git_badge(&git));
        }
        let left_line = left.join("  ");
        let left_width = visible_width(&left_line);
        let (primary, pair) = tips_for_index(current_tip_index());
        let remaining = width.saturating_sub(left_width + 2);
        let tip = pair
            .as_deref()
            .filter(|tip| visible_width(tip) <= remaining)
            .or_else(|| {
                (!primary.is_empty() && visible_width(&primary) <= remaining)
                    .then_some(primary.as_str())
            });
        let line1 = if let Some(tip) = tip {
            let padding = width.saturating_sub(left_width + visible_width(tip));
            format!(
                "{left_line}{}{}",
                " ".repeat(padding),
                theme.fg(ColorToken::TextMuted, tip)
            )
        } else {
            truncate_to_width(&left_line, width, "…", false)
        };

        let context = format_context_status(
            self.state.context_usage,
            self.state.context_tokens,
            self.state.max_context_tokens,
        );
        let context_width = visible_width(&context);
        let line2 = if let Some(hint) = &self.transient_hint {
            let max_hint_width = width.saturating_sub(context_width + 1);
            let hint = truncate_to_width(hint, max_hint_width, "…", false);
            let padding = width.saturating_sub(visible_width(&hint) + context_width);
            format!(
                "{}{}{}",
                theme.bold_fg(ColorToken::Warning, &hint),
                " ".repeat(padding),
                theme.fg(ColorToken::Text, &context)
            )
        } else {
            format!(
                "{}{}",
                " ".repeat(width.saturating_sub(context_width)),
                theme.fg(ColorToken::Text, &context)
            )
        };
        vec![
            truncate_to_width(&line1, width, "", false),
            truncate_to_width(&line2, width, "", false),
        ]
    }

    fn sync_goal_clock(&mut self) {
        let key = goal_snapshot_key(self.state.goal.as_ref());
        if key != self.goal_snapshot_key {
            self.goal_snapshot_key = key;
            self.goal_observed_at = Instant::now();
        }
    }

    fn sync_goal_timer(&mut self) {
        if self.state.goal.as_ref().map(|goal| goal.status) == Some(GoalStatus::Active) {
            if self.goal_timer.is_some() {
                return;
            }
            let callback = Arc::clone(&self.on_refresh);
            let (sender, receiver) = mpsc::channel();
            self.goal_timer_stop = Some(sender);
            self.goal_timer = Some(thread::spawn(move || {
                while receiver.recv_timeout(GOAL_TIMER_INTERVAL).is_err() {
                    callback();
                }
            }));
        } else {
            self.stop_goal_timer();
        }
    }

    fn stop_goal_timer(&mut self) {
        if let Some(sender) = self.goal_timer_stop.take() {
            let _ = sender.send(());
        }
        if let Some(timer) = self.goal_timer.take() {
            let _ = timer.join();
        }
    }

    fn goal_wall_clock_ms(&self) -> Option<u64> {
        let goal = self.state.goal.as_ref()?;
        if goal.status == GoalStatus::Active {
            Some(
                goal.wall_clock_ms.saturating_add(
                    self.goal_observed_at
                        .elapsed()
                        .as_millis()
                        .min(u128::from(u64::MAX)) as u64,
                ),
            )
        } else {
            Some(goal.wall_clock_ms)
        }
    }
}

impl Drop for FooterComponent {
    fn drop(&mut self) {
        self.stop_goal_timer();
    }
}

impl Component for FooterComponent {
    fn render(&mut self, width: usize) -> Vec<String> {
        self.render_footer(width)
    }

    fn invalidate(&mut self) {}

    fn role(&self) -> ComponentRole {
        ComponentRole::Other
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

fn push_background_badge(lines: &mut Vec<String>, count: usize, one: &str, many: &str) {
    if count == 0 {
        return;
    }
    let noun = if count == 1 { one } else { many };
    lines.push(current_theme().fg(ColorToken::Primary, &format!("[{count} {noun} running]")));
}

fn format_goal_badge(goal: Option<&GoalSnapshot>, wall_clock_ms: Option<u64>) -> Option<String> {
    let goal = goal?;
    if !matches!(
        goal.status,
        GoalStatus::Active | GoalStatus::Paused | GoalStatus::Blocked
    ) {
        return None;
    }
    let theme = current_theme();
    let dot_color = match goal.status {
        GoalStatus::Active => ColorToken::Primary,
        GoalStatus::Blocked => ColorToken::Warning,
        GoalStatus::Paused => ColorToken::TextMuted,
        GoalStatus::Complete => return None,
    };
    let turns = goal.budget.turn_budget.map_or_else(
        || {
            format!(
                "{} {}",
                goal.turns_used,
                if goal.turns_used == 1 {
                    "turn"
                } else {
                    "turns"
                }
            )
        },
        |budget| format!("{}/{} turns", goal.turns_used, budget),
    );
    Some(format!(
        "{}{}{}",
        theme.fg(ColorToken::TextMuted, "[goal "),
        theme.fg(dot_color, "●"),
        theme.fg(
            ColorToken::TextMuted,
            &format!(
                " {} · {} · {}]",
                goal.status.as_str(),
                format_badge_elapsed(wall_clock_ms.unwrap_or(goal.wall_clock_ms)),
                turns
            )
        )
    ))
}

fn format_badge_elapsed(ms: u64) -> String {
    let seconds = (ms as f64 / 1_000.0).round() as u64;
    if seconds < 60 {
        format!("{seconds}s")
    } else {
        let minutes = seconds / 60;
        if minutes < 60 {
            format!("{minutes}m")
        } else {
            format!("{}h{}m", minutes / 60, minutes % 60)
        }
    }
}

fn model_display_name(state: &FooterState) -> Option<String> {
    if state.model.is_empty() {
        return None;
    }
    Some(state.available_models.get(&state.model).map_or_else(
        || state.model.clone(),
        |model| {
            let effective = effective_model_alias(model, None);
            effective.display_name.unwrap_or(effective.model)
        },
    ))
}

fn shorten_cwd(path: &str) -> String {
    if path.is_empty() {
        return String::new();
    }
    let normalized = path.replace('\\', "/");
    let home = env::var("HOME").unwrap_or_default().replace('\\', "/");
    let work = if !home.is_empty() && normalized == home {
        "~".to_owned()
    } else if !home.is_empty() && normalized.starts_with(&(home.clone() + "/")) {
        format!("~{}", &normalized[home.len()..])
    } else {
        normalized
    };
    let segments = work
        .split('/')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    if segments.len() <= MAX_CWD_SEGMENTS {
        work
    } else {
        format!(
            "…/{}",
            segments[segments.len() - MAX_CWD_SEGMENTS..].join("/")
        )
    }
}

fn format_context_status(usage: f64, tokens: Option<u64>, max_tokens: Option<u64>) -> String {
    if let (Some(tokens), Some(max_tokens)) = (tokens, max_tokens)
        && max_tokens > 0
    {
        return format!(
            "context: {:.0}% ({}/{})",
            usage_percent(tokens as f64, max_tokens as f64),
            format_token_count(tokens as f64),
            format_token_count(max_tokens as f64)
        );
    }
    format!("context: {}%", usage_percent_from_ratio(usage))
}

fn format_footer_git_badge(status: &GitStatus) -> String {
    let theme = current_theme();
    let base = theme.fg(ColorToken::TextDim, &format_git_badge_base(status));
    status
        .pull_request
        .as_ref()
        .map_or(base.clone(), |pull_request| {
            format!(
                "{base} {}",
                theme.fg(
                    ColorToken::Primary,
                    &format_pull_request_badge(
                        pull_request,
                        FormatGitBadgeOptions {
                            link_pull_request: true
                        }
                    )
                )
            )
        })
}

fn tips_for_index(index: i128) -> (String, Option<String>) {
    static ROTATION: std::sync::LazyLock<Vec<ToolbarTip>> =
        std::sync::LazyLock::new(|| build_weighted_tips(&ALL_TIPS));
    if ROTATION.is_empty() {
        return (String::new(), None);
    }
    let offset = index.rem_euclid(ROTATION.len() as i128) as usize;
    let current = ROTATION[offset];
    if ROTATION.len() == 1 || current.solo {
        return (current.text.to_owned(), None);
    }
    let next = ROTATION[(offset + 1) % ROTATION.len()];
    if next.solo || next.text == current.text {
        (current.text.to_owned(), None)
    } else {
        (
            current.text.to_owned(),
            Some(format!("{}{TIP_SEPARATOR}{}", current.text, next.text)),
        )
    }
}

fn current_tip_index() -> i128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| {
            (duration.as_millis() / TIP_ROTATE_INTERVAL_MS) as i128
        })
}

fn goal_snapshot_key(goal: Option<&GoalSnapshot>) -> Option<String> {
    let goal = goal?;
    Some(format!(
        "{}\0{}\0{}\0{}\0{}\0{}\0{:?}\0{:?}\0{:?}",
        goal.goal_id,
        goal.status.as_str(),
        goal.terminal_reason.as_deref().unwrap_or_default(),
        goal.turns_used,
        goal.tokens_used,
        goal.wall_clock_ms,
        goal.budget.token_budget,
        goal.budget.turn_budget,
        goal.budget.wall_clock_budget_ms
    ))
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use crate::{
        sdk::{model_alias::ModelAlias, types::GoalBudgetReport},
        tui::components::render::visible_width,
    };

    use super::*;

    fn model(display_name: Option<&str>, efforts: Option<&[&str]>) -> ModelAlias {
        ModelAlias {
            provider: "test".to_owned(),
            model: "model-id".to_owned(),
            max_context_size: 128_000,
            max_output_size: None,
            capabilities: None,
            display_name: display_name.map(str::to_owned),
            reasoning_key: None,
            protocol: None,
            adaptive_thinking: None,
            support_efforts: efforts
                .map(|values| values.iter().map(|value| (*value).to_owned()).collect()),
            default_effort: None,
            beta_api: None,
            overrides: None,
        }
    }

    fn state() -> FooterState {
        FooterState {
            model: "test/model".to_owned(),
            work_dir: format!("{}/not-a-git-repo", std::env::temp_dir().display()),
            permission_mode: PermissionMode::Manual,
            plan_mode: false,
            swarm_mode: false,
            thinking_effort: ThinkingEffort::from("off"),
            context_usage: 0.427,
            context_tokens: None,
            max_context_tokens: None,
            available_models: BTreeMap::from([(
                "test/model".to_owned(),
                model(Some("Test Model"), None),
            )]),
            goal: None,
        }
    }

    fn footer(state: FooterState) -> FooterComponent {
        FooterComponent::new(state, Arc::new(|| {}))
    }

    fn strip_terminal(text: &str) -> String {
        regex::Regex::new(r"\x1b\[[0-9;]*m")
            .expect("ANSI regex")
            .replace_all(text, "")
            .into_owned()
    }

    #[test]
    fn renders_modes_model_thinking_background_counts_and_context() {
        let mut state = state();
        state.permission_mode = PermissionMode::Auto;
        state.plan_mode = true;
        state.swarm_mode = true;
        state.thinking_effort = ThinkingEffort::from("high");
        state.available_models.insert(
            "test/model".to_owned(),
            model(Some("Test Model"), Some(&["low", "high"])),
        );
        state.context_tokens = Some(1_536);
        state.max_context_tokens = Some(4_096);
        let mut footer = footer(state);
        footer.set_background_counts(1, 2);
        let output = strip_terminal(&footer.render(160).join("\n"));
        assert!(output.contains("auto plan swarm"));
        assert!(output.contains("Test Model thinking: high"));
        assert!(output.contains("[1 task running]"));
        assert!(output.contains("[2 agents running]"));
        assert!(output.contains("context: 38% (1.5k/4k)"));
    }

    #[test]
    fn transient_hint_shares_context_line_and_counts_clamp() {
        let mut footer = footer(state());
        footer.set_background_counts(-3, -1);
        footer.set_transient_hint(Some("Press Ctrl+C again to exit".to_owned()));
        assert_eq!(footer.transient_hint(), Some("Press Ctrl+C again to exit"));
        let lines = footer.render(60);
        let output = strip_terminal(&lines.join("\n"));
        assert!(!output.contains("task running"));
        assert!(output.contains("Press Ctrl+C again"));
        assert!(output.contains("context: 43%"));
        assert!(lines.iter().all(|line| visible_width(line) <= 60));
    }

    #[test]
    fn renders_goal_badges_and_stops_timer_on_terminal_state() {
        let repaint_count = Arc::new(AtomicUsize::new(0));
        let callback_count = Arc::clone(&repaint_count);
        let mut state = state();
        state.goal = Some(GoalSnapshot {
            goal_id: "goal-1".to_owned(),
            objective: "finish".to_owned(),
            completion_criterion: None,
            status: GoalStatus::Active,
            turns_used: 1,
            tokens_used: 25,
            wall_clock_ms: 65_000,
            budget: GoalBudgetReport {
                token_budget: None,
                turn_budget: Some(5),
                wall_clock_budget_ms: None,
                remaining_tokens: None,
                remaining_turns: Some(4),
                remaining_wall_clock_ms: None,
                token_budget_reached: false,
                turn_budget_reached: false,
                wall_clock_budget_reached: false,
                over_budget: false,
            },
            terminal_reason: None,
        });
        let mut footer = FooterComponent::new(
            state.clone(),
            Arc::new(move || {
                callback_count.fetch_add(1, Ordering::Relaxed);
            }),
        );
        let output = strip_terminal(&footer.render(120).join("\n"));
        assert!(output.contains("[goal ● active · 1m · 1/5 turns]"));
        assert!(footer.goal_timer.is_some());

        state.goal.as_mut().expect("goal").status = GoalStatus::Complete;
        footer.set_state(state);
        assert!(footer.goal_timer.is_none());
        assert!(!strip_terminal(&footer.render(120).join("\n")).contains("[goal "));
        assert_eq!(repaint_count.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn helper_boundaries_match_original_footer() {
        assert_eq!(format_badge_elapsed(59_500), "1m");
        assert_eq!(format_badge_elapsed(60_000), "1m");
        assert_eq!(format_badge_elapsed(3_900_000), "1h5m");
        assert_eq!(format_context_status(f64::NAN, None, None), "context: 0%");
        assert_eq!(shorten_cwd("/a/b/c/d/e"), "…/c/d/e");
        let (primary, pair) = tips_for_index(0);
        assert!(!primary.is_empty());
        if let Some(pair) = pair {
            assert!(pair.contains(TIP_SEPARATOR));
        }
    }
}
