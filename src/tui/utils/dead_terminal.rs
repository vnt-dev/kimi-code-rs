/// Detect Node-compatible terminal error codes that mean the controlling PTY
/// is no longer writable. Callers extract the optional platform error code at
/// their I/O boundary and pass it here.
///
/// Original:
///   apps/kimi-code/src/tui/utils/dead-terminal.ts
///   isDeadTerminalError()
pub fn is_dead_terminal_error(code: Option<&str>) -> bool {
    matches!(code, Some("EIO" | "EPIPE" | "ENOTCONN"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_only_dead_terminal_error_codes() {
        for code in ["EIO", "EPIPE", "ENOTCONN"] {
            assert!(is_dead_terminal_error(Some(code)));
        }
        for code in [None, Some(""), Some("ENOENT"), Some("EACCES")] {
            assert!(!is_dead_terminal_error(code));
        }
    }
}
