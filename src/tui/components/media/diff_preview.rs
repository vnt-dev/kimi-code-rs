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

#[cfg(test)]
mod tests {
    use super::*;

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
}
