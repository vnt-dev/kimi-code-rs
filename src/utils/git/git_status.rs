use std::{
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Arc,
        mpsc::{self, Receiver},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use regex::Regex;
use url::Url;

use crate::utils::terminal_hyperlink::to_terminal_hyperlink;

const BRANCH_TTL: Duration = Duration::from_secs(5);
const STATUS_TTL: Duration = Duration::from_secs(15);
const PULL_REQUEST_TTL: Duration = Duration::from_secs(60);
const SPAWN_TIMEOUT: Duration = Duration::from_millis(500);
const PR_SPAWN_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PullRequestInfo {
    pub number: u64,
    pub url: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitStatus {
    pub branch: String,
    pub dirty: bool,
    pub ahead: u64,
    pub behind: u64,
    pub diff_added: u64,
    pub diff_deleted: u64,
    pub pull_request: Option<PullRequestInfo>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FormatGitBadgeOptions {
    pub link_pull_request: bool,
}

#[derive(Debug, Clone, Default)]
struct WorktreeStatus {
    dirty: bool,
    ahead: u64,
    behind: u64,
    diff_added: u64,
    diff_deleted: u64,
}

struct PendingPullRequest {
    branch: String,
    receiver: Receiver<Option<PullRequestInfo>>,
    handle: JoinHandle<()>,
}

/// Cached branch, working-tree and pull-request reader for the footer.
///
/// Original: `utils/git/git-status.ts`, `createGitStatusCache()`.
pub struct GitStatusCache {
    work_dir: PathBuf,
    is_repo: bool,
    on_change: Option<Arc<dyn Fn() + Send + Sync>>,
    branch: Option<String>,
    branch_fetched_at: Option<Instant>,
    status: WorktreeStatus,
    status_fetched_at: Option<Instant>,
    pull_request: Option<PullRequestInfo>,
    pull_request_branch: Option<String>,
    pull_request_fetched_at: Option<Instant>,
    pending_pull_request: Option<PendingPullRequest>,
}

impl GitStatusCache {
    pub fn new(
        work_dir: impl Into<PathBuf>,
        on_change: Option<Arc<dyn Fn() + Send + Sync>>,
    ) -> Self {
        let work_dir = work_dir.into();
        let is_repo = detect_git_repo(&work_dir);
        Self {
            work_dir,
            is_repo,
            on_change,
            branch: None,
            branch_fetched_at: None,
            status: WorktreeStatus::default(),
            status_fetched_at: None,
            pull_request: None,
            pull_request_branch: None,
            pull_request_fetched_at: None,
            pending_pull_request: None,
        }
    }

    pub fn get_status(&mut self) -> Option<GitStatus> {
        if !self.is_repo {
            return None;
        }
        self.collect_pull_request();
        let now = Instant::now();
        if expired(self.branch_fetched_at, BRANCH_TTL, now) {
            self.branch = read_branch(&self.work_dir);
            self.branch_fetched_at = Some(now);
        }
        let branch = self.branch.clone()?;
        if expired(self.status_fetched_at, STATUS_TTL, now) {
            self.status = read_status(&self.work_dir);
            self.status_fetched_at = Some(now);
        }
        self.refresh_pull_request_if_needed(&branch, now);
        Some(GitStatus {
            branch: branch.clone(),
            dirty: self.status.dirty,
            ahead: self.status.ahead,
            behind: self.status.behind,
            diff_added: self.status.diff_added,
            diff_deleted: self.status.diff_deleted,
            pull_request: (self.pull_request_branch.as_deref() == Some(&branch))
                .then(|| self.pull_request.clone())
                .flatten(),
        })
    }

    fn refresh_pull_request_if_needed(&mut self, branch: &str, now: Instant) {
        if self.pending_pull_request.is_some() {
            return;
        }
        let fresh = self.pull_request_branch.as_deref() == Some(branch)
            && !expired(self.pull_request_fetched_at, PULL_REQUEST_TTL, now);
        if fresh {
            return;
        }
        let work_dir = self.work_dir.clone();
        let branch = branch.to_owned();
        let thread_branch = branch.clone();
        let previous = (self.pull_request_branch.as_deref() == Some(branch.as_str()))
            .then(|| self.pull_request.clone())
            .flatten();
        let on_change = self.on_change.clone();
        let (sender, receiver) = mpsc::channel();
        let handle = thread::spawn(move || {
            let result = read_pull_request(&work_dir);
            if result != previous
                && let Some(on_change) = on_change
            {
                on_change();
            }
            let _ = sender.send(result);
        });
        self.pending_pull_request = Some(PendingPullRequest {
            branch: thread_branch,
            receiver,
            handle,
        });
    }

    fn collect_pull_request(&mut self) {
        let Some(pending) = self.pending_pull_request.as_ref() else {
            return;
        };
        let Ok(value) = pending.receiver.try_recv() else {
            return;
        };
        let pending = self.pending_pull_request.take().expect("pending PR exists");
        let _ = pending.handle.join();
        self.pull_request = value;
        self.pull_request_branch = Some(pending.branch);
        self.pull_request_fetched_at = Some(Instant::now());
    }
}

impl Drop for GitStatusCache {
    fn drop(&mut self) {
        if let Some(pending) = self.pending_pull_request.take() {
            let _ = pending.handle.join();
        }
    }
}

fn expired(fetched_at: Option<Instant>, ttl: Duration, now: Instant) -> bool {
    fetched_at.is_none_or(|fetched_at| now.duration_since(fetched_at) >= ttl)
}

fn detect_git_repo(work_dir: &Path) -> bool {
    run_with_timeout(
        "git",
        &[
            "-C",
            &work_dir.to_string_lossy(),
            "rev-parse",
            "--is-inside-work-tree",
        ],
        None,
        SPAWN_TIMEOUT,
        &[],
    )
    .is_some_and(|output| output.trim() == "true")
}

fn read_branch(work_dir: &Path) -> Option<String> {
    let output = run_with_timeout(
        "git",
        &[
            "-C",
            &work_dir.to_string_lossy(),
            "branch",
            "--show-current",
        ],
        None,
        SPAWN_TIMEOUT,
        &[],
    )?;
    let branch = output.trim();
    (!branch.is_empty()).then(|| branch.to_owned())
}

fn read_status(work_dir: &Path) -> WorktreeStatus {
    let Some(output) = run_with_timeout(
        "git",
        &[
            "-C",
            &work_dir.to_string_lossy(),
            "status",
            "--porcelain",
            "-b",
        ],
        None,
        SPAWN_TIMEOUT,
        &[],
    ) else {
        return WorktreeStatus::default();
    };
    let ahead_behind =
        Regex::new(r"\[(?:ahead (\d+))?(?:, )?(?:behind (\d+))?\]").expect("ahead/behind regex");
    let mut status = WorktreeStatus::default();
    for line in output.lines() {
        if line.starts_with("## ") {
            if let Some(captures) = ahead_behind.captures(line) {
                status.ahead = captures
                    .get(1)
                    .and_then(|m| m.as_str().parse().ok())
                    .unwrap_or(0);
                status.behind = captures
                    .get(2)
                    .and_then(|m| m.as_str().parse().ok())
                    .unwrap_or(0);
            }
        } else if !line.trim().is_empty() {
            status.dirty = true;
        }
    }
    if status.dirty {
        (status.diff_added, status.diff_deleted) = read_diff_stats(work_dir);
    }
    status
}

fn read_diff_stats(work_dir: &Path) -> (u64, u64) {
    let Some(output) = run_with_timeout(
        "git",
        &[
            "-C",
            &work_dir.to_string_lossy(),
            "diff",
            "--numstat",
            "HEAD",
            "--",
        ],
        None,
        SPAWN_TIMEOUT,
        &[],
    ) else {
        return (0, 0);
    };
    output.lines().fold((0, 0), |(added, deleted), line| {
        let mut fields = line.split('\t');
        (
            added + parse_diff_numstat_count(fields.next()),
            deleted + parse_diff_numstat_count(fields.next()),
        )
    })
}

fn read_pull_request(work_dir: &Path) -> Option<PullRequestInfo> {
    let output = run_with_timeout(
        "gh",
        &["pr", "view", "--json", "number,url"],
        Some(work_dir),
        PR_SPAWN_TIMEOUT,
        &[("GH_NO_UPDATE_NOTIFIER", "1"), ("GH_PROMPT_DISABLED", "1")],
    )?;
    parse_pull_request(&output)
}

fn run_with_timeout(
    program: &str,
    args: &[&str],
    current_dir: Option<&Path>,
    timeout: Duration,
    environment: &[(&str, &str)],
) -> Option<String> {
    let mut command = Command::new(program);
    command
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    if let Some(current_dir) = current_dir {
        command.current_dir(current_dir);
    }
    command.envs(environment.iter().copied());
    let mut child = command.spawn().ok()?;
    let mut stdout = child.stdout.take()?;
    let reader = thread::spawn(move || {
        let mut output = String::new();
        stdout.read_to_string(&mut output).map(|_| output).ok()
    });
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if started.elapsed() < timeout => thread::sleep(Duration::from_millis(10)),
            Ok(None) => {
                let _ = child.kill();
                break child.wait().ok();
            }
            Err(_) => break None,
        }
    };
    let output = reader.join().ok().flatten()?;
    status.filter(|status| status.success()).map(|_| output)
}

// Original: utils/git/git-status.ts formatGitBadgeBase()
pub fn format_git_badge_base(status: &GitStatus) -> String {
    let mut parts = Vec::new();
    if let Some(diff) = format_diff_stats(status) {
        parts.push(diff);
    }
    let mut sync = String::new();
    if status.ahead > 0 {
        sync.push_str(&format!("↑{}", status.ahead));
    }
    if status.behind > 0 {
        sync.push_str(&format!("↓{}", status.behind));
    }
    if !sync.is_empty() {
        parts.push(sync);
    }
    if parts.is_empty() {
        status.branch.clone()
    } else {
        format!("{} [{}]", status.branch, parts.join(" "))
    }
}

// Original: utils/git/git-status.ts formatPullRequestBadge()
pub fn format_pull_request_badge(
    pull_request: &PullRequestInfo,
    options: FormatGitBadgeOptions,
) -> String {
    let text = format!("[PR#{}]", pull_request.number);
    if options.link_pull_request && is_safe_http_url(&pull_request.url) {
        to_terminal_hyperlink(&text, &pull_request.url)
    } else {
        text
    }
}

// Original: utils/git/git-status.ts formatGitBadge()
pub fn format_git_badge(status: &GitStatus, options: FormatGitBadgeOptions) -> String {
    let base = format_git_badge_base(status);
    status
        .pull_request
        .as_ref()
        .map_or(base.clone(), |pull_request| {
            format!(
                "{base} {}",
                format_pull_request_badge(pull_request, options)
            )
        })
}

fn format_diff_stats(status: &GitStatus) -> Option<String> {
    let mut parts = Vec::new();
    if status.diff_added > 0 {
        parts.push(format!("+{}", status.diff_added));
    }
    if status.diff_deleted > 0 {
        parts.push(format!("-{}", status.diff_deleted));
    }
    if !parts.is_empty() {
        Some(parts.join(" "))
    } else if status.dirty {
        Some("±".to_owned())
    } else {
        None
    }
}

pub(crate) fn parse_diff_numstat_count(value: Option<&str>) -> u64 {
    let Some(value) = value.map(str::trim_start) else {
        return 0;
    };
    let digits = value
        .strip_prefix('+')
        .unwrap_or(value)
        .chars()
        .take_while(char::is_ascii_digit)
        .collect::<String>();
    digits
        .parse::<u64>()
        .ok()
        .filter(|value| *value > 0)
        .unwrap_or(0)
}

pub(crate) fn parse_pull_request(stdout: &str) -> Option<PullRequestInfo> {
    let value = serde_json::from_str::<serde_json::Value>(stdout).ok()?;
    let number = value.get("number")?.as_u64().filter(|number| *number > 0)?;
    let url = value.get("url")?.as_str()?;
    is_safe_http_url(url).then(|| PullRequestInfo {
        number,
        url: url.to_owned(),
    })
}

pub(crate) fn is_safe_http_url(value: &str) -> bool {
    if value
        .chars()
        .any(|character| character <= '\u{1f}' || character == '\u{7f}')
    {
        return false;
    }
    Url::parse(value).is_ok_and(|url| matches!(url.scheme(), "http" | "https"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status() -> GitStatus {
        GitStatus {
            branch: "main".to_owned(),
            dirty: true,
            ahead: 2,
            behind: 1,
            diff_added: 12,
            diff_deleted: 3,
            pull_request: None,
        }
    }

    #[test]
    fn formats_diff_sync_and_dirty_only_badges() {
        assert_eq!(format_git_badge_base(&status()), "main [+12 -3 ↑2↓1]");
        let dirty = GitStatus {
            ahead: 0,
            behind: 0,
            diff_added: 0,
            diff_deleted: 0,
            ..status()
        };
        assert_eq!(format_git_badge_base(&dirty), "main [±]");
        let clean = GitStatus {
            dirty: false,
            ..dirty
        };
        assert_eq!(format_git_badge_base(&clean), "main");
    }

    #[test]
    fn formats_safe_pull_request_as_optional_terminal_link() {
        let mut status = status();
        status.pull_request = Some(PullRequestInfo {
            number: 12,
            url: "https://github.com/acme/repo/pull/12".to_owned(),
        });
        let plain = format_git_badge(&status, FormatGitBadgeOptions::default());
        assert!(plain.ends_with("[PR#12]"));
        assert!(!plain.contains("\u{1b}]8;;"));
        let linked = format_git_badge(
            &status,
            FormatGitBadgeOptions {
                link_pull_request: true,
            },
        );
        assert!(linked.contains("\u{1b}]8;;https://github.com/acme/repo/pull/12\u{7}"));
        assert!(linked.contains("\u{1b}]8;;\u{7}"));
    }

    #[test]
    fn parses_only_positive_integer_prs_with_safe_http_urls() {
        assert_eq!(
            parse_pull_request(r#"{"number":7,"url":"http://example.test/pull/7"}"#),
            Some(PullRequestInfo {
                number: 7,
                url: "http://example.test/pull/7".to_owned()
            })
        );
        for invalid in [
            r#"{"number":0,"url":"https://example.test"}"#,
            r#"{"number":1.5,"url":"https://example.test"}"#,
            r#"{"number":1,"url":"file:///tmp/a"}"#,
            "{not-json}",
        ] {
            assert_eq!(parse_pull_request(invalid), None);
        }
        assert!(!is_safe_http_url("https://example.test/\nattack"));
    }

    #[test]
    fn parses_numstat_like_javascript_parse_int() {
        assert_eq!(parse_diff_numstat_count(Some("12")), 12);
        assert_eq!(parse_diff_numstat_count(Some("12px")), 12);
        assert_eq!(parse_diff_numstat_count(Some("-")), 0);
        assert_eq!(parse_diff_numstat_count(Some("bad")), 0);
        assert_eq!(parse_diff_numstat_count(None), 0);
    }
}
