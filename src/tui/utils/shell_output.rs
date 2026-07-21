// Original:
//   apps/kimi-code/src/tui/utils/shell-output.ts
//   sanitizeShellOutput()
//
// Rust adaptation:
//   A single-pass byte scanner replaces the four JavaScript regular
//   expressions. It recognizes the same CSI, OSC, and short ESC forms while
//   preserving UTF-8 text, newlines, and tabs.
pub fn sanitize_shell_output(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut clean = Vec::with_capacity(bytes.len());
    let mut index = 0;

    while index < bytes.len() {
        let byte = bytes[index];
        if byte == 0x1b {
            if bytes.get(index + 1) == Some(&b']') {
                if let Some(end) = osc_end(bytes, index + 2) {
                    index = end;
                    continue;
                }
            } else if bytes.get(index + 1) == Some(&b'[')
                && let Some(end) = csi_end(bytes, index + 2)
            {
                index = end;
                continue;
            }

            index += short_escape_len(bytes, index);
            continue;
        }

        if is_stripped_c0(byte) {
            index += 1;
            continue;
        }

        clean.push(byte);
        index += 1;
    }

    // Every removed sequence is ASCII, so copying the remaining bytes from a
    // valid `str` cannot invalidate its UTF-8 encoding.
    String::from_utf8(clean).expect("removing ASCII controls preserves UTF-8")
}

fn osc_end(bytes: &[u8], mut index: usize) -> Option<usize> {
    while index < bytes.len() {
        if bytes[index] == 0x07 {
            return Some(index + 1);
        }
        if bytes[index] == 0x1b && bytes.get(index + 1) == Some(&b'\\') {
            return Some(index + 2);
        }
        index += 1;
    }
    None
}

fn csi_end(bytes: &[u8], mut index: usize) -> Option<usize> {
    while bytes
        .get(index)
        .is_some_and(|byte| (0x30..=0x3f).contains(byte))
    {
        index += 1;
    }
    while bytes
        .get(index)
        .is_some_and(|byte| (0x20..=0x2f).contains(byte))
    {
        index += 1;
    }
    bytes
        .get(index)
        .is_some_and(|byte| (0x40..=0x7e).contains(byte))
        .then_some(index + 1)
}

fn short_escape_len(bytes: &[u8], index: usize) -> usize {
    let Some(&next) = bytes.get(index + 1) else {
        return 1;
    };
    if (0x20..=0x2f).contains(&next)
        && bytes
            .get(index + 2)
            .is_some_and(|byte| (0x30..=0x7e).contains(byte))
    {
        3
    } else if (0x30..=0x7e).contains(&next) {
        2
    } else {
        1
    }
}

fn is_stripped_c0(byte: u8) -> bool {
    matches!(byte, 0x00..=0x08 | 0x0b..=0x1f)
}

#[cfg(test)]
mod tests {
    use super::sanitize_shell_output;

    const ESC: char = '\u{001b}';
    const BEL: char = '\u{0007}';

    #[test]
    fn leaves_plain_utf8_text_newlines_and_tabs_untouched() {
        assert_eq!(
            sanitize_shell_output("hello\n世界\tmoon"),
            "hello\n世界\tmoon"
        );
    }

    #[test]
    fn strips_sgr_colour_sequences() {
        assert_eq!(
            sanitize_shell_output(&format!("{ESC}[31mred{ESC}[0m")),
            "red"
        );
        assert_eq!(
            sanitize_shell_output(&format!("{ESC}[1;32mbold green{ESC}[0m")),
            "bold green"
        );
    }

    #[test]
    fn strips_private_modes_clear_screen_and_cursor_movement() {
        assert_eq!(
            sanitize_shell_output(&format!("{ESC}[?1049h{ESC}[?25l")),
            ""
        );
        assert_eq!(
            sanitize_shell_output(&format!("before{ESC}[?2004hafter")),
            "beforeafter"
        );
        assert_eq!(
            sanitize_shell_output(&format!("{ESC}[2J{ESC}[Hhello")),
            "hello"
        );
        assert_eq!(sanitize_shell_output(&format!("{ESC}[10;5Hhi")), "hi");
    }

    #[test]
    fn strips_osc_titles_and_hyperlink_controls() {
        assert_eq!(
            sanitize_shell_output(&format!("{ESC}]0;my title{BEL}text")),
            "text"
        );
        let link = format!("{ESC}]8;;https://example.com{ESC}\\click here{ESC}]8;;{ESC}\\");
        assert_eq!(sanitize_shell_output(&link), "click here");
    }

    #[test]
    fn strips_c0_controls_except_newline_and_tab() {
        assert_eq!(
            sanitize_shell_output("frame1\rframe2\rframe3"),
            "frame1frame2frame3"
        );
        assert_eq!(sanitize_shell_output("line\r\nnext"), "line\nnext");
        assert_eq!(
            sanitize_shell_output(&format!("a\u{0008}b{BEL}c\0d")),
            "abcd"
        );
    }

    #[test]
    fn strips_single_character_escape_commands() {
        assert_eq!(
            sanitize_shell_output(&format!("{ESC}c{ESC}7{ESC}8text")),
            "text"
        );
    }

    #[test]
    fn handles_large_input_in_one_pass() {
        let huge = format!("{ESC}[31m{}\r{ESC}[0m", "x".repeat(2_000_000));
        assert_eq!(sanitize_shell_output(&huge), "x".repeat(2_000_000));
    }

    #[test]
    fn cleans_a_realistic_tui_server_burst() {
        let messy = format!(
            "{ESC}[?1049h{ESC}[?25l{ESC}[2J{ESC}[H{ESC}[1m{ESC}[32mVITE{ESC}[0m ready in 120ms\r\n{ESC}]0;dev server{BEL}  Local: http://localhost:5173/"
        );
        let result = sanitize_shell_output(&messy);
        assert!(!result.contains(ESC));
        assert!(!result.contains('\r'));
        assert!(result.contains("VITE ready in 120ms"));
        assert!(result.contains("Local: http://localhost:5173/"));
    }

    #[test]
    fn unterminated_sequences_match_the_original_regex_fallback() {
        assert_eq!(sanitize_shell_output(&format!("{ESC}]title")), "title");
        assert_eq!(sanitize_shell_output(&format!("{ESC}[31")), "31");
    }
}
