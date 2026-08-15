//! Login-shell `PATH` overlay.
//!
//! A login shell is a unix concept; Windows has no login shell, so the whole
//! probe/apply chain is compiled out there and call sites are gated with
//! `#[cfg(unix)]`. `merge_login_shell_path` is platform-neutral and remains
//! available everywhere.

#[cfg(unix)]
use std::collections::HashMap;

#[cfg(unix)]
use std::time::Duration;

#[cfg(unix)]
use super::environment_probe::ExecFileText;

#[cfg(unix)]
const LOGIN_SHELL_ENV_TIMEOUT: Duration = Duration::from_secs(5);

/// Probes the login shell's `PATH` by running a login shell.
#[cfg(unix)]
pub async fn probe_login_shell_path(
    env: &HashMap<String, String>,
    user_shell: impl FnOnce() -> Option<String>,
    exec_file_text: &ExecFileText,
) -> Option<String> {
    let shell = env
        .get("SHELL")
        .map(String::as_str)
        .map(str::trim)
        .filter(|shell| !shell.is_empty())
        .map(str::to_owned)
        .or_else(user_shell)?;
    if shell.is_empty() {
        return None;
    }
    let output = exec_file_text(
        shell,
        vec!["-l".into(), "-c".into(), "/usr/bin/env".into()],
        LOGIN_SHELL_ENV_TIMEOUT,
    )
    .await?;
    output
        .lines()
        .filter_map(|line| line.strip_prefix("PATH=").map(str::trim))
        .rfind(|path| !path.is_empty())
        .map(str::to_owned)
}

// Original: mergeLoginShellPath(); current entries retain order and priority.
pub fn merge_login_shell_path(current_path: Option<&str>, login_shell_path: &str) -> String {
    let current = current_path.unwrap_or_default();
    let mut seen = current
        .split(':')
        .filter(|entry| !entry.is_empty())
        .collect::<std::collections::HashSet<_>>();
    let additions = login_shell_path
        .split(':')
        .filter(|entry| entry.starts_with('/') && seen.insert(entry))
        .collect::<Vec<_>>();
    if additions.is_empty() {
        return current.to_owned();
    }
    if current_path.is_none() {
        additions.join(":")
    } else {
        format!("{current}:{}", additions.join(":"))
    }
}

#[cfg(unix)]
pub async fn apply_login_shell_path(
    env: &mut HashMap<String, String>,
    user_shell: impl FnOnce() -> Option<String>,
    exec_file_text: &ExecFileText,
) {
    let Some(login_path) = probe_login_shell_path(env, user_shell, exec_file_text).await else {
        return;
    };
    let current = env.get("PATH").cloned();
    let merged = merge_login_shell_path(current.as_deref(), &login_path);
    if merged != current.as_deref().unwrap_or_default() {
        env.insert("PATH".into(), merged);
    }
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    use parking_lot::Mutex;
    #[cfg(unix)]
    use std::sync::Arc;

    use super::*;

    #[cfg(unix)]
    #[tokio::test]
    async fn probes_last_path_and_uses_account_shell_fallback() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let exec: ExecFileText = Arc::new({
            let calls = Arc::clone(&calls);
            move |file, args, timeout| {
                calls.lock().push((file, args, timeout));
                Box::pin(async { Some("PATH=/noise\nbanner\nPATH=/real:/usr/bin\n".into()) })
            }
        });
        let path = probe_login_shell_path(&HashMap::new(), || Some("/bin/zsh".into()), &exec).await;
        assert_eq!(path.as_deref(), Some("/real:/usr/bin"));
        assert_eq!(calls.lock()[0].0, "/bin/zsh");
        assert_eq!(calls.lock()[0].2, Duration::from_secs(5));
    }

    #[test]
    fn merge_preserves_current_shape_and_adds_only_absolute_unique_entries() {
        assert_eq!(
            merge_login_shell_path(Some("/usr/bin:/bin"), "/opt/homebrew/bin:/usr/bin:/extra"),
            "/usr/bin:/bin:/opt/homebrew/bin:/extra"
        );
        assert_eq!(
            merge_login_shell_path(Some("/a::/b:/a:"), "/b:/a"),
            "/a::/b:/a:"
        );
        assert_eq!(merge_login_shell_path(Some(""), "/a"), ":/a");
        assert_eq!(merge_login_shell_path(None, "/a:/b"), "/a:/b");
        assert_eq!(merge_login_shell_path(Some("/a"), ".:bin:../x:/b"), "/a:/b");
    }
}
