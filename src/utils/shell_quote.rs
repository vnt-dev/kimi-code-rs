#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellPlatform {
    Posix,
    WindowsCmd,
}

pub fn current_shell_platform() -> ShellPlatform {
    if cfg!(windows) {
        ShellPlatform::WindowsCmd
    } else {
        ShellPlatform::Posix
    }
}

// Original:
//   apps/kimi-code/src/utils/shell-quote.ts
//   quoteShellArg()
pub fn quote_shell_arg(value: &str) -> String {
    quote_shell_arg_for(value, current_shell_platform())
}

pub fn quote_shell_arg_for(value: &str, platform: ShellPlatform) -> String {
    match platform {
        ShellPlatform::Posix => format!("'{}'", value.replace('\'', "'\\''")),
        ShellPlatform::WindowsCmd => format!("\"{}\"", value.replace('"', "\\\"")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_posix_arguments_with_the_original_single_quote_sequence() {
        assert_eq!(
            quote_shell_arg_for("hello world", ShellPlatform::Posix),
            "'hello world'"
        );
        assert_eq!(
            quote_shell_arg_for("it's ready", ShellPlatform::Posix),
            "'it'\\''s ready'"
        );
        assert_eq!(quote_shell_arg_for("", ShellPlatform::Posix), "''");
    }

    #[test]
    fn quotes_cmd_arguments_and_preserves_backslash_quote_quirk() {
        assert_eq!(
            quote_shell_arg_for("hello world", ShellPlatform::WindowsCmd),
            "\"hello world\""
        );
        assert_eq!(
            quote_shell_arg_for("say \"hi\"", ShellPlatform::WindowsCmd),
            "\"say \\\"hi\\\"\""
        );
        assert_eq!(quote_shell_arg_for("", ShellPlatform::WindowsCmd), "\"\"");
    }
}
