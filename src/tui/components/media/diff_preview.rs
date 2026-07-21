#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffLineKind {
    Context,
    Add,
    Delete,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffLine {
    pub kind: DiffLineKind,
    pub line_num: usize,
    pub code: String,
}

// Original:
//   apps/kimi-code/src/tui/components/media/diff-preview.ts
//   computeDiffLines()
pub fn compute_diff_lines(
    old_text: &str,
    new_text: &str,
    old_start: usize,
    new_start: usize,
    is_incomplete: bool,
) -> Vec<DiffLine> {
    let old_lines = if old_text.is_empty() {
        Vec::new()
    } else {
        old_text.split('\n').collect::<Vec<_>>()
    };
    let new_lines = if new_text.is_empty() {
        Vec::new()
    } else {
        new_text.split('\n').collect::<Vec<_>>()
    };
    let old_len = old_lines.len();
    let new_len = new_lines.len();
    let mut longest_common_subsequence = vec![vec![0_usize; new_len + 1]; old_len + 1];
    for old_index in 1..=old_len {
        for new_index in 1..=new_len {
            longest_common_subsequence[old_index][new_index] =
                if old_lines[old_index - 1] == new_lines[new_index - 1] {
                    longest_common_subsequence[old_index - 1][new_index - 1] + 1
                } else {
                    longest_common_subsequence[old_index - 1][new_index]
                        .max(longest_common_subsequence[old_index][new_index - 1])
                };
        }
    }

    let mut reversed = Vec::new();
    let mut old_index = old_len;
    let mut new_index = new_len;
    while old_index > 0 || new_index > 0 {
        if old_index > 0 && new_index > 0 && old_lines[old_index - 1] == new_lines[new_index - 1] {
            reversed.push(DiffLine {
                kind: DiffLineKind::Context,
                line_num: new_start + new_index - 1,
                code: new_lines[new_index - 1].to_owned(),
            });
            old_index -= 1;
            new_index -= 1;
        } else if new_index > 0
            && (old_index == 0
                || longest_common_subsequence[old_index][new_index - 1]
                    >= longest_common_subsequence[old_index - 1][new_index])
        {
            reversed.push(DiffLine {
                kind: DiffLineKind::Add,
                line_num: new_start + new_index - 1,
                code: new_lines[new_index - 1].to_owned(),
            });
            new_index -= 1;
        } else {
            reversed.push(DiffLine {
                kind: DiffLineKind::Delete,
                line_num: old_start + old_index - 1,
                code: old_lines[old_index - 1].to_owned(),
            });
            old_index -= 1;
        }
    }
    reversed.reverse();

    if is_incomplete {
        let retained = reversed
            .iter()
            .rposition(|line| line.kind != DiffLineKind::Delete)
            .map_or(0, |index| index + 1);
        reversed.truncate(retained);
    }
    reversed
}

fn style_header_count(kind: DiffLineKind, text: &str) -> String {
    let token = match kind {
        DiffLineKind::Add => crate::tui::theme::ColorToken::DiffAddedStrong,
        DiffLineKind::Delete => crate::tui::theme::ColorToken::DiffRemovedStrong,
        DiffLineKind::Context => crate::tui::theme::ColorToken::DiffMeta,
    };
    crate::tui::theme::current_theme().bold_fg(token, text)
}

fn style_line(kind: DiffLineKind, text: &str) -> String {
    let token = match kind {
        DiffLineKind::Add => crate::tui::theme::ColorToken::DiffAdded,
        DiffLineKind::Delete => crate::tui::theme::ColorToken::DiffRemoved,
        DiffLineKind::Context => crate::tui::theme::ColorToken::Text,
    };
    crate::tui::theme::current_theme().fg(token, text)
}

fn style_gutter(text: &str) -> String {
    crate::tui::theme::current_theme().fg(crate::tui::theme::ColorToken::DiffGutter, text)
}

fn style_meta(text: &str) -> String {
    crate::tui::theme::current_theme().fg(crate::tui::theme::ColorToken::DiffMeta, text)
}

// Original: diff-preview.ts renderDiffLines()
pub fn render_diff_lines(
    old_text: &str,
    new_text: &str,
    path: &str,
    is_incomplete: bool,
    old_start: Option<usize>,
    new_start: Option<usize>,
    max_lines: Option<usize>,
) -> Vec<String> {
    let changed_lines = compute_diff_lines(
        old_text,
        new_text,
        old_start.unwrap_or(1),
        new_start.unwrap_or(1),
        is_incomplete,
    )
    .into_iter()
    .filter(|line| line.kind != DiffLineKind::Context)
    .collect::<Vec<_>>();
    let added = changed_lines
        .iter()
        .filter(|line| line.kind == DiffLineKind::Add)
        .count();
    let removed = changed_lines
        .iter()
        .filter(|line| line.kind == DiffLineKind::Delete)
        .count();
    let mut header = String::new();
    if added > 0 {
        header.push_str(&style_header_count(
            DiffLineKind::Add,
            &format!("+{added} "),
        ));
    }
    if removed > 0 {
        header.push_str(&style_header_count(
            DiffLineKind::Delete,
            &format!("-{removed} "),
        ));
    }
    header.push_str(path);
    let mut output = vec![header];
    let shown_count = max_lines.map_or(changed_lines.len(), |limit| limit.min(changed_lines.len()));
    for line in changed_lines.iter().take(shown_count) {
        let marker = match line.kind {
            DiffLineKind::Add => '+',
            DiffLineKind::Delete => '-',
            DiffLineKind::Context => continue,
        };
        output.push(format!(
            "{}{}",
            style_gutter(&format!("{:>4} ", line.line_num)),
            style_line(line.kind, &format!("{marker} {}", line.code))
        ));
    }
    let hidden = changed_lines.len() - shown_count;
    if hidden > 0 {
        let suffix = if hidden > 1 { "s" } else { "" };
        output.push(style_meta(&format!(
            "     … {hidden} more change{suffix} hidden (ctrl+o to expand)"
        )));
    }
    output
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ClusteredDiffOptions {
    pub context_lines: Option<usize>,
    pub max_lines: Option<usize>,
    pub is_incomplete: bool,
    pub expand_key_hint: Option<String>,
    pub old_start: Option<usize>,
    pub new_start: Option<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Cluster {
    start: usize,
    end: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ClusterSummary {
    clusters: Vec<Cluster>,
    changed_count: usize,
    added_count: usize,
    removed_count: usize,
}

fn build_clusters(diff_lines: &[DiffLine], context_lines: usize) -> ClusterSummary {
    let mut change_indices = Vec::new();
    let mut added_count = 0;
    let mut removed_count = 0;
    for (index, line) in diff_lines.iter().enumerate() {
        match line.kind {
            DiffLineKind::Add => {
                added_count += 1;
                change_indices.push(index);
            }
            DiffLineKind::Delete => {
                removed_count += 1;
                change_indices.push(index);
            }
            DiffLineKind::Context => {}
        }
    }
    if change_indices.is_empty() {
        return ClusterSummary {
            clusters: Vec::new(),
            changed_count: 0,
            added_count,
            removed_count,
        };
    }

    let mut clusters = Vec::new();
    let merge_gap = context_lines.saturating_mul(2);
    let mut group_start = change_indices[0];
    let mut group_end = group_start;
    for &index in &change_indices[1..] {
        if index - group_end <= merge_gap {
            group_end = index;
        } else {
            clusters.push(Cluster {
                start: group_start.saturating_sub(context_lines),
                end: group_end
                    .saturating_add(context_lines)
                    .min(diff_lines.len() - 1),
            });
            group_start = index;
            group_end = index;
        }
    }
    clusters.push(Cluster {
        start: group_start.saturating_sub(context_lines),
        end: group_end
            .saturating_add(context_lines)
            .min(diff_lines.len() - 1),
    });
    ClusterSummary {
        clusters,
        changed_count: change_indices.len(),
        added_count,
        removed_count,
    }
}

fn format_diff_row(line: &DiffLine) -> String {
    let gutter = style_gutter(&format!("{:>4} ", line.line_num));
    match line.kind {
        DiffLineKind::Add => format!(
            "{gutter}{}",
            style_line(line.kind, &format!("+ {}", line.code))
        ),
        DiffLineKind::Delete => {
            format!(
                "{gutter}{}",
                style_line(line.kind, &format!("- {}", line.code))
            )
        }
        DiffLineKind::Context => format!("{gutter}  {}", line.code),
    }
}

// Original: diff-preview.ts renderDiffLinesClustered()
pub fn render_diff_lines_clustered(
    old_text: &str,
    new_text: &str,
    path: &str,
    options: &ClusteredDiffOptions,
) -> Vec<String> {
    let context_lines = options.context_lines.unwrap_or(3);
    let diff_lines = compute_diff_lines(
        old_text,
        new_text,
        options.old_start.unwrap_or(1),
        options.new_start.unwrap_or(1),
        options.is_incomplete,
    );
    let summary = build_clusters(&diff_lines, context_lines);
    let mut header = String::new();
    if summary.added_count > 0 {
        header.push_str(&style_header_count(
            DiffLineKind::Add,
            &format!("+{} ", summary.added_count),
        ));
    }
    if summary.removed_count > 0 {
        header.push_str(&style_header_count(
            DiffLineKind::Delete,
            &format!("-{} ", summary.removed_count),
        ));
    }
    header.push_str(path);
    let mut output = vec![header];
    if summary.clusters.is_empty() {
        return output;
    }

    let cap = options.max_lines.unwrap_or(usize::MAX);
    let mut body = 0_usize;
    let mut previous_end = None;
    let mut truncated = false;
    let mut shown_changes = 0_usize;
    'clusters: for cluster in summary.clusters {
        if body >= cap {
            truncated = true;
            break;
        }
        if let Some(end) = previous_end {
            let gap = cluster.start.saturating_sub(end + 1);
            if gap > 0 {
                if body.saturating_add(1) > cap {
                    truncated = true;
                    break;
                }
                let suffix = if gap > 1 { "s" } else { "" };
                output.push(style_meta(&format!(
                    "     … {gap} unchanged line{suffix} …"
                )));
                body += 1;
            }
        }
        for (index, line) in diff_lines
            .iter()
            .enumerate()
            .take(cluster.end + 1)
            .skip(cluster.start)
        {
            if body >= cap {
                truncated = true;
                break 'clusters;
            }
            output.push(format_diff_row(line));
            body += 1;
            if line.kind != DiffLineKind::Context {
                shown_changes += 1;
            }
            previous_end = Some(index);
        }
    }
    if truncated {
        let hidden = summary.changed_count.saturating_sub(shown_changes);
        if hidden > 0 {
            let suffix = if hidden > 1 { "s" } else { "" };
            let hint = options.expand_key_hint.as_deref().unwrap_or("ctrl+o");
            output.push(style_meta(&format!(
                "     … {hidden} more change{suffix} hidden ({hint} to expand)"
            )));
        }
    }
    output
}

#[cfg(test)]
mod tests {
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

    fn kinds(lines: &[DiffLine]) -> Vec<DiffLineKind> {
        lines.iter().map(|line| line.kind).collect()
    }

    #[test]
    fn computes_complete_diff_with_source_line_numbers() {
        let lines = compute_diff_lines("A\nB\nC\nD", "A\nB", 10, 20, false);
        assert_eq!(
            kinds(&lines),
            [
                DiffLineKind::Context,
                DiffLineKind::Context,
                DiffLineKind::Delete,
                DiffLineKind::Delete,
            ]
        );
        assert_eq!(lines[0].line_num, 20);
        assert_eq!(lines[2].line_num, 12);
    }

    #[test]
    fn suppresses_only_trailing_deletes_while_incomplete() {
        assert_eq!(
            kinds(&compute_diff_lines("A\nB\nC\nD", "A\nB", 1, 1, true)),
            [DiffLineKind::Context, DiffLineKind::Context]
        );
        assert!(compute_diff_lines("A\nB\nC", "", 1, 1, true).is_empty());
        assert_eq!(
            kinds(&compute_diff_lines("A\nB\nC", "A\nB\nX", 1, 1, true)),
            [
                DiffLineKind::Context,
                DiffLineKind::Context,
                DiffLineKind::Delete,
                DiffLineKind::Add,
            ]
        );
        assert_eq!(
            kinds(&compute_diff_lines("A\nB\nC\nD", "A\nC", 1, 1, true)),
            [
                DiffLineKind::Context,
                DiffLineKind::Delete,
                DiffLineKind::Context,
            ]
        );
    }

    #[test]
    fn handles_empty_and_trailing_empty_lines_like_javascript_split() {
        assert!(compute_diff_lines("", "", 1, 1, false).is_empty());
        let lines = compute_diff_lines("a", "a\n", 1, 1, false);
        assert_eq!(kinds(&lines), [DiffLineKind::Context, DiffLineKind::Add]);
        assert_eq!(lines[1].code, "");
    }

    #[test]
    fn renders_changed_lines_and_respects_incomplete_suppression() {
        let complete = strip_sgr(
            &render_diff_lines("A\nB\nC\nD", "A\nB", "test.ts", false, None, None, None).join("\n"),
        );
        assert!(complete.contains("-2"));
        assert!(complete.contains('C'));
        assert!(complete.contains('D'));

        let incomplete = strip_sgr(
            &render_diff_lines("A\nB\nC\nD", "A\nB", "test.ts", true, None, None, None).join("\n"),
        );
        assert!(!incomplete.contains("-2"));
        assert!(!incomplete.contains('C'));
        assert!(!incomplete.contains('D'));
    }

    #[test]
    fn clustered_diff_renders_header_context_and_line_offsets() {
        let rendered = strip_sgr(
            &render_diff_lines_clustered(
                "A\nB\nC",
                "A\nX\nC",
                "foo.ts",
                &ClusteredDiffOptions {
                    context_lines: Some(1),
                    old_start: Some(10),
                    new_start: Some(20),
                    ..ClusteredDiffOptions::default()
                },
            )
            .join("\n"),
        );
        for expected in [
            "+1", "-1", "foo.ts", "  20   A", "  11 - B", "  21 + X", "  22   C",
        ] {
            assert!(
                rendered.contains(expected),
                "missing {expected:?} in {rendered:?}"
            );
        }
        let unchanged =
            render_diff_lines_clustered("A\nB", "A\nB", "foo.ts", &ClusteredDiffOptions::default());
        assert_eq!(unchanged.len(), 1);
    }

    #[test]
    fn clustered_diff_elides_distant_unchanged_regions() {
        let old_lines = (1..=30)
            .map(|index| format!("L{index}"))
            .collect::<Vec<_>>();
        let mut new_lines = old_lines.clone();
        new_lines[1] = "L2X".to_owned();
        new_lines[28] = "L29X".to_owned();
        let rendered = strip_sgr(
            &render_diff_lines_clustered(
                &old_lines.join("\n"),
                &new_lines.join("\n"),
                "f.ts",
                &ClusteredDiffOptions {
                    context_lines: Some(2),
                    ..ClusteredDiffOptions::default()
                },
            )
            .join("\n"),
        );
        assert!(rendered.contains("L2X"));
        assert!(rendered.contains("L29X"));
        assert!(rendered.contains("unchanged lines"));
        assert!(!rendered.contains("L15"));
    }

    #[test]
    fn clustered_diff_caps_a_large_cluster_with_expand_hint() {
        let old_text = (1..=100)
            .map(|index| format!("old{index}"))
            .collect::<Vec<_>>()
            .join("\n");
        let new_text = (1..=100)
            .map(|index| format!("new{index}"))
            .collect::<Vec<_>>()
            .join("\n");
        let output = render_diff_lines_clustered(
            &old_text,
            &new_text,
            "big.ts",
            &ClusteredDiffOptions {
                context_lines: Some(3),
                max_lines: Some(10),
                ..ClusteredDiffOptions::default()
            },
        );
        assert_eq!(output.len(), 12);
        let rendered = strip_sgr(&output.join("\n"));
        assert!(rendered.contains("+100"));
        assert!(rendered.contains("-100"));
        assert!(rendered.contains("ctrl+o to expand"));
    }
}
