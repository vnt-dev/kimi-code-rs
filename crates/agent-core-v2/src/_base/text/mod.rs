pub mod line_endings;

/// UTF-16 code-unit length of `value`, matching JavaScript string length.
pub fn utf16_len(value: &str) -> usize {
    value.encode_utf16().count()
}

/// Keeps the first `units` UTF-16 code units of `value`, lossily decoding them back to UTF-8.
pub fn slice_utf16(value: &str, units: usize) -> String {
    String::from_utf16_lossy(&value.encode_utf16().take(units).collect::<Vec<_>>())
}

/// Truncates `value` to at most `max_units` UTF-16 code units, appending `marker`
/// when truncation happened. `value` is returned unchanged when it already fits.
pub fn truncate_utf16(value: &str, max_units: usize, marker: &str) -> String {
    if utf16_len(value) <= max_units {
        return value.to_owned();
    }
    let target = max_units.max(utf16_len(marker));
    let mut truncated = slice_utf16(value, target - utf16_len(marker));
    truncated.push_str(marker);
    truncated
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf16_len_measures_code_units_not_bytes() {
        assert_eq!(utf16_len("abc"), 3);
        assert_eq!(utf16_len("😀"), 2);
        assert_eq!(utf16_len(""), 0);
    }

    #[test]
    fn slice_keeps_whole_code_units_and_truncates_surrogate_pairs_lossily() {
        assert_eq!(slice_utf16("abcde", 3), "abc");
        assert_eq!(slice_utf16("😀x", 1), "\u{fffd}");
        assert_eq!(slice_utf16("abcde", 99), "abcde");
    }

    #[test]
    fn truncate_appends_marker_only_when_needed() {
        assert_eq!(truncate_utf16("abc", 5, "..."), "abc");
        assert_eq!(truncate_utf16("abcdef", 5, "..."), "ab...");
        assert_eq!(truncate_utf16("😀😀😀", 4, "…"), "😀\u{fffd}…");
        assert_eq!(truncate_utf16("abc", 1, "..."), "...");
    }
}
