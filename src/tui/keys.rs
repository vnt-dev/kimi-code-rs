use std::sync::LazyLock;

use regex::Regex;

static KITTY_CSI_U: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\x1b\[(\d+)(?::(\d*))?(?::(\d+))?(?:;(\d+))?(?::(\d+))?u$")
        .expect("valid Kitty CSI-u regex")
});
static KITTY_ARROW: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\x1b\[1;(\d+)(?::\d+)?([ABCD])$").expect("valid Kitty arrow regex")
});
static KITTY_FUNCTION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\x1b\[(\d+)(?:;(\d+))?(?::\d+)?~$").expect("valid Kitty function regex")
});

const SHIFT: i64 = 1;
const ALT: i64 = 2;
const CTRL: i64 = 4;
const LOCK_MASK: i64 = 64 + 128;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListKey {
    Up,
    Down,
    PageUp,
    PageDown,
    Backspace,
}

// Original:
//   packages/pi-tui/src/keys.ts
//   decodeKittyPrintable()
pub fn decode_kitty_printable(data: &str) -> Option<String> {
    let captures = KITTY_CSI_U.captures(data)?;
    let codepoint = captures.get(1)?.as_str().parse::<i64>().ok()?;
    let shifted = captures
        .get(2)
        .filter(|value| !value.as_str().is_empty())
        .and_then(|value| value.as_str().parse::<i64>().ok());
    let modifier = captures
        .get(4)
        .and_then(|value| value.as_str().parse::<i64>().ok())
        .unwrap_or(1)
        - 1;
    if modifier & !(SHIFT | LOCK_MASK) != 0 || modifier & (ALT | CTRL) != 0 {
        return None;
    }
    let effective = if modifier & SHIFT != 0 {
        shifted.unwrap_or(codepoint)
    } else {
        codepoint
    };
    let effective = normalize_kitty_functional_codepoint(effective);
    if effective < 32 {
        return None;
    }
    char::from_u32(u32::try_from(effective).ok()?).map(|character| character.to_string())
}

fn normalize_kitty_functional_codepoint(codepoint: i64) -> i64 {
    match codepoint {
        57_399..=57_408 => codepoint - 57_399 + i64::from(b'0'),
        57_409 => i64::from(b'.'),
        57_410 => i64::from(b'/'),
        57_411 => i64::from(b'*'),
        57_412 => i64::from(b'-'),
        57_413 => i64::from(b'+'),
        57_415 => i64::from(b'='),
        57_416 => i64::from(b','),
        57_417 => -4,
        57_418 => -3,
        57_419 => -1,
        57_420 => -2,
        57_421 => -12,
        57_422 => -13,
        57_423 => -14,
        57_424 => -15,
        57_425 => -11,
        57_426 => -10,
        _ => codepoint,
    }
}

// Original:
//   packages/pi-tui/src/keys.ts
//   matchesKey() subset used by SearchableList.
pub fn matches_list_key(data: &str, key: ListKey) -> bool {
    let legacy = match key {
        ListKey::Up => matches!(data, "\u{1b}[A" | "\u{1b}OA"),
        ListKey::Down => matches!(data, "\u{1b}[B" | "\u{1b}OB"),
        ListKey::PageUp => matches!(data, "\u{1b}[5~" | "\u{1b}[[5~"),
        ListKey::PageDown => matches!(data, "\u{1b}[6~" | "\u{1b}[[6~"),
        ListKey::Backspace => matches!(data, "\u{7f}" | "\u{8}"),
    };
    if legacy {
        return true;
    }

    if let Some((codepoint, modifier)) = parse_kitty_csi_u(data) {
        let expected = match key {
            ListKey::Up => -1,
            ListKey::Down => -2,
            ListKey::PageUp => -12,
            ListKey::PageDown => -13,
            ListKey::Backspace => 127,
        };
        if modifier & !LOCK_MASK == 0 && normalize_kitty_functional_codepoint(codepoint) == expected
        {
            return true;
        }
    }

    if let Some(captures) = KITTY_ARROW.captures(data) {
        let modifier = captures[1].parse::<i64>().unwrap_or(1) - 1;
        let arrow = &captures[2];
        return modifier & !LOCK_MASK == 0
            && matches!((key, arrow), (ListKey::Up, "A") | (ListKey::Down, "B"));
    }
    if let Some(captures) = KITTY_FUNCTION.captures(data) {
        let modifier = captures
            .get(2)
            .and_then(|value| value.as_str().parse::<i64>().ok())
            .unwrap_or(1)
            - 1;
        let number = captures[1].parse::<u8>().ok();
        return modifier & !LOCK_MASK == 0
            && matches!(
                (key, number),
                (ListKey::PageUp, Some(5)) | (ListKey::PageDown, Some(6))
            );
    }
    false
}

fn parse_kitty_csi_u(data: &str) -> Option<(i64, i64)> {
    let captures = KITTY_CSI_U.captures(data)?;
    let codepoint = captures[1].parse().ok()?;
    let modifier = captures
        .get(4)
        .and_then(|value| value.as_str().parse::<i64>().ok())
        .unwrap_or(1)
        - 1;
    Some((codepoint, modifier))
}

#[cfg(test)]
mod tests {
    use super::{ListKey, decode_kitty_printable, matches_list_key};

    #[test]
    fn decodes_plain_shifted_and_keypad_printable_keys() {
        assert_eq!(decode_kitty_printable("\u{1b}[114u").as_deref(), Some("r"));
        assert_eq!(
            decode_kitty_printable("\u{1b}[97:65;2u").as_deref(),
            Some("A")
        );
        assert_eq!(
            decode_kitty_printable("\u{1b}[57400u").as_deref(),
            Some("1")
        );
    }

    #[test]
    fn rejects_control_alt_and_invalid_codepoints() {
        assert_eq!(decode_kitty_printable("\u{1b}[97;5u"), None);
        assert_eq!(decode_kitty_printable("\u{1b}[97;3u"), None);
        assert_eq!(decode_kitty_printable("\u{1b}[9u"), None);
        assert_eq!(decode_kitty_printable("not a key"), None);
    }

    #[test]
    fn matches_legacy_and_kitty_list_navigation() {
        assert!(matches_list_key("\u{1b}[A", ListKey::Up));
        assert!(matches_list_key("\u{1b}[B", ListKey::Down));
        assert!(matches_list_key("\u{1b}[5~", ListKey::PageUp));
        assert!(matches_list_key("\u{1b}[6~", ListKey::PageDown));
        assert!(matches_list_key("\u{7f}", ListKey::Backspace));
        assert!(matches_list_key("\u{1b}[1;1A", ListKey::Up));
        assert!(matches_list_key("\u{1b}[57420u", ListKey::Down));
        assert!(matches_list_key("\u{1b}[57421u", ListKey::PageUp));
        assert!(matches_list_key("\u{1b}[127u", ListKey::Backspace));
    }
}
