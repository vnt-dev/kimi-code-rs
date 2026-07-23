//! App-scoped local Git service.
//!
//! Original: `packages/agent-core-v2/src/app/git/gitService.ts`.

use std::{
    collections::{HashMap, HashSet},
    io,
    path::Path,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use async_trait::async_trait;
use serde_json::{Map, Value};
use tokio::io::AsyncReadExt;

use crate::{
    _base::{
        di::{
            descriptors::SyncDescriptor,
            instantiation::ServicesAccessorExt,
            scope::{InstantiationType, LifecycleScope, register_scoped_service},
        },
        errors::errors::{Error2, Error2Options},
    },
    os::interface::{
        host_file_system::{HOST_FILE_SYSTEM_SERVICE_ID, HostFileSystemService},
        host_process::{
            HOST_PROCESS_SERVICE_ID, HostProcess, HostProcessOptions, HostProcessService,
            ProcessSignal, SharedProcessReader,
        },
    },
    session::session_fs::errors::{
        FS_GIT_UNAVAILABLE, FS_PATH_NOT_FOUND, ensure_fs_errors_registered,
    },
};

use super::{
    contract::{
        FS_GIT_SERVICE_ID, FsDiffResponse, FsGitStatusResponse, FsPullRequest, GitServiceContract,
        GitServiceHandle, GitServiceResult,
    },
    parsers::{parse_numstat, parse_porcelain, parse_pull_request},
};

const DIFF_MAX_UTF16_UNITS: usize = 1_048_576;
const PR_SPAWN_TIMEOUT: Duration = Duration::from_secs(5);
const PULL_REQUEST_TTL: Duration = Duration::from_secs(60);

#[derive(Clone)]
struct PullRequestCacheEntry {
    value: Option<FsPullRequest>,
    fetched_at: Instant,
}

pub struct GitService {
    host_process: Arc<dyn HostProcessService>,
    fs: Arc<dyn HostFileSystemService>,
    pull_request_cache: Mutex<HashMap<String, PullRequestCacheEntry>>,
}

impl GitService {
    pub fn new(
        host_process: Arc<dyn HostProcessService>,
        fs: Arc<dyn HostFileSystemService>,
    ) -> Self {
        ensure_fs_errors_registered();
        Self {
            host_process,
            fs,
            pull_request_cache: Mutex::new(HashMap::new()),
        }
    }

    // Original: GitService.readPullRequest(). Cache age starts before spawning,
    // matching the source's `fetchedAt: now` behavior.
    async fn read_pull_request(&self, cwd: &str) -> GitServiceResult<Option<FsPullRequest>> {
        let now = Instant::now();
        if let Some(cached) = self.pull_request_cache.lock().unwrap().get(cwd)
            && now.duration_since(cached.fetched_at) < PULL_REQUEST_TTL
        {
            return Ok(cached.value.clone());
        }

        let result = self
            .run_command(
                "gh",
                &strings(&["pr", "view", "--json", "number,url,state"]),
                cwd,
                RunOptions {
                    timeout: Some(PR_SPAWN_TIMEOUT),
                    env: Some(HashMap::from([
                        ("GH_NO_UPDATE_NOTIFIER".into(), "1".into()),
                        ("GH_PROMPT_DISABLED".into(), "1".into()),
                    ])),
                },
            )
            .await?;
        let value = (result.exit_code == 0)
            .then(|| parse_pull_request(&result.stdout))
            .flatten();
        self.pull_request_cache.lock().unwrap().insert(
            cwd.to_owned(),
            PullRequestCacheEntry {
                value: value.clone(),
                fetched_at: now,
            },
        );
        Ok(value)
    }

    // Original: GitService.runCommand(). Spawn and wait errors become exit -1;
    // output stream failures still propagate. A timed-out command is killed and
    // drained before returning so no unmanaged task remains.
    async fn run_command(
        &self,
        command: &str,
        args: &[String],
        cwd: &str,
        options: RunOptions,
    ) -> Result<RunResult, io::Error> {
        let process = match self
            .host_process
            .spawn(
                command,
                args,
                HostProcessOptions {
                    cwd: Some(cwd.to_owned()),
                    env: options.env,
                    ..HostProcessOptions::default()
                },
            )
            .await
        {
            Ok(process) => process,
            Err(_) => return Ok(RunResult::failed()),
        };
        let _dispose = ProcessDisposeGuard(Arc::clone(&process));
        let work = collect_process(Arc::clone(&process));
        tokio::pin!(work);

        let Some(timeout) = options.timeout else {
            return work.await;
        };
        tokio::select! {
            biased;
            result = &mut work => result,
            () = tokio::time::sleep(timeout) => {
                let _ = process.kill(Some(ProcessSignal::Kill)).await;
                Ok(work.await.unwrap_or_else(|_| RunResult::failed()))
            }
        }
    }

    fn git_unavailable(cwd: &str, detail: String) -> Error2 {
        Error2::with_options(
            FS_GIT_UNAVAILABLE,
            format!("git unavailable at {cwd}: {detail}"),
            Error2Options {
                details: Some(Map::from_iter([
                    ("cwd".into(), Value::String(cwd.into())),
                    ("detail".into(), Value::String(detail)),
                ])),
                ..Error2Options::default()
            },
        )
    }
}

#[async_trait]
impl GitServiceContract for GitService {
    // Original: GitService.status(). Commands remain sequential and retain the
    // original best-effort numstat and pull-request behavior.
    async fn status(
        &self,
        cwd: &str,
        path_filter: Option<&HashSet<String>>,
    ) -> GitServiceResult<FsGitStatusResponse> {
        let inside = self
            .run_command(
                "git",
                &strings(&["rev-parse", "--is-inside-work-tree"]),
                cwd,
                RunOptions::default(),
            )
            .await?;
        if inside.exit_code != 0 || inside.stdout.trim() != "true" {
            let detail = nonempty_or(
                inside.stderr.trim(),
                format!("git rev-parse exit {}", inside.exit_code),
            );
            return Err(Box::new(Self::git_unavailable(cwd, detail)));
        }

        let porcelain = self
            .run_command(
                "git",
                &strings(&["status", "--porcelain=v1", "--branch"]),
                cwd,
                RunOptions::default(),
            )
            .await?;
        if porcelain.exit_code != 0 {
            let detail = nonempty_or(
                porcelain.stderr.trim(),
                format!("git status exit {}", porcelain.exit_code),
            );
            return Err(Box::new(Self::git_unavailable(cwd, detail)));
        }

        let mut response = parse_porcelain(&porcelain.stdout, path_filter);
        let dirty = porcelain
            .stdout
            .split('\n')
            .any(|line| !line.is_empty() && !line.starts_with("## "));
        if dirty {
            let head = self
                .run_command(
                    "git",
                    &strings(&["rev-parse", "--verify", "--quiet", "HEAD"]),
                    cwd,
                    RunOptions::default(),
                )
                .await?;
            if head.exit_code == 0 {
                let numstat = self
                    .run_command(
                        "git",
                        &strings(&["diff", "--no-color", "--numstat", "HEAD", "--"]),
                        cwd,
                        RunOptions::default(),
                    )
                    .await?;
                if numstat.exit_code == 0 {
                    let summary = parse_numstat(&numstat.stdout);
                    response.additions = summary.additions;
                    response.deletions = summary.deletions;
                }
            }
        }
        response.pull_request = self.read_pull_request(cwd).await?;
        Ok(response)
    }

    // Original: GitService.diff(). `/dev/null` is intentionally preserved as
    // part of the existing Git CLI protocol.
    async fn diff(
        &self,
        cwd: &str,
        relative_path: &str,
        absolute_path: &str,
    ) -> GitServiceResult<FsDiffResponse> {
        let inside = self
            .run_command(
                "git",
                &strings(&["rev-parse", "--is-inside-work-tree"]),
                cwd,
                RunOptions::default(),
            )
            .await?;
        if inside.exit_code != 0 || inside.stdout.trim() != "true" {
            let detail = nonempty_or(
                inside.stderr.trim(),
                format!("git rev-parse exit {}", inside.exit_code),
            );
            return Err(Box::new(Self::git_unavailable(cwd, detail)));
        }

        let status = self
            .run_command(
                "git",
                &[
                    "status".into(),
                    "--porcelain=v1".into(),
                    "--".into(),
                    relative_path.into(),
                ],
                cwd,
                RunOptions::default(),
            )
            .await?;
        if status.exit_code != 0 {
            let detail = nonempty_or(
                status.stderr.trim(),
                format!("git status exit {}", status.exit_code),
            );
            return Err(Box::new(Self::git_unavailable(cwd, detail)));
        }
        let untracked = status.stdout.starts_with("??");
        let head = self
            .run_command(
                "git",
                &strings(&["rev-parse", "--verify", "--quiet", "HEAD"]),
                cwd,
                RunOptions::default(),
            )
            .await?;

        let diff = if untracked || head.exit_code != 0 {
            let result = self
                .run_command(
                    "git",
                    &[
                        "diff".into(),
                        "--no-color".into(),
                        "--no-index".into(),
                        "--".into(),
                        "/dev/null".into(),
                        relative_path.into(),
                    ],
                    cwd,
                    RunOptions::default(),
                )
                .await?;
            if !matches!(result.exit_code, 0 | 1) {
                let detail = nonempty_or(
                    result.stderr.trim(),
                    format!("git diff exit {}", result.exit_code),
                );
                return Err(Box::new(Self::git_unavailable(cwd, detail)));
            }
            result.stdout
        } else {
            let result = self
                .run_command(
                    "git",
                    &[
                        "diff".into(),
                        "--no-color".into(),
                        "HEAD".into(),
                        "--".into(),
                        relative_path.into(),
                    ],
                    cwd,
                    RunOptions::default(),
                )
                .await?;
            if result.exit_code != 0 {
                let detail = nonempty_or(
                    result.stderr.trim(),
                    format!("git diff exit {}", result.exit_code),
                );
                return Err(Box::new(Self::git_unavailable(cwd, detail)));
            }
            if result.stdout.is_empty()
                && status.stdout.is_empty()
                && self.fs.lstat(Path::new(absolute_path)).await.is_err()
            {
                return Err(Box::new(Error2::with_options(
                    FS_PATH_NOT_FOUND,
                    format!("path not found: {relative_path}"),
                    Error2Options {
                        details: Some(Map::from_iter([(
                            "path".into(),
                            Value::String(relative_path.into()),
                        )])),
                        ..Error2Options::default()
                    },
                )));
            }
            result.stdout
        };

        let (diff, truncated) = truncate_diff(diff, DIFF_MAX_UTF16_UNITS);
        Ok(FsDiffResponse {
            path: relative_path.into(),
            diff,
            truncated,
        })
    }
}

#[derive(Default)]
struct RunOptions {
    timeout: Option<Duration>,
    env: Option<HashMap<String, String>>,
}

#[derive(Debug, Eq, PartialEq)]
struct RunResult {
    exit_code: i32,
    stdout: String,
    stderr: String,
}

impl RunResult {
    fn failed() -> Self {
        Self {
            exit_code: -1,
            stdout: String::new(),
            stderr: String::new(),
        }
    }
}

struct ProcessDisposeGuard(Arc<dyn HostProcess>);

impl Drop for ProcessDisposeGuard {
    fn drop(&mut self) {
        self.0.dispose();
    }
}

async fn collect_process(process: Arc<dyn HostProcess>) -> Result<RunResult, io::Error> {
    let stdout = collect(process.stdout());
    let stderr = collect(process.stderr());
    let wait = async { Ok::<_, io::Error>(process.wait().await.unwrap_or(-1)) };
    let (stdout, stderr, exit_code) = tokio::try_join!(stdout, stderr, wait)?;
    Ok(RunResult {
        exit_code,
        stdout,
        stderr,
    })
}

async fn collect(stream: SharedProcessReader) -> Result<String, io::Error> {
    let mut stream = stream.lock().await;
    let mut bytes = Vec::new();
    stream.read_to_end(&mut bytes).await?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

fn nonempty_or(value: &str, fallback: String) -> String {
    if value.is_empty() {
        fallback
    } else {
        value.to_owned()
    }
}

fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
}

fn truncate_diff(mut diff: String, max_utf16_units: usize) -> (String, bool) {
    if diff.encode_utf16().count() <= max_utf16_units {
        return (diff, false);
    }
    let mut units = 0;
    let mut boundary = 0;
    // JavaScript `slice` can end between a surrogate pair. Rust `String`
    // cannot represent that isolated surrogate, so the Rust adaptation stops
    // before the scalar only in that boundary case.
    for (index, character) in diff.char_indices() {
        let next = units + character.len_utf16();
        if next > max_utf16_units {
            break;
        }
        units = next;
        boundary = index + character.len_utf8();
    }
    diff.truncate(boundary);
    (diff, true)
}

pub fn register_git_service() {
    ensure_fs_errors_registered();
    register_scoped_service(
        LifecycleScope::App,
        FS_GIT_SERVICE_ID,
        SyncDescriptor::new(|accessor| {
            let host_process = accessor.get(HOST_PROCESS_SERVICE_ID)?;
            let fs = accessor.get(HOST_FILE_SYSTEM_SERVICE_ID)?;
            let service: Arc<dyn GitServiceContract> = Arc::new(GitService::new(
                Arc::clone(&host_process.0),
                Arc::clone(&fs.0),
            ));
            Ok(GitServiceHandle(service))
        }),
        InstantiationType::Eager,
        "git",
    );
}

#[cfg(test)]
mod tests {
    use std::{
        collections::VecDeque,
        sync::atomic::{AtomicBool, Ordering},
    };

    use tokio::{
        io::sink,
        sync::{Mutex as AsyncMutex, Notify},
    };

    use crate::{
        os::{
            backends::node_local::host_fs_service::HostFileSystem,
            interface::host_process::{
                HostProcessError, ProcessReader, ProcessWriter, SharedProcessWriter,
            },
        },
        session::session_fs::errors::{FS_GIT_UNAVAILABLE, FS_PATH_NOT_FOUND},
    };

    use super::*;

    #[derive(Clone)]
    struct ProcessSpec {
        exit_code: i32,
        stdout: String,
        stderr: String,
        wait_for_kill: bool,
    }

    impl ProcessSpec {
        fn new(exit_code: i32, stdout: &str, stderr: &str) -> Self {
            Self {
                exit_code,
                stdout: stdout.into(),
                stderr: stderr.into(),
                wait_for_kill: false,
            }
        }

        fn hanging() -> Self {
            Self {
                exit_code: -1,
                stdout: String::new(),
                stderr: String::new(),
                wait_for_kill: true,
            }
        }
    }

    struct StubProcess {
        spec: ProcessSpec,
        stdin: SharedProcessWriter,
        stdout: SharedProcessReader,
        stderr: SharedProcessReader,
        disposed: AtomicBool,
        killed: Mutex<Vec<ProcessSignal>>,
        kill_notification: Notify,
    }

    impl StubProcess {
        fn new(spec: ProcessSpec) -> Self {
            let stdout: ProcessReader = Box::new(std::io::Cursor::new(spec.stdout.clone()));
            let stderr: ProcessReader = Box::new(std::io::Cursor::new(spec.stderr.clone()));
            let stdin: ProcessWriter = Box::new(sink());
            Self {
                spec,
                stdin: Arc::new(AsyncMutex::new(stdin)),
                stdout: Arc::new(AsyncMutex::new(stdout)),
                stderr: Arc::new(AsyncMutex::new(stderr)),
                disposed: AtomicBool::new(false),
                killed: Mutex::new(Vec::new()),
                kill_notification: Notify::new(),
            }
        }
    }

    #[async_trait]
    impl HostProcess for StubProcess {
        fn pid(&self) -> i64 {
            1
        }
        fn exit_code(&self) -> Option<i32> {
            Some(self.spec.exit_code)
        }
        fn stdin(&self) -> SharedProcessWriter {
            Arc::clone(&self.stdin)
        }
        fn stdout(&self) -> SharedProcessReader {
            Arc::clone(&self.stdout)
        }
        fn stderr(&self) -> SharedProcessReader {
            Arc::clone(&self.stderr)
        }
        async fn wait(&self) -> Result<i32, HostProcessError> {
            if self.spec.wait_for_kill {
                self.kill_notification.notified().await;
            }
            Ok(self.spec.exit_code)
        }
        async fn kill(&self, signal: Option<ProcessSignal>) -> Result<(), HostProcessError> {
            self.killed
                .lock()
                .unwrap()
                .push(signal.unwrap_or(ProcessSignal::Terminate));
            self.kill_notification.notify_waiters();
            Ok(())
        }
        fn dispose(&self) {
            self.disposed.store(true, Ordering::Release);
        }
    }

    #[derive(Clone, Debug, Eq, PartialEq)]
    struct Invocation {
        command: String,
        args: Vec<String>,
        options: HostProcessOptions,
    }

    struct StubProcessService {
        specs: Mutex<VecDeque<ProcessSpec>>,
        invocations: Mutex<Vec<Invocation>>,
        spawned: Mutex<Vec<Arc<StubProcess>>>,
    }

    impl StubProcessService {
        fn new(specs: impl IntoIterator<Item = ProcessSpec>) -> Self {
            Self {
                specs: Mutex::new(specs.into_iter().collect()),
                invocations: Mutex::new(Vec::new()),
                spawned: Mutex::new(Vec::new()),
            }
        }
    }

    #[async_trait]
    impl HostProcessService for StubProcessService {
        async fn spawn(
            &self,
            command: &str,
            args: &[String],
            options: HostProcessOptions,
        ) -> Result<Arc<dyn HostProcess>, HostProcessError> {
            self.invocations.lock().unwrap().push(Invocation {
                command: command.into(),
                args: args.to_vec(),
                options,
            });
            let spec = self
                .specs
                .lock()
                .unwrap()
                .pop_front()
                .expect("unexpected process spawn");
            let process = Arc::new(StubProcess::new(spec));
            self.spawned.lock().unwrap().push(Arc::clone(&process));
            Ok(process)
        }
    }

    fn service(
        specs: impl IntoIterator<Item = ProcessSpec>,
    ) -> (GitService, Arc<StubProcessService>) {
        let process = Arc::new(StubProcessService::new(specs));
        let git = GitService::new(
            Arc::clone(&process) as Arc<dyn HostProcessService>,
            Arc::new(HostFileSystem),
        );
        (git, process)
    }

    fn error_code<'a>(error: &'a (dyn std::error::Error + 'static)) -> Option<&'a str> {
        error
            .downcast_ref::<Error2>()
            .map(|error| error.code.as_str())
    }

    #[tokio::test]
    async fn status_preserves_command_order_stats_filter_and_pr_cache() {
        let first = [
            ProcessSpec::new(0, "true\n", ""),
            ProcessSpec::new(0, "## main\n M a.txt\n M b.txt\n", ""),
            ProcessSpec::new(0, "head\n", ""),
            ProcessSpec::new(0, "2\t1\ta.txt\n3\t0\tb.txt\n", ""),
            ProcessSpec::new(
                0,
                r#"{"number":7,"url":"https://github.com/a/b/pull/7","state":"OPEN"}"#,
                "",
            ),
            ProcessSpec::new(0, "true\n", ""),
            ProcessSpec::new(0, "## main\n", ""),
        ];
        let (git, process) = service(first);
        let filter = HashSet::from(["a.txt".to_owned()]);

        let first = git.status("/repo", Some(&filter)).await.unwrap();
        assert_eq!(
            first.entries,
            indexmap::IndexMap::from([("a.txt".into(), super::super::FsGitStatus::Modified)])
        );
        assert_eq!((first.additions, first.deletions), (5, 1));
        assert_eq!(first.pull_request.unwrap().number, 7);
        assert!(
            git.status("/repo", None)
                .await
                .unwrap()
                .pull_request
                .is_some()
        );

        let invocations = process.invocations.lock().unwrap();
        assert_eq!(invocations.len(), 7);
        assert_eq!(
            invocations[0].args,
            strings(&["rev-parse", "--is-inside-work-tree"])
        );
        assert_eq!(
            invocations[1].args,
            strings(&["status", "--porcelain=v1", "--branch"])
        );
        assert_eq!(
            invocations[2].args,
            strings(&["rev-parse", "--verify", "--quiet", "HEAD"])
        );
        assert_eq!(
            invocations[3].args,
            strings(&["diff", "--no-color", "--numstat", "HEAD", "--"])
        );
        assert_eq!(invocations[4].command, "gh");
        assert_eq!(
            invocations[4].options.env.as_ref().unwrap()["GH_PROMPT_DISABLED"],
            "1"
        );
    }

    #[tokio::test]
    async fn status_reports_git_unavailable_with_original_details() {
        let (git, _) = service([ProcessSpec::new(128, "", "fatal: not a repository\n")]);
        let error = git.status("/no-repo", None).await.unwrap_err();
        assert_eq!(error_code(error.as_ref()), Some(FS_GIT_UNAVAILABLE));
        let error = error.downcast_ref::<Error2>().unwrap();
        assert_eq!(error.details.as_ref().unwrap()["cwd"], "/no-repo");
        assert_eq!(
            error.message,
            "git unavailable at /no-repo: fatal: not a repository"
        );
    }

    #[tokio::test]
    async fn diff_accepts_no_index_exit_one_for_untracked_files() {
        let (git, process) = service([
            ProcessSpec::new(0, "true\n", ""),
            ProcessSpec::new(0, "?? new.txt\n", ""),
            ProcessSpec::new(0, "head\n", ""),
            ProcessSpec::new(1, "+brand new\n", ""),
        ]);

        let result = git.diff("/repo", "new.txt", "/repo/new.txt").await.unwrap();
        assert_eq!(result.diff, "+brand new\n");
        assert!(!result.truncated);
        assert_eq!(
            process.invocations.lock().unwrap()[3].args,
            strings(&[
                "diff",
                "--no-color",
                "--no-index",
                "--",
                "/dev/null",
                "new.txt"
            ])
        );
    }

    #[tokio::test]
    async fn diff_reports_missing_tracked_path() {
        let (git, _) = service([
            ProcessSpec::new(0, "true\n", ""),
            ProcessSpec::new(0, "", ""),
            ProcessSpec::new(0, "head\n", ""),
            ProcessSpec::new(0, "", ""),
        ]);
        let missing = std::env::temp_dir().join(format!("missing-{}", uuid::Uuid::new_v4()));

        let error = git
            .diff("/repo", "missing.txt", missing.to_str().unwrap())
            .await
            .unwrap_err();
        assert_eq!(error_code(error.as_ref()), Some(FS_PATH_NOT_FOUND));
    }

    #[tokio::test(start_paused = true)]
    async fn command_timeout_kills_drains_and_disposes_the_process() {
        let (git, process_service) = service([ProcessSpec::hanging()]);

        let result = git
            .run_command(
                "gh",
                &strings(&["pr", "view"]),
                "/repo",
                RunOptions {
                    timeout: Some(Duration::from_secs(5)),
                    env: None,
                },
            )
            .await
            .unwrap();

        assert_eq!(result, RunResult::failed());
        let process = Arc::clone(&process_service.spawned.lock().unwrap()[0]);
        assert_eq!(*process.killed.lock().unwrap(), vec![ProcessSignal::Kill]);
        assert!(process.disposed.load(Ordering::Acquire));
    }

    #[test]
    fn diff_truncation_counts_javascript_utf16_units() {
        assert_eq!(truncate_diff("a😀b".into(), 3), ("a😀".into(), true));
        assert_eq!(truncate_diff("a😀b".into(), 4), ("a😀b".into(), false));
    }
}
