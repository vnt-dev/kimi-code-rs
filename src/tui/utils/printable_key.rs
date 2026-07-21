use crate::tui::keys::decode_kitty_printable;

// Original:
//   apps/kimi-code/src/tui/utils/printable-key.ts
//   printableChar()
pub fn printable_char(data: &str) -> String {
    decode_kitty_printable(data).unwrap_or_else(|| data.to_owned())
}

// Original:
//   apps/kimi-code/src/tui/utils/printable-key.ts
//   isPrintableChar()
pub fn is_printable_char(character: &str) -> bool {
    if character.encode_utf16().count() != 1 {
        return false;
    }
    let Some(codepoint) = character.chars().next().map(u32::from) else {
        return false;
    };
    codepoint >= 0x20 && codepoint != 0x7f
}

#[cfg(test)]
mod tests {
    use super::{is_printable_char, printable_char};

    #[test]
    fn decodes_kitty_and_preserves_bare_input() {
        assert_eq!(printable_char("\u{1b}[113u"), "q");
        assert_eq!(printable_char("q"), "q");
    }

    #[test]
    fn accepts_one_utf16_printable_unit_only() {
        assert!(is_printable_char(" "));
        assert!(is_printable_char("a"));
        assert!(is_printable_char("中"));
        assert!(!is_printable_char(""));
        assert!(!is_printable_char("ab"));
        assert!(!is_printable_char("\n"));
        assert!(!is_printable_char("\u{7f}"));
        assert!(!is_printable_char("😀"));
    }
}
