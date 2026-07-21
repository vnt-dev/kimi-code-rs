/// Original:
///   apps/kimi-code/src/utils/terminal-hyperlink.ts
///   toTerminalHyperlink()
pub fn to_terminal_hyperlink(text: &str, url: &str) -> String {
    format!("\u{1b}]8;;{url}\u{7}{text}\u{1b}]8;;\u{7}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wraps_text_in_an_osc8_bel_terminated_link() {
        assert_eq!(
            to_terminal_hyperlink("report.md", "file:///tmp/report.md"),
            "\u{1b}]8;;file:///tmp/report.md\u{7}report.md\u{1b}]8;;\u{7}"
        );
    }

    #[test]
    fn preserves_empty_and_unescaped_values_like_the_original() {
        assert_eq!(
            to_terminal_hyperlink("", "https://example.test/a b"),
            "\u{1b}]8;;https://example.test/a b\u{7}\u{1b}]8;;\u{7}"
        );
    }
}
