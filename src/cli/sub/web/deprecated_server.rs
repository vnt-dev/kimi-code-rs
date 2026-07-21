pub const DEPRECATED_SERVER_NOTICE: &str = "`kimi server` has been deprecated and no longer works.\n\
Use `kimi web` instead — it runs the local server in the foreground and opens the web UI (`--no-open` to skip).\n\
To stop a server started by a version before 0.28.0, use `kimi server kill`.\n\
This notice will be removed in the next major version of Kimi Code.\n";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeprecatedServerDisposition {
    Exit(i32),
}

pub trait DeprecatedServerRuntime {
    fn write_stderr(&self, text: &str);
}

// Original:
//   apps/kimi-code/src/cli/sub/web/deprecated-server.ts
//   registerDeprecatedServerCommand().action()
//
// The CLI parser owns swallowing the legacy positional arguments and flags;
// every non-kill invocation reaches this single behavior-preserving handler.
pub fn handle_deprecated_server(
    runtime: &dyn DeprecatedServerRuntime,
) -> DeprecatedServerDisposition {
    runtime.write_stderr(DEPRECATED_SERVER_NOTICE);
    DeprecatedServerDisposition::Exit(1)
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    #[derive(Default)]
    struct RuntimeMock {
        stderr: Mutex<String>,
    }

    impl DeprecatedServerRuntime for RuntimeMock {
        fn write_stderr(&self, text: &str) {
            self.stderr.lock().expect("stderr").push_str(text);
        }
    }

    #[test]
    fn prints_the_complete_notice_and_exits_one() {
        let runtime = RuntimeMock::default();
        let disposition = handle_deprecated_server(&runtime);

        assert_eq!(disposition, DeprecatedServerDisposition::Exit(1));
        assert_eq!(
            runtime.stderr.lock().expect("stderr").as_str(),
            DEPRECATED_SERVER_NOTICE
        );
        for required in [
            "`kimi server` has been deprecated and no longer works.",
            "kimi web",
            "kimi server kill",
            "0.28.0",
            "next major version",
        ] {
            assert!(DEPRECATED_SERVER_NOTICE.contains(required));
        }
    }
}
