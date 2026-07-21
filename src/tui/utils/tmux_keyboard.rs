use std::{future::Future, process::Stdio, time::Duration};

use tokio::{process::Command, time::timeout};

use super::terminal_notification::{Environment, ProcessEnvironment};

const TMUX_QUERY_TIMEOUT: Duration = Duration::from_millis(2_000);

pub const TMUX_EXTENDED_KEYS_OFF_WARNING: &str = "tmux extended-keys is off. Modified Enter keys may not work. Add `set -g extended-keys on` to ~/.tmux.conf and restart tmux.";
pub const TMUX_EXTENDED_KEYS_FORMAT_XTERM_WARNING: &str = "tmux extended-keys-format is xterm. Kimi Code works best with csi-u. Add `set -g extended-keys-format csi-u` to ~/.tmux.conf and restart tmux.";

pub trait TmuxOptionReader {
    fn read_tmux_option(&self, option: &str) -> impl Future<Output = Option<String>> + Send;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct ProcessTmuxOptionReader;

impl TmuxOptionReader for ProcessTmuxOptionReader {
    async fn read_tmux_option(&self, option: &str) -> Option<String> {
        read_tmux_option_from_process(option).await
    }
}

/// Original:
///   apps/kimi-code/src/tui/utils/tmux-keyboard.ts
///   detectTmuxKeyboardWarning()
pub async fn detect_tmux_keyboard_warning<R>(
    env: &dyn Environment,
    reader: &R,
) -> Option<&'static str>
where
    R: TmuxOptionReader + ?Sized,
{
    if env.value("TMUX").is_none_or(|value| value.is_empty()) {
        return None;
    }

    // The original uses Promise.all, so both independent tmux queries begin
    // together and neither warning path delays the other query.
    let (extended_keys, extended_keys_format) = tokio::join!(
        reader.read_tmux_option("extended-keys"),
        reader.read_tmux_option("extended-keys-format")
    );

    let extended_keys = extended_keys?;
    if extended_keys != "on" && extended_keys != "always" {
        return Some(TMUX_EXTENDED_KEYS_OFF_WARNING);
    }
    if extended_keys_format.as_deref() == Some("xterm") {
        return Some(TMUX_EXTENDED_KEYS_FORMAT_XTERM_WARNING);
    }
    None
}

pub async fn detect_process_tmux_keyboard_warning() -> Option<&'static str> {
    detect_tmux_keyboard_warning(&ProcessEnvironment, &ProcessTmuxOptionReader).await
}

async fn read_tmux_option_from_process(option: &str) -> Option<String> {
    let mut command = Command::new("tmux");
    command
        .args(["show", "-gv", option])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    let child = command.spawn().ok()?;
    let output = timeout(TMUX_QUERY_TIMEOUT, child.wait_with_output())
        .await
        .ok()?
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

#[cfg(test)]
mod tests {
    use std::{collections::HashMap, sync::Mutex};

    use super::*;

    struct MapReader {
        values: HashMap<String, Option<String>>,
        calls: Mutex<Vec<String>>,
    }

    impl MapReader {
        fn new(values: &[(&str, Option<&str>)]) -> Self {
            Self {
                values: values
                    .iter()
                    .map(|(key, value)| ((*key).to_owned(), value.map(str::to_owned)))
                    .collect(),
                calls: Mutex::new(Vec::new()),
            }
        }

        fn call_count(&self) -> usize {
            self.calls.lock().map_or(0, |calls| calls.len())
        }
    }

    impl TmuxOptionReader for MapReader {
        async fn read_tmux_option(&self, option: &str) -> Option<String> {
            if let Ok(mut calls) = self.calls.lock() {
                calls.push(option.to_owned());
            }
            self.values.get(option).cloned().flatten()
        }
    }

    fn tmux_env() -> HashMap<String, String> {
        HashMap::from([("TMUX".to_owned(), "/tmp/tmux/default,123,0".to_owned())])
    }

    #[tokio::test]
    async fn skips_checks_outside_tmux() {
        let reader = MapReader::new(&[]);
        assert_eq!(
            detect_tmux_keyboard_warning(&HashMap::new(), &reader).await,
            None
        );
        assert_eq!(reader.call_count(), 0);
    }

    #[tokio::test]
    async fn does_not_warn_when_options_cannot_be_queried() {
        let reader = MapReader::new(&[("extended-keys", None), ("extended-keys-format", None)]);
        assert_eq!(
            detect_tmux_keyboard_warning(&tmux_env(), &reader).await,
            None
        );
        assert_eq!(reader.call_count(), 2);
    }

    #[tokio::test]
    async fn warns_for_disabled_extended_keys_before_format() {
        let reader = MapReader::new(&[
            ("extended-keys", Some("off")),
            ("extended-keys-format", Some("xterm")),
        ]);
        assert_eq!(
            detect_tmux_keyboard_warning(&tmux_env(), &reader).await,
            Some(TMUX_EXTENDED_KEYS_OFF_WARNING)
        );
    }

    #[tokio::test]
    async fn warns_for_xterm_format() {
        let reader = MapReader::new(&[
            ("extended-keys", Some("on")),
            ("extended-keys-format", Some("xterm")),
        ]);
        assert_eq!(
            detect_tmux_keyboard_warning(&tmux_env(), &reader).await,
            Some(TMUX_EXTENDED_KEYS_FORMAT_XTERM_WARNING)
        );
    }

    #[tokio::test]
    async fn accepts_on_or_always_with_compatible_format() {
        for (extended_keys, format) in [("on", Some("csi-u")), ("always", None)] {
            let reader = MapReader::new(&[
                ("extended-keys", Some(extended_keys)),
                ("extended-keys-format", format),
            ]);
            assert_eq!(
                detect_tmux_keyboard_warning(&tmux_env(), &reader).await,
                None
            );
        }
    }
}
