//! Pure parsers for Git and GitHub CLI output.
//!
//! Original: `packages/agent-core-v2/src/app/git/gitParsers.ts`.

use std::{collections::HashSet, sync::LazyLock};

use indexmap::IndexMap;
use regex::Regex;
use serde_json::Value;
use url::Url;

use super::contract::{FsGitStatus, FsGitStatusResponse, FsPullRequest, FsPullRequestState};

static AHEAD_PATTERN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"ahead (\d+)").expect("ahead regex is valid"));
static BEHIND_PATTERN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"behind (\d+)").expect("behind regex is valid"));

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct NumstatSummary {
    pub additions: u64,
    pub deletions: u64,
}

// Original: gitParsers.ts, parsePorcelain(). Parsing is synchronous because it
// only transforms an in-memory command result.
pub fn parse_porcelain(stdout: &str, filter: Option<&HashSet<String>>) -> FsGitStatusResponse {
    let mut branch = String::new();
    let mut ahead = 0;
    let mut behind = 0;
    let mut entries = IndexMap::new();

    for line in stdout.split('\n') {
        if line.is_empty() {
            continue;
        }
        if let Some(header) = line.strip_prefix("## ") {
            let parsed = parse_branch_header(header);
            branch = parsed.branch;
            ahead = parsed.ahead;
            behind = parsed.behind;
            continue;
        }

        if line.len() < 4 {
            continue;
        }
        let status_code = &line[..2];
        let mut path = &line[3..];
        if matches!(status_code.as_bytes().first(), Some(b'R' | b'C'))
            && let Some(arrow) = path.find(" -> ")
        {
            path = &path[arrow + 4..];
        }
        let wire_path = path.trim().replace('\\', "/");
        if filter.is_some_and(|filter| !filter.contains(&wire_path)) {
            continue;
        }
        entries.insert(wire_path, collapse_xy(status_code));
    }

    FsGitStatusResponse {
        branch,
        ahead,
        behind,
        entries,
        additions: 0,
        deletions: 0,
        pull_request: None,
    }
}

// Original: gitParsers.ts, parseNumstat().
pub fn parse_numstat(stdout: &str) -> NumstatSummary {
    let mut summary = NumstatSummary::default();
    for line in stdout.split('\n') {
        if line.is_empty() {
            continue;
        }
        let mut fields = line.split('\t');
        summary.additions = summary
            .additions
            .saturating_add(parse_numstat_count(fields.next()));
        summary.deletions = summary
            .deletions
            .saturating_add(parse_numstat_count(fields.next()));
    }
    summary
}

fn parse_numstat_count(value: Option<&str>) -> u64 {
    let Some(value) = value else {
        return 0;
    };
    let value = value.trim_start();
    if value == "-" || value.starts_with('-') {
        return 0;
    }
    let digits = value
        .strip_prefix('+')
        .unwrap_or(value)
        .chars()
        .take_while(char::is_ascii_digit)
        .collect::<String>();
    digits
        .parse::<u64>()
        .ok()
        .filter(|count| *count > 0)
        .unwrap_or(0)
}

#[derive(Debug, Eq, PartialEq)]
struct BranchHeader {
    branch: String,
    ahead: u64,
    behind: u64,
}

fn parse_branch_header(rest: &str) -> BranchHeader {
    if rest.starts_with("HEAD (no branch)") {
        return BranchHeader {
            branch: String::new(),
            ahead: 0,
            behind: 0,
        };
    }
    if let Some(branch) = rest.strip_prefix("No commits yet on ") {
        return BranchHeader {
            branch: branch.to_owned(),
            ahead: 0,
            behind: 0,
        };
    }

    let mut branch = rest;
    let mut ahead = 0;
    let mut behind = 0;
    if let Some(bracket) = rest.find(" [") {
        branch = &rest[..bracket];
        let counters = rest
            .get(bracket + 2..rest.len().saturating_sub(1))
            .unwrap_or_default();
        ahead = capture_count(&AHEAD_PATTERN, counters);
        behind = capture_count(&BEHIND_PATTERN, counters);
    }
    if let Some(dots) = branch.find("...") {
        branch = &branch[..dots];
    }

    BranchHeader {
        branch: branch.to_owned(),
        ahead,
        behind,
    }
}

fn capture_count(pattern: &Regex, input: &str) -> u64 {
    pattern
        .captures(input)
        .and_then(|captures| captures.get(1))
        .and_then(|value| value.as_str().parse().ok())
        .unwrap_or(0)
}

fn collapse_xy(status_code: &str) -> FsGitStatus {
    if status_code == "??" {
        return FsGitStatus::Untracked;
    }
    if status_code == "!!" {
        return FsGitStatus::Ignored;
    }
    if matches!(status_code, "DD" | "AU" | "UD" | "UA" | "DU" | "AA" | "UU") {
        return FsGitStatus::Conflicted;
    }

    let contains = |needle| status_code.as_bytes().contains(&needle);
    if contains(b'D') {
        FsGitStatus::Deleted
    } else if contains(b'M') || contains(b'T') {
        FsGitStatus::Modified
    } else if contains(b'R') || contains(b'C') {
        FsGitStatus::Renamed
    } else if contains(b'A') {
        FsGitStatus::Added
    } else {
        FsGitStatus::Clean
    }
}

// Original: gitParsers.ts, parsePullRequest(). The source intentionally accepts
// only open, merged, and closed here even though the wire schema also has draft.
pub fn parse_pull_request(stdout: &str) -> Option<FsPullRequest> {
    let Value::Object(raw) = serde_json::from_str(stdout).ok()? else {
        return None;
    };
    let number = raw.get("number")?.as_u64()?;
    if number == 0 {
        return None;
    }
    let url = raw.get("url")?.as_str()?;
    if !is_safe_http_url(url) {
        return None;
    }
    let state = match raw.get("state")?.as_str()?.to_lowercase().as_str() {
        "open" => FsPullRequestState::Open,
        "merged" => FsPullRequestState::Merged,
        "closed" => FsPullRequestState::Closed,
        _ => return None,
    };

    Some(FsPullRequest {
        number,
        state,
        url: url.to_owned(),
    })
}

fn is_safe_http_url(value: &str) -> bool {
    if value.chars().any(|character| {
        let code = character as u32;
        code <= 0x1f || code == 0x7f
    }) {
        return false;
    }
    Url::parse(value).is_ok_and(|url| matches!(url.scheme(), "https" | "http"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn porcelain_parses_branch_counters_and_empty_entries() {
        let result = parse_porcelain("## main...origin/main [ahead 2, behind 3]\n", None);
        assert_eq!(result.branch, "main");
        assert_eq!(result.ahead, 2);
        assert_eq!(result.behind, 3);
        assert!(result.entries.is_empty());
    }

    #[test]
    fn porcelain_classifies_and_normalizes_paths() {
        let result = parse_porcelain(
            "## dev\n M src\\a.ts\n?? src/b.ts\nR  old.ts -> new.ts\nD  src/c.ts\n",
            None,
        );
        assert_eq!(result.branch, "dev");
        assert_eq!(
            result.entries,
            IndexMap::from([
                ("src/a.ts".into(), FsGitStatus::Modified),
                ("src/b.ts".into(), FsGitStatus::Untracked),
                ("new.ts".into(), FsGitStatus::Renamed),
                ("src/c.ts".into(), FsGitStatus::Deleted),
            ])
        );
    }

    #[test]
    fn porcelain_applies_filter_and_handles_special_headers() {
        let filter = HashSet::from(["src/a.ts".to_owned()]);
        let result = parse_porcelain(
            "## No commits yet on first\n M src/a.ts\n M src/b.ts\n",
            Some(&filter),
        );
        assert_eq!(result.branch, "first");
        assert_eq!(
            result.entries,
            IndexMap::from([("src/a.ts".into(), FsGitStatus::Modified)])
        );
        assert_eq!(parse_porcelain("## HEAD (no branch)\n", None).branch, "");
    }

    #[test]
    fn porcelain_preserves_xy_precedence() {
        let result = parse_porcelain(
            "## main\nDD conflict\n T type\nC  old -> copy\nA  added\n!! ignored\n   clean\n",
            None,
        );
        assert_eq!(result.entries["conflict"], FsGitStatus::Conflicted);
        assert_eq!(result.entries["type"], FsGitStatus::Modified);
        assert_eq!(result.entries["copy"], FsGitStatus::Renamed);
        assert_eq!(result.entries["added"], FsGitStatus::Added);
        assert_eq!(result.entries["ignored"], FsGitStatus::Ignored);
        assert_eq!(result.entries["clean"], FsGitStatus::Clean);
    }

    #[test]
    fn numstat_sums_lines_and_treats_binary_or_invalid_counts_as_zero() {
        assert_eq!(
            parse_numstat("10\t2\tsrc/a.ts\n3\t0\tsrc/b.ts\n"),
            NumstatSummary {
                additions: 13,
                deletions: 2
            }
        );
        assert_eq!(
            parse_numstat("-\t-\timage.png\n5\t1\tsrc/a.ts\n"),
            NumstatSummary {
                additions: 5,
                deletions: 1
            }
        );
        assert_eq!(parse_numstat(""), NumstatSummary::default());
        assert_eq!(
            parse_numstat("3files\t-2\todd\n"),
            NumstatSummary {
                additions: 3,
                deletions: 0
            }
        );
    }

    #[test]
    fn pull_request_normalizes_valid_states() {
        assert_eq!(
            parse_pull_request(
                r#"{"number":12,"url":"https://github.com/acme/repo/pull/12","state":"OPEN"}"#
            ),
            Some(FsPullRequest {
                number: 12,
                state: FsPullRequestState::Open,
                url: "https://github.com/acme/repo/pull/12".into(),
            })
        );
    }

    #[test]
    fn pull_request_rejects_malformed_or_unsafe_values() {
        assert_eq!(parse_pull_request("not json"), None);
        assert_eq!(
            parse_pull_request(r#"{"number":1,"url":"ftp://x/y","state":"open"}"#),
            None
        );
        assert_eq!(
            parse_pull_request(r#"{"number":1,"url":"https://x/y","state":"weird"}"#),
            None
        );
        assert_eq!(
            parse_pull_request(r#"{"number":1,"url":"https://x/y\u0000","state":"open"}"#),
            None
        );
        assert_eq!(
            parse_pull_request(r#"{"number":1,"url":"https://x/y","state":"draft"}"#),
            None
        );
    }
}
