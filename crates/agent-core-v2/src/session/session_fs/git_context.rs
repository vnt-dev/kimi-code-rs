//! Best-effort git context for the explore-agent prompt prefix.
//!
//! Original: `session/sessionFs/gitContext.ts`.

use std::{sync::Arc, time::Duration};

use tokio::io::AsyncWriteExt;

use crate::{
    _base::log::{LogPayload, Logger},
    os::interface::host_process::{ProcessSignal, SharedProcessReader},
    session::process::{ProcessExecOptions, SessionProcess, SessionProcessRunnerContract},
};

use super::fs_process::read_stream;

const GIT_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_DIRTY_FILES: usize = 20;
const MAX_COMMIT_LINE_LENGTH: usize = 200;
const ALLOWED_HOSTS: &[&str] = &[
    "github.com",
    "gitlab.com",
    "gitee.com",
    "bitbucket.org",
    "codeberg.org",
    "git.sr.ht",
];

enum GitResult {
    Ok(String),
    Timeout,
    SpawnError,
    CommandFailed { stderr: Option<String> },
}

impl GitResult {
    fn stdout(&self) -> &str {
        match self {
            Self::Ok(stdout) => stdout,
            Self::Timeout | Self::SpawnError | Self::CommandFailed { .. } => "",
        }
    }
}

/// Collect repository facts for a newly created explore agent.
///
/// Each probe is intentionally independent and best-effort. A repository
/// check failure only produces a visible result for Git's normal
/// `not a git repository` case; all other failures omit the block, matching
/// the source's privacy and prompt-noise behavior.
pub async fn collect_git_context(
    runner: &dyn SessionProcessRunnerContract,
    cwd: &str,
    log: Option<&dyn Logger>,
) -> String {
    let rev_parse_args = ["rev-parse", "--is-inside-work-tree"];
    let rev_parse = run_git(runner, cwd, &rev_parse_args).await;
    if !matches!(rev_parse, GitResult::Ok(_)) {
        if let GitResult::CommandFailed {
            stderr: Some(stderr),
        } = &rev_parse
            && stderr.contains("not a git repository")
        {
            return r#"<git-context status="unavailable" reason="not-a-repo"/>"#.into();
        }
        log_git_failure(cwd, &rev_parse_args, &rev_parse, log);
        return String::new();
    }

    let remote_args = ["remote", "get-url", "origin"];
    let branch_args = ["symbolic-ref", "--short", "HEAD"];
    let status_args = ["status", "--porcelain"];
    let log_args = ["log", "-3", "--format=%h %s"];
    let (remote, branch, status, history) = tokio::join!(
        run_git(runner, cwd, &remote_args),
        run_git(runner, cwd, &branch_args),
        run_git(runner, cwd, &status_args),
        run_git(runner, cwd, &log_args),
    );
    for (args, result) in [
        (&remote_args[..], &remote),
        (&branch_args[..], &branch),
        (&status_args[..], &status),
        (&log_args[..], &history),
    ] {
        if !matches!(result, GitResult::Ok(_)) {
            log_git_failure(cwd, args, result, log);
        }
    }

    let mut sections = vec![format!("Working directory: {cwd}")];
    if let Some(remote) = sanitize_remote_url(remote.stdout()) {
        sections.push(format!("Remote: {remote}"));
        if let Some(project) = parse_project_name(&remote) {
            sections.push(format!("Project: {project}"));
        }
    }
    if !branch.stdout().is_empty() {
        sections.push(format!("Branch: {}", branch.stdout()));
    }
    let dirty = status
        .stdout()
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect::<Vec<_>>();
    if !dirty.is_empty() {
        let mut body = dirty
            .iter()
            .take(MAX_DIRTY_FILES)
            .map(|line| format!("  {line}"))
            .collect::<Vec<_>>()
            .join("\n");
        if dirty.len() > MAX_DIRTY_FILES {
            body.push_str(&format!(
                "\n  ... and {} more",
                dirty.len() - MAX_DIRTY_FILES
            ));
        }
        sections.push(format!("Dirty files ({}):\n{body}", dirty.len()));
    }
    let history = history
        .stdout()
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect::<Vec<_>>();
    if !history.is_empty() {
        let body = history
            .into_iter()
            .map(|line| format!("  {}", truncate_chars(line, MAX_COMMIT_LINE_LENGTH)))
            .collect::<Vec<_>>()
            .join("\n");
        sections.push(format!("Recent commits:\n{body}"));
    }
    if sections.len() == 1 {
        String::new()
    } else {
        format!("<git-context>\n{}\n</git-context>", sections.join("\n"))
    }
}

/// Original: `sanitizeRemoteUrl()`. SCP remote syntax is accepted only for
/// the source allowlist; URL credentials, query strings, and fragments are
/// discarded before rendering the model-visible prompt.
pub fn sanitize_remote_url(remote_url: &str) -> Option<String> {
    for host in ALLOWED_HOSTS {
        if remote_url.starts_with(&format!("git@{host}:")) {
            return Some(remote_url.into());
        }
    }
    let parsed = url::Url::parse(remote_url).ok()?;
    if !ALLOWED_HOSTS.contains(&parsed.host_str()?) {
        return None;
    }
    let port = parsed
        .port()
        .map(|port| format!(":{port}"))
        .unwrap_or_default();
    Some(format!(
        "https://{}{port}{}",
        parsed.host_str()?,
        parsed.path()
    ))
}

/// Original: `parseProjectName()`.
pub fn parse_project_name(remote_url: &str) -> Option<String> {
    let raw_path = remote_url
        .split_once(':')
        .filter(|(prefix, _)| prefix.contains('@') && !prefix.contains('/'))
        .map(|(_, path)| path.to_owned())
        .or_else(|| {
            url::Url::parse(remote_url)
                .ok()
                .map(|url| url.path().into())
        })?;
    let normalized = raw_path.trim_start_matches('/').trim_end_matches('/');
    let project = normalized
        .strip_suffix(".git")
        .unwrap_or(normalized)
        .to_owned();
    (!project.is_empty()).then_some(project)
}

async fn run_git(runner: &dyn SessionProcessRunnerContract, cwd: &str, args: &[&str]) -> GitResult {
    let command = std::iter::once("git".to_owned())
        .chain(std::iter::once("-C".to_owned()))
        .chain(std::iter::once(cwd.to_owned()))
        .chain(args.iter().map(|arg| (*arg).to_owned()))
        .collect::<Vec<_>>();
    let process = match runner
        .exec(
            &command,
            Some(ProcessExecOptions {
                cwd: None,
                env: None,
            }),
        )
        .await
    {
        Ok(process) => process,
        Err(_) => return GitResult::SpawnError,
    };
    close_stdin(&process).await;
    let mut work = Box::pin(collect_process(Arc::clone(&process)));
    let result = tokio::select! {
        result = &mut work => result,
        () = tokio::time::sleep(GIT_TIMEOUT) => {
            let _ = process.kill(Some(ProcessSignal::Kill)).await;
            let _ = work.await;
            Err(())
        }
    };
    process.dispose();
    match result {
        Ok((stdout, _stderr, 0)) => GitResult::Ok(stdout.trim().into()),
        Ok((_, stderr, _)) => GitResult::CommandFailed {
            stderr: Some(stderr.trim().into()),
        },
        Err(()) => GitResult::Timeout,
    }
}

async fn close_stdin(process: &SessionProcess) {
    let stdin_handle = process.stdin();
    let mut stdin = stdin_handle.lock().await;
    let _ = stdin.shutdown().await;
}

async fn collect_process(process: SessionProcess) -> Result<(String, String, i32), ()> {
    let (stdout, stderr, exit_code) = tokio::try_join!(
        collect_stream(process.stdout()),
        collect_stream(process.stderr()),
        async { process.wait().await.map_err(|_| ()) },
    )?;
    Ok((stdout, stderr, exit_code))
}

async fn collect_stream(stream: SharedProcessReader) -> Result<String, ()> {
    read_stream(stream).await.map_err(|_| ())
}

fn log_git_failure(cwd: &str, args: &[&str], failure: &GitResult, log: Option<&dyn Logger>) {
    let Some(log) = log else { return };
    let context = serde_json::Map::from_iter([
        ("cwd".into(), serde_json::Value::String(cwd.into())),
        (
            "command".into(),
            serde_json::Value::String(format!("git {}", args.join(" "))),
        ),
    ]);
    match failure {
        GitResult::Timeout => log.debug(
            "git context command timed out",
            Some(LogPayload::Context(context)),
        ),
        GitResult::SpawnError => log.warn(
            "git context command failed to spawn",
            Some(LogPayload::Context(context)),
        ),
        GitResult::CommandFailed { .. } => log.debug(
            "git context command failed",
            Some(LogPayload::Context(context)),
        ),
        GitResult::Ok(_) => {}
    }
}

fn truncate_chars(value: &str, maximum: usize) -> String {
    value.chars().take(maximum).collect()
}

#[cfg(test)]
mod tests {
    use parking_lot::Mutex;
    use std::collections::HashMap;

    use async_trait::async_trait;
    use tokio::{
        io::{duplex, sink},
        sync::Mutex as AsyncMutex,
    };

    use crate::os::interface::host_process::{
        HostProcess, HostProcessError, ProcessSignal, SharedProcessWriter,
    };

    use super::*;

    #[derive(Clone)]
    struct Script {
        stdout: &'static str,
        stderr: &'static str,
        exit_code: i32,
    }

    struct ScriptedRunner {
        scripts: HashMap<String, Script>,
        commands: Mutex<Vec<Vec<String>>>,
    }

    struct ScriptedProcess {
        stdout: SharedProcessReader,
        stderr: SharedProcessReader,
        exit_code: i32,
    }

    #[async_trait]
    impl HostProcess for ScriptedProcess {
        fn pid(&self) -> i64 {
            1
        }
        fn exit_code(&self) -> Option<i32> {
            Some(self.exit_code)
        }
        fn stdin(&self) -> SharedProcessWriter {
            Arc::new(AsyncMutex::new(Box::new(sink())))
        }
        fn stdout(&self) -> SharedProcessReader {
            Arc::clone(&self.stdout)
        }
        fn stderr(&self) -> SharedProcessReader {
            Arc::clone(&self.stderr)
        }
        async fn wait(&self) -> Result<i32, HostProcessError> {
            Ok(self.exit_code)
        }
        async fn kill(&self, _: Option<ProcessSignal>) -> Result<(), HostProcessError> {
            Ok(())
        }
        fn dispose(&self) {}
    }

    #[async_trait]
    impl SessionProcessRunnerContract for ScriptedRunner {
        async fn exec(
            &self,
            args: &[String],
            _: Option<ProcessExecOptions>,
        ) -> crate::session::process::SessionProcessRunnerResult<SessionProcess> {
            self.commands.lock().push(args.to_vec());
            let script = self.scripts[&args[3..].join(" ")].clone();
            let stdout = reader(script.stdout).await;
            let stderr = reader(script.stderr).await;
            Ok(Arc::new(ScriptedProcess {
                stdout,
                stderr,
                exit_code: script.exit_code,
            }))
        }
    }

    async fn reader(text: &str) -> SharedProcessReader {
        let (mut writer, reader) = duplex(text.len().max(1));
        writer.write_all(text.as_bytes()).await.unwrap();
        drop(writer);
        Arc::new(AsyncMutex::new(Box::new(reader)))
    }

    fn runner(scripts: &[(&str, Script)]) -> ScriptedRunner {
        ScriptedRunner {
            scripts: scripts
                .iter()
                .map(|(name, script)| ((*name).into(), script.clone()))
                .collect(),
            commands: Mutex::new(Vec::new()),
        }
    }

    #[test]
    fn sanitizes_only_allowed_remotes_without_credentials_or_query_data() {
        assert_eq!(
            sanitize_remote_url("git@github.com:org/project.git"),
            Some("git@github.com:org/project.git".into())
        );
        assert_eq!(
            sanitize_remote_url("https://token@example.invalid/a.git"),
            None
        );
        assert_eq!(
            sanitize_remote_url("https://user:secret@github.com/org/project.git?x=1#top"),
            Some("https://github.com/org/project.git".into())
        );
    }

    #[test]
    fn parses_scp_and_url_project_names_like_the_source() {
        assert_eq!(
            parse_project_name("git@gitlab.com:group/project.git"),
            Some("group/project".into())
        );
        assert_eq!(
            parse_project_name("https://github.com/org/project.git"),
            Some("org/project".into())
        );
        assert_eq!(parse_project_name("not a URL"), None);
    }

    #[tokio::test]
    async fn renders_successful_probes_and_only_surfaces_not_a_repo_failures() {
        let success = runner(&[
            (
                "rev-parse --is-inside-work-tree",
                Script {
                    stdout: "true\n",
                    stderr: "",
                    exit_code: 0,
                },
            ),
            (
                "remote get-url origin",
                Script {
                    stdout: "git@github.com:org/project.git\n",
                    stderr: "",
                    exit_code: 0,
                },
            ),
            (
                "symbolic-ref --short HEAD",
                Script {
                    stdout: "main\n",
                    stderr: "",
                    exit_code: 0,
                },
            ),
            (
                "status --porcelain",
                Script {
                    stdout: " M src/lib.rs\n?? new.rs\n",
                    stderr: "",
                    exit_code: 0,
                },
            ),
            (
                "log -3 --format=%h %s",
                Script {
                    stdout: "abcd change\n",
                    stderr: "",
                    exit_code: 0,
                },
            ),
        ]);
        assert_eq!(
            collect_git_context(&success, "/repo", None).await,
            "<git-context>\nWorking directory: /repo\nRemote: git@github.com:org/project.git\nProject: org/project\nBranch: main\nDirty files (2):\n  M src/lib.rs\n  ?? new.rs\nRecent commits:\n  abcd change\n</git-context>"
        );
        assert!(
            success
                .commands
                .lock()
                .iter()
                .all(|args| args[..3] == ["git", "-C", "/repo"])
        );

        let not_a_repo = runner(&[(
            "rev-parse --is-inside-work-tree",
            Script {
                stdout: "",
                stderr: "fatal: not a git repository\n",
                exit_code: 128,
            },
        )]);
        assert_eq!(
            collect_git_context(&not_a_repo, "/plain", None).await,
            r#"<git-context status="unavailable" reason="not-a-repo"/>"#
        );
    }
}
