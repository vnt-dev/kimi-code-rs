use std::{collections::HashMap, sync::Arc, time::Duration};

use futures_util::future::BoxFuture;
use tokio::{process::Command, sync::OnceCell};

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

pub type IsFile = Arc<dyn Fn(String) -> BoxFuture<'static, bool> + Send + Sync>;
pub type ExecFileText =
    Arc<dyn Fn(String, Vec<String>, Duration) -> BoxFuture<'static, Option<String>> + Send + Sync>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ShellName {
    Bash,
    Sh,
}

impl ShellName {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Bash => "bash",
            Self::Sh => "sh",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PathClass {
    Posix,
    Win32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HostEnvironmentInfo {
    pub os_kind: String,
    pub os_arch: String,
    pub os_version: String,
    pub shell_name: ShellName,
    pub shell_path: String,
    pub path_class: PathClass,
    pub home_dir: String,
}

pub struct HostEnvironmentProbeDeps {
    pub platform: String,
    pub arch: String,
    pub release: String,
    pub home_dir: String,
    pub env: HashMap<String, String>,
    pub is_file: IsFile,
    pub exec_file_text: ExecFileText,
}

#[derive(Clone, Debug, thiserror::Error)]
#[error("{message}")]
pub struct HostEnvironmentProbeError {
    message: String,
}

const GIT_EXEC_PATH_TIMEOUT: Duration = Duration::from_secs(5);
const MINGW_PREFIXES: &[&str] = &["mingw32", "mingw64", "ucrt64", "clang64", "clangarm64"];

// Original: packages/agent-core-v2/src/_base/execEnv/environmentProbe.ts.
pub async fn probe_host_environment(
    deps: &HostEnvironmentProbeDeps,
) -> Result<HostEnvironmentInfo, HostEnvironmentProbeError> {
    let os_kind = match deps.platform.as_str() {
        "darwin" => "macOS",
        "linux" => "Linux",
        "win32" => "Windows",
        other => other,
    }
    .to_owned();
    let path_class = if deps.platform == "win32" {
        PathClass::Win32
    } else {
        PathClass::Posix
    };
    let (shell_name, shell_path) = if deps.platform == "win32" {
        (ShellName::Bash, locate_windows_git_bash(deps).await?)
    } else {
        let mut found = None;
        for candidate in ["/bin/bash", "/usr/bin/bash", "/usr/local/bin/bash"] {
            if (deps.is_file)(candidate.to_owned()).await {
                found = Some(candidate.to_owned());
                break;
            }
        }
        found.map_or((ShellName::Sh, "/bin/sh".to_owned()), |path| {
            (ShellName::Bash, path)
        })
    };
    Ok(HostEnvironmentInfo {
        os_kind,
        os_arch: deps.arch.clone(),
        os_version: deps.release.clone(),
        shell_name,
        shell_path,
        path_class,
        home_dir: deps.home_dir.clone(),
    })
}

async fn locate_windows_git_bash(
    deps: &HostEnvironmentProbeDeps,
) -> Result<String, HostEnvironmentProbeError> {
    let mut checked = Vec::new();
    if let Some(override_path) = non_blank(environment_value(deps, "KIMI_SHELL_PATH")) {
        checked.push(override_path.to_owned());
        if (deps.is_file)(override_path.to_owned()).await {
            return Ok(override_path.to_owned());
        }
    }
    let git_executables = find_executables_on_path(
        "git.exe",
        environment_value(deps, "PATH").map(String::as_str),
        "win32",
        Arc::clone(&deps.is_file),
    )
    .await;
    for git_exe in git_executables {
        for candidate in git_bash_candidates_from_git_exe(&git_exe) {
            checked.push(candidate.clone());
            if (deps.is_file)(candidate.clone()).await {
                return Ok(candidate);
            }
        }
        let output = (deps.exec_file_text)(
            git_exe.clone(),
            vec!["--exec-path".into()],
            GIT_EXEC_PATH_TIMEOUT,
        )
        .await;
        let Some(exec_path) = output
            .as_deref()
            .and_then(|output| output.lines().map(str::trim).find(|line| !line.is_empty()))
        else {
            continue;
        };
        for candidate in git_bash_candidates_from_git_exec_path(exec_path) {
            checked.push(candidate.clone());
            if (deps.is_file)(candidate.clone()).await {
                return Ok(candidate);
            }
        }
    }
    let mut candidates = vec![
        r"C:\Program Files\Git\bin\bash.exe".to_owned(),
        r"C:\Program Files\Git\usr\bin\bash.exe".to_owned(),
        r"C:\Program Files (x86)\Git\bin\bash.exe".to_owned(),
        r"C:\Program Files (x86)\Git\usr\bin\bash.exe".to_owned(),
    ];
    if let Some(local) = non_blank(environment_value(deps, "LOCALAPPDATA")) {
        candidates.push(format!(r"{local}\Programs\Git\bin\bash.exe"));
        candidates.push(format!(r"{local}\Programs\Git\usr\bin\bash.exe"));
    }
    for candidate in candidates {
        checked.push(candidate.clone());
        if (deps.is_file)(candidate.clone()).await {
            return Ok(candidate);
        }
    }
    Err(HostEnvironmentProbeError {
        message: format!(
            "Git Bash was not found on this Windows host. Install Git for Windows from https://gitforwindows.org/ or set KIMI_SHELL_PATH to a bash.exe. Checked: {}.",
            checked.join(", ")
        ),
    })
}

fn non_blank(value: Option<&String>) -> Option<&str> {
    value
        .map(String::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn environment_value<'a>(deps: &'a HostEnvironmentProbeDeps, name: &str) -> Option<&'a String> {
    deps.env.get(name).or_else(|| {
        (deps.platform == "win32")
            .then(|| {
                deps.env
                    .iter()
                    .find(|(key, _)| key.eq_ignore_ascii_case(name))
                    .map(|(_, value)| value)
            })
            .flatten()
    })
}

fn git_bash_candidates_from_git_exe(git_exe: &str) -> Vec<String> {
    let normalized = normalize_windows_path(git_exe);
    let mut parts = normalized.split('\\').collect::<Vec<_>>();
    let Some(_file_name) = parts.pop() else {
        return Vec::new();
    };
    let Some(directory) = parts.pop() else {
        return Vec::new();
    };
    if !directory.eq_ignore_ascii_case("cmd") && !directory.eq_ignore_ascii_case("bin") {
        return Vec::new();
    }
    git_bash_candidates_from_root(&parts.join("\\"))
}

fn git_bash_candidates_from_git_exec_path(exec_path: &str) -> Vec<String> {
    let normalized = normalize_windows_path(exec_path);
    let parts = normalized.split('\\').collect::<Vec<_>>();
    if let Some(index) = parts
        .iter()
        .rposition(|part| MINGW_PREFIXES.contains(&part.to_ascii_lowercase().as_str()))
    {
        let root = parts[..index].join("\\");
        if !root.is_empty() {
            return git_bash_candidates_from_root(&root);
        }
    }
    let root = if parts.len() >= 2 {
        parts[..parts.len() - 2].join("\\")
    } else {
        normalized
    };
    git_bash_candidates_from_root(&root)
}

fn git_bash_candidates_from_root(root: &str) -> Vec<String> {
    vec![
        format!(r"{root}\bin\bash.exe"),
        format!(r"{root}\usr\bin\bash.exe"),
    ]
}

fn normalize_windows_path(path: &str) -> String {
    let replaced = path.replace('/', "\\");
    let mut parts = Vec::new();
    for part in replaced.split('\\') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            part => parts.push(part),
        }
    }
    parts.join("\\")
}

async fn find_executables_on_path(
    name: &str,
    path: Option<&str>,
    platform: &str,
    is_file: IsFile,
) -> Vec<String> {
    let Some(path) = path.filter(|path| !path.is_empty()) else {
        return Vec::new();
    };
    let (list_separator, directory_separator) = if platform == "win32" {
        (';', '\\')
    } else {
        (':', '/')
    };
    let mut paths = Vec::new();
    for directory in path
        .split(list_separator)
        .map(str::trim)
        .filter(|p| !p.is_empty())
    {
        if platform == "win32" && !is_absolute_windows_path(directory) {
            continue;
        }
        let candidate = format!(
            "{}{}{}",
            directory.trim_end_matches(directory_separator),
            directory_separator,
            name
        );
        if is_file(candidate.clone()).await
            && !paths
                .iter()
                .any(|path: &String| path.eq_ignore_ascii_case(&candidate))
        {
            paths.push(candidate);
        }
    }
    paths
}

fn is_absolute_windows_path(path: &str) -> bool {
    let bytes = path.as_bytes();
    (bytes.len() >= 3 && bytes[1] == b':' && matches!(bytes[2], b'\\' | b'/'))
        || path.starts_with("\\\\")
}

pub async fn exec_file_text(
    file: impl AsRef<std::ffi::OsStr>,
    args: &[String],
    timeout: Duration,
) -> Option<String> {
    let mut command = Command::new(file);
    command.args(args);
    #[cfg(windows)]
    command.creation_flags(CREATE_NO_WINDOW);
    let output = tokio::time::timeout(timeout, command.output())
        .await
        .ok()?
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

pub async fn probe_host_environment_from_node()
-> Result<HostEnvironmentInfo, HostEnvironmentProbeError> {
    static CACHED: OnceCell<Result<HostEnvironmentInfo, HostEnvironmentProbeError>> =
        OnceCell::const_new();
    CACHED
        .get_or_init(|| async {
            let platform = match std::env::consts::OS {
                "macos" => "darwin",
                "windows" => "win32",
                other => other,
            };
            let env = std::env::vars().collect::<HashMap<_, _>>();
            let release = if platform == "win32" {
                exec_file_text("cmd", &["/C".into(), "ver".into()], Duration::from_secs(5)).await
            } else {
                exec_file_text("uname", &["-r".into()], Duration::from_secs(5)).await
            }
            .unwrap_or_default()
            .trim()
            .to_owned();
            let is_file: IsFile =
                Arc::new(|path| Box::pin(async move { tokio::fs::metadata(path).await.is_ok() }));
            let exec: ExecFileText = Arc::new(|file, args, timeout| {
                Box::pin(async move { exec_file_text(file, &args, timeout).await })
            });
            probe_host_environment(&HostEnvironmentProbeDeps {
                platform: platform.into(),
                arch: std::env::consts::ARCH.into(),
                release,
                home_dir: env
                    .get("HOME")
                    .or_else(|| env.get("USERPROFILE"))
                    .or_else(|| {
                        (platform == "win32")
                            .then(|| {
                                env.iter()
                                    .find(|(key, _)| key.eq_ignore_ascii_case("USERPROFILE"))
                                    .map(|(_, value)| value)
                            })
                            .flatten()
                    })
                    .cloned()
                    .unwrap_or_default(),
                env,
                is_file,
                exec_file_text: exec,
            })
            .await
        })
        .await
        .clone()
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    #[tokio::test]
    async fn resolves_msys_native_git_through_exec_path() {
        for prefix in ["ucrt64", "clang64", "clangarm64"] {
            let git = format!(r"C:\msys64\{prefix}\bin\git.exe");
            let existing = Arc::new(HashSet::from([
                git.clone(),
                r"C:\msys64\usr\bin\bash.exe".to_owned(),
            ]));
            let is_file: IsFile = Arc::new(move |path| {
                let existing = Arc::clone(&existing);
                Box::pin(async move { existing.contains(&path) })
            });
            let output = format!("C:/msys64/{prefix}/libexec/git-core\n");
            let exec: ExecFileText = Arc::new(move |_, _, _| {
                let output = output.clone();
                Box::pin(async move { Some(output) })
            });
            let info = probe_host_environment(&HostEnvironmentProbeDeps {
                platform: "win32".into(),
                arch: "x86_64".into(),
                release: "1.2.3".into(),
                home_dir: r"C:\Users\me".into(),
                env: HashMap::from([("PATH".into(), format!(r"C:\msys64\{prefix}\bin"))]),
                is_file,
                exec_file_text: exec,
            })
            .await
            .unwrap();
            assert_eq!(info.shell_path, r"C:\msys64\usr\bin\bash.exe");
            assert_eq!(info.shell_name, ShellName::Bash);
        }
    }

    #[tokio::test]
    async fn windows_environment_keys_are_case_insensitive() {
        let git = r"D:\Program Files\Git\cmd\git.exe".to_owned();
        let bash = r"D:\Program Files\Git\bin\bash.exe".to_owned();
        let existing = Arc::new(HashSet::from([git, bash.clone()]));
        let is_file: IsFile = Arc::new(move |path| {
            let existing = Arc::clone(&existing);
            Box::pin(async move { existing.contains(&path) })
        });
        let info = probe_host_environment(&HostEnvironmentProbeDeps {
            platform: "win32".into(),
            arch: "x86_64".into(),
            release: "1.2.3".into(),
            home_dir: r"C:\Users\me".into(),
            env: HashMap::from([("Path".into(), r"D:\Program Files\Git\cmd".into())]),
            is_file,
            exec_file_text: Arc::new(|_, _, _| Box::pin(async { None })),
        })
        .await
        .unwrap();
        assert_eq!(info.shell_path, bash);

        let override_path = r"D:\portable\bash.exe".to_owned();
        let override_file = override_path.clone();
        let overridden = probe_host_environment(&HostEnvironmentProbeDeps {
            platform: "win32".into(),
            arch: "x86_64".into(),
            release: "1.2.3".into(),
            home_dir: r"C:\Users\me".into(),
            env: HashMap::from([("kimi_shell_path".into(), override_path.clone())]),
            is_file: Arc::new(move |path| {
                let override_file = override_file.clone();
                Box::pin(async move { path == override_file })
            }),
            exec_file_text: Arc::new(|_, _, _| Box::pin(async { None })),
        })
        .await
        .unwrap();
        assert_eq!(overridden.shell_path, override_path);
    }

    #[tokio::test]
    async fn posix_prefers_bash_then_falls_back_to_sh() {
        let deps = |exists: bool| HostEnvironmentProbeDeps {
            platform: "linux".into(),
            arch: "x64".into(),
            release: "6".into(),
            home_dir: "/home/u".into(),
            env: HashMap::new(),
            is_file: Arc::new(move |path| Box::pin(async move { exists && path == "/bin/bash" })),
            exec_file_text: Arc::new(|_, _, _| Box::pin(async { None })),
        };
        assert_eq!(
            probe_host_environment(&deps(true))
                .await
                .unwrap()
                .shell_name,
            ShellName::Bash
        );
        assert_eq!(
            probe_host_environment(&deps(false))
                .await
                .unwrap()
                .shell_name,
            ShellName::Sh
        );
    }
}
