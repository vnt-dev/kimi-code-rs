const ARGS_PREVIEW_MAX_UTF16_UNITS: usize = 200;

pub(super) fn args_preview(args: Option<&str>) -> Option<String> {
    let trimmed = args.unwrap_or_default().trim();
    if trimmed.is_empty() {
        return None;
    }
    let units: Vec<u16> = trimmed.encode_utf16().collect();
    if units.len() <= ARGS_PREVIEW_MAX_UTF16_UNITS {
        return Some(trimmed.to_owned());
    }
    let mut preview = String::from_utf16_lossy(&units[..ARGS_PREVIEW_MAX_UTF16_UNITS]);
    preview.push('…');
    Some(preview)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trims_omits_and_limits_using_javascript_utf16_length() {
        assert_eq!(args_preview(None), None);
        assert_eq!(args_preview(Some(" \n\t ")), None);
        assert_eq!(
            args_preview(Some("  deploy now  ")).as_deref(),
            Some("deploy now")
        );
        assert_eq!(
            args_preview(Some(&"a".repeat(201)))
                .unwrap_or_default()
                .len(),
            203
        );

        let boundary = format!("{}😀", "a".repeat(199));
        let preview = args_preview(Some(&boundary)).unwrap_or_default();
        assert!(preview.ends_with("�…"));
    }
}
