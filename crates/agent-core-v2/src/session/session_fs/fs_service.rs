//! Session-scoped filesystem service.
//!
//! Original: `packages/agent-core-v2/src/session/sessionFs/fsService.ts`.

use std::{
    cmp::Ordering,
    collections::{HashMap, HashSet, VecDeque},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
use std::sync::{Arc, atomic::{AtomicBool, Ordering as AtomicOrdering}};
use parking_lot::Mutex;

use async_trait::async_trait;
use base64::{Engine, engine::general_purpose::STANDARD};
use chrono::{DateTime, SecondsFormat, Utc};
use indexmap::IndexMap;
use serde_json::{Map, Value};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use tokio::sync::Mutex as AsyncMutex;

use crate::{
    _base::{
        di::{
            descriptors::SyncDescriptor,
            instantiation::ServicesAccessorExt,
            scope::{InstantiationType, LifecycleScope, register_scoped_service},
        },
        errors::errors::{Error2, Error2Options},
        utils::abort::{AbortController, AbortSignal},
    },
    app::{
        git::{
            FS_GIT_SERVICE_ID, FsDiffRequest, FsDiffResponse, FsGitStatusRequest,
            FsGitStatusResponse, GitServiceHandle,
        },
        telemetry::{TELEMETRY_SERVICE_ID, TelemetryProperties, TelemetryServiceHandle},
    },
    os::interface::{
        host_file_system::{
            HOST_FILE_SYSTEM_SERVICE_ID, HostFileStat, HostFileSystemServiceHandle,
        },
        host_fs_errors::{HostFsError, OS_FS_ALREADY_EXISTS, OS_FS_NOT_DIRECTORY, OS_FS_NOT_FOUND},
        host_process::ProcessSignal,
    },
    session::{
        process::{
            ProcessExecOptions, SESSION_PROCESS_RUNNER_SERVICE_ID, SessionProcessRunnerHandle,
        },
        workspace_context::{SESSION_WORKSPACE_CONTEXT_ID, SessionWorkspaceContextHandle},
    },
};

use super::{
    FS_ALREADY_EXISTS, FS_GREP_TIMEOUT, FS_IS_BINARY, FS_IS_DIRECTORY, FS_PATH_ESCAPES,
    FS_PATH_NOT_FOUND, FS_TOO_LARGE, FsDownloadResolved, FsEntry, FsGrepFileHit, FsGrepMatch,
    FsGrepRequest, FsGrepResponse, FsKind, FsListManyPartialError, FsListManyRequest,
    FsListManyResponse, FsListRequest, FsListResponse, FsListSort, FsMkdirRequest, FsMkdirResponse,
    FsPathResolved, FsReadEncoding, FsReadRequest, FsReadRequestEncoding, FsReadResponse,
    FsSearchHit, FsSearchRequest, FsSearchResponse, FsStatManyRequest, FsStatManyResponse,
    FsStatRequest, FsStatResponse, GitignoreMatcher, RgJsonRecord, RgJsonRecordType, RgProbe,
    RgResolution, SESSION_FS_SERVICE_ID, SessionFsError, SessionFsResult, SessionFsServiceContract,
    SessionFsServiceHandle, compile_glob_set, compile_grep_pattern, compute_fuzzy_score,
    compute_match_positions, ensure_fs_errors_registered, ensure_rg_path, rg_path, rg_text,
    run_command, strip_trailing_newline,
};

const SEARCH_HARD_CAP: usize = 500;
const GREP_TIMEOUT_MS: u64 = 30_000;
const WALK_MAX_DEPTH: usize = 64;
const FS_READ_MAX_BYTES: u64 = 10 * 1024 * 1024;
const FS_BINARY_SAMPLE_BYTES: usize = 4_096;
const FS_BINARY_NONPRINTABLE_FRACTION: f64 = 0.3;

const WIRE_FS_PATH_NOT_FOUND: i64 = 40_409;
const WIRE_FS_IS_DIRECTORY: i64 = 40_906;
const WIRE_FS_IS_BINARY: i64 = 40_907;
const WIRE_FS_TOO_LARGE: i64 = 41_302;
const WIRE_FS_TOO_MANY_RESULTS: i64 = 41_303;
const WIRE_INTERNAL_ERROR: i64 = 50_001;

pub struct SessionFsService {
    workspace: SessionWorkspaceContextHandle,
    host_fs: HostFileSystemServiceHandle,
    runner: SessionProcessRunnerHandle,
    telemetry: TelemetryServiceHandle,
    git: GitServiceHandle,
    gitignore_cache: AsyncMutex<HashMap<String, GitignoreMatcher>>,
    rg_resolution: AsyncMutex<Option<Option<RgResolution>>>,
    real_roots_cache: AsyncMutex<Option<(String, Vec<PathBuf>)>>,
}

impl SessionFsService {
    pub fn new(
        workspace: SessionWorkspaceContextHandle,
        host_fs: HostFileSystemServiceHandle,
        runner: SessionProcessRunnerHandle,
        telemetry: TelemetryServiceHandle,
        git: GitServiceHandle,
    ) -> Self {
        ensure_fs_errors_registered();
        Self {
            workspace,
            host_fs,
            runner,
            telemetry,
            git,
            gitignore_cache: AsyncMutex::new(HashMap::new()),
            rg_resolution: AsyncMutex::new(None),
            real_roots_cache: AsyncMutex::new(None),
        }
    }

    fn absolute_of(&self, relative: &str) -> PathBuf {
        if relative.is_empty() || relative == "." {
            self.workspace.work_dir()
        } else {
            self.workspace.work_dir().join(relative)
        }
    }

    async fn matcher(&self) -> GitignoreMatcher {
        let cwd = self.workspace.work_dir().to_string_lossy().into_owned();
        let mut cache = self.gitignore_cache.lock().await;
        if let Some(matcher) = cache.get(&cwd) {
            return matcher.clone();
        }
        let mut matcher = GitignoreMatcher::new();
        matcher.add(".git/");
        if let Ok(contents) = self
            .host_fs
            .read_text(&self.workspace.work_dir().join(".gitignore"), None)
            .await
        {
            matcher.add(&contents);
        }
        cache.insert(cwd, matcher.clone());
        matcher
    }

    async fn walk_entries(&self, matcher: Option<&GitignoreMatcher>) -> Vec<WalkEntry> {
        let mut output = Vec::new();
        let mut stack = vec![(String::new(), 0_usize)];
        while let Some((root, depth)) = stack.pop() {
            if depth > WALK_MAX_DEPTH {
                continue;
            }
            let Ok(entries) = self.host_fs.read_dir(&self.absolute_of(&root)).await else {
                continue;
            };
            let mut directories = Vec::new();
            for entry in entries {
                if entry.name == ".git" {
                    continue;
                }
                let relative = if root.is_empty() {
                    entry.name.clone()
                } else {
                    format!("{root}/{}", entry.name)
                };
                let is_directory = entry.is_directory && !entry.is_symbolic_link;
                let probe = if is_directory {
                    format!("{relative}/")
                } else {
                    relative.clone()
                };
                if matcher.is_some_and(|matcher| matcher.ignores(&probe)) {
                    continue;
                }
                let kind = if entry.is_symbolic_link {
                    FsKind::Symlink
                } else if is_directory {
                    FsKind::Directory
                } else {
                    FsKind::File
                };
                output.push(WalkEntry {
                    relative: relative.clone(),
                    name: entry.name,
                    kind,
                });
                if is_directory {
                    directories.push(relative);
                }
            }
            for directory in directories.into_iter().rev() {
                stack.push((directory, depth + 1));
            }
        }
        output
    }

    async fn resolve_rg(&self) -> Option<RgResolution> {
        let mut cached = self.rg_resolution.lock().await;
        if let Some(value) = cached.as_ref() {
            return value.clone();
        }
        let probe = SessionRgProbe {
            runner: self.runner.clone(),
            cwd: self.workspace.work_dir().to_string_lossy().into_owned(),
        };
        let resolution = ensure_rg_path(&probe, Default::default()).await.ok();
        *cached = Some(resolution.clone());
        resolution
    }

    async fn grep_with_rg(
        &self,
        request: FsGrepRequest,
        signal: AbortSignal,
        started_at: Instant,
        rg_binary: &str,
    ) -> SessionFsResult<FsGrepResponse> {
        let mut args = vec![rg_binary.into(), "--json".into()];
        if request.context_lines > 0 {
            args.extend(["--context".into(), request.context_lines.to_string()]);
        }
        if !request.case_sensitive {
            args.push("--ignore-case".into());
        }
        if !request.regex {
            args.push("--fixed-strings".into());
        }
        if request.follow_gitignore {
            args.push("--no-require-git".into());
        } else {
            args.push("--no-ignore".into());
        }
        if let Some(globs) = request.include_globs.as_ref() {
            for glob in globs {
                args.extend(["--glob".into(), glob.clone()]);
            }
        }
        if let Some(globs) = request.exclude_globs.as_ref() {
            for glob in globs {
                args.extend(["--glob".into(), format!("!{glob}")]);
            }
        }
        args.extend([
            "--max-count".into(),
            request.max_matches_per_file.to_string(),
            request.pattern.clone(),
            ".".into(),
        ]);

        let process = self
            .runner
            .exec(
                &args,
                Some(ProcessExecOptions {
                    cwd: Some(self.workspace.work_dir().to_string_lossy().into_owned()),
                    env: None,
                }),
            )
            .await?;
        let killed = Arc::new(AtomicBool::new(false));
        let abort_process = process.clone();
        let abort_killed = Arc::clone(&killed);
        let abort_signal = signal.clone();
        let abort_task = tokio::spawn(async move {
            abort_signal.cancelled().await;
            if !abort_killed.swap(true, AtomicOrdering::AcqRel) {
                let _ = abort_process.kill(Some(ProcessSignal::Kill)).await;
            }
        });

        let stdout = process.stdout();
        let stdout_process = process.clone();
        let stdout_killed = Arc::clone(&killed);
        let accumulator = Arc::new(Mutex::new(RgJsonAccumulator::new(request.clone())));
        let stdout_accumulator = Arc::clone(&accumulator);
        let stdout_task = tokio::spawn(async move {
            let mut stdout = stdout.lock().await;
            let mut reader = BufReader::new(&mut **stdout);
            let mut line = String::new();
            loop {
                line.clear();
                let count = reader.read_line(&mut line).await?;
                if count == 0 {
                    break;
                }
                let record = line.trim_end_matches(['\r', '\n']);
                if !record.is_empty() {
                    let capped = {
                        let mut accumulator = stdout_accumulator.lock();
                        accumulator.feed(record);
                        accumulator.capped()
                    };
                    if capped && !stdout_killed.swap(true, AtomicOrdering::AcqRel) {
                        let _ = stdout_process.kill(Some(ProcessSignal::Kill)).await;
                    }
                }
            }
            Ok::<_, std::io::Error>(())
        });
        let stderr = process.stderr();
        let stderr_task = tokio::spawn(async move {
            let mut bytes = Vec::new();
            stderr.lock().await.read_to_end(&mut bytes).await?;
            Ok::<_, std::io::Error>(bytes)
        });
        let wait_process = process.clone();
        let wait_task = tokio::spawn(async move { wait_process.wait().await });

        let stdout_result = stdout_task.await;
        match stderr_task.await {
            Ok(Ok(_)) => {}
            Ok(Err(_error)) if killed.load(AtomicOrdering::Acquire) => {}
            Ok(Err(error)) => return Err(Box::new(error)),
            Err(_error) if killed.load(AtomicOrdering::Acquire) => {}
            Err(error) => return Err(Box::new(error)),
        }
        let _ = wait_task.await;
        abort_task.abort();
        process.dispose();

        match stdout_result {
            Ok(Ok(())) => {}
            Ok(Err(_error)) if killed.load(AtomicOrdering::Acquire) => {}
            Ok(Err(error)) => return Err(Box::new(error)),
            Err(_error) if killed.load(AtomicOrdering::Acquire) => {}
            Err(error) => return Err(Box::new(error)),
        }
        let accumulator = {
            let mut accumulator = accumulator.lock();
            std::mem::replace(&mut *accumulator, RgJsonAccumulator::new(request))
        };
        accumulator.finish(signal.aborted(), elapsed_millis(started_at))
    }

    async fn grep_with_node(
        &self,
        request: FsGrepRequest,
        signal: AbortSignal,
        started_at: Instant,
    ) -> SessionFsResult<FsGrepResponse> {
        let matcher = if request.follow_gitignore {
            Some(self.matcher().await)
        } else {
            None
        };
        let expression = compile_grep_pattern(&request)?;
        let include_globs = request
            .include_globs
            .as_deref()
            .map(compile_glob_set)
            .transpose()?;
        let exclude_globs = request
            .exclude_globs
            .as_deref()
            .map(compile_glob_set)
            .transpose()?;
        let entries = self.walk_entries(matcher.as_ref()).await;
        let files = entries
            .into_iter()
            .filter(|entry| entry.kind == FsKind::File)
            .filter(|entry| {
                include_globs
                    .as_ref()
                    .is_none_or(|globs| globs.is_match(&entry.relative))
            })
            .filter(|entry| {
                exclude_globs
                    .as_ref()
                    .is_none_or(|globs| !globs.is_match(&entry.relative))
            })
            .map(|entry| entry.relative)
            .collect::<Vec<_>>();

        let mut hits = Vec::new();
        let mut files_scanned = 0;
        let mut total_matches = 0;
        let mut truncated = false;
        for relative in files {
            if signal.aborted() {
                if total_matches == 0 && files_scanned == 0 {
                    return Err(grep_timeout_error(elapsed_millis(started_at)));
                }
                truncated = true;
                break;
            }
            if files_scanned >= request.max_files {
                truncated = true;
                break;
            }
            files_scanned += 1;
            let Ok(content) = self
                .host_fs
                .read_text(&self.absolute_of(&relative), None)
                .await
            else {
                continue;
            };
            let lines = split_lines_like_js(&content);
            let mut matches = Vec::new();
            for (index, line) in lines.iter().enumerate() {
                let utf16 = line.encode_utf16().collect::<Vec<_>>();
                let Some(found) = expression.find_from_ucs2(&utf16, 0).next() else {
                    continue;
                };
                if matches.len() >= request.max_matches_per_file {
                    break;
                }
                let before_start = index.saturating_sub(request.context_lines);
                let before = lines[before_start..index].to_vec();
                let after_end = (index + 1 + request.context_lines).min(lines.len());
                let after = lines[index + 1..after_end].to_vec();
                matches.push(FsGrepMatch {
                    line: index as u64 + 1,
                    col: found.range.start as u64 + 1,
                    text: line.clone(),
                    before,
                    after,
                });
                total_matches += 1;
                if total_matches >= request.max_total_matches {
                    truncated = true;
                    break;
                }
            }
            if !matches.is_empty() {
                hits.push(FsGrepFileHit {
                    path: relative,
                    matches,
                });
            }
            if total_matches >= request.max_total_matches {
                break;
            }
        }
        Ok(FsGrepResponse {
            files: hits,
            files_scanned,
            truncated,
            elapsed_ms: elapsed_millis(started_at),
        })
    }

    async fn real_roots(&self) -> Vec<PathBuf> {
        let directories = std::iter::once(self.workspace.work_dir())
            .chain(self.workspace.additional_dirs())
            .collect::<Vec<_>>();
        let key = directories
            .iter()
            .map(|path| path.to_string_lossy())
            .collect::<Vec<_>>()
            .join("\n");
        let mut cache = self.real_roots_cache.lock().await;
        if let Some((cached_key, roots)) = cache.as_ref()
            && cached_key == &key
        {
            return roots.clone();
        }
        let mut roots = Vec::new();
        for directory in directories {
            match self.host_fs.real_path(&directory).await {
                Ok(real) => roots.push(PathBuf::from(real)),
                Err(_) => roots.push(directory),
            }
        }
        *cache = Some((key, roots.clone()));
        roots
    }

    async fn realpath_existing_prefix(&self, absolute: &Path) -> SessionFsResult<PathBuf> {
        let mut tail = Vec::new();
        let mut current = absolute.to_path_buf();
        for _ in 0..256 {
            match self.host_fs.real_path(&current).await {
                Ok(real) => {
                    let mut resolved = PathBuf::from(real);
                    for component in tail.iter().rev() {
                        resolved.push(component);
                    }
                    return Ok(resolved);
                }
                Err(error) if is_missing_path_error(&error) => {
                    let Some(name) = current.file_name().map(|name| name.to_os_string()) else {
                        return Ok(absolute.to_path_buf());
                    };
                    let Some(parent) = current.parent() else {
                        return Ok(absolute.to_path_buf());
                    };
                    if parent == current {
                        return Ok(absolute.to_path_buf());
                    }
                    tail.push(name);
                    current = parent.to_path_buf();
                }
                Err(error) => return Err(Box::new(error)),
            }
        }
        Ok(absolute.to_path_buf())
    }

    async fn resolve_within(&self, input: &str) -> SessionFsResult<PathBuf> {
        if input.is_empty() || input == "/" {
            return Err(path_escape_error(input, "empty", "rejected (empty)"));
        }
        if Path::new(input).is_absolute() {
            return Err(path_escape_error(input, "absolute", "rejected (absolute)"));
        }
        if input.split(['/', '\\']).any(|segment| segment == "..") {
            return Err(path_escape_error(
                input,
                "dotdot_segment",
                "rejected (dotdot segment)",
            ));
        }
        let absolute = self.workspace.resolve(input);
        if !self.workspace.is_within(&absolute.to_string_lossy()) {
            return Err(path_escape_error(
                input,
                "resolved_outside",
                "escapes workspace",
            ));
        }
        let resolved = self.realpath_existing_prefix(&absolute).await?;
        if !self
            .real_roots()
            .await
            .iter()
            .any(|root| is_inside_or_equal(&resolved, root))
        {
            return Err(path_escape_error(
                input,
                "symlink_outside",
                "escapes workspace through a symlink",
            ));
        }
        Ok(absolute)
    }

    fn to_relative(&self, absolute: &Path) -> String {
        let cwd = self.workspace.work_dir();
        if absolute == cwd {
            return ".".into();
        }
        absolute
            .strip_prefix(&cwd)
            .map(|relative| relative.to_string_lossy().replace('\\', "/"))
            .unwrap_or_else(|_| absolute.to_string_lossy().replace('\\', "/"))
    }
}

#[async_trait]
impl SessionFsServiceContract for SessionFsService {
    async fn list(&self, request: FsListRequest) -> SessionFsResult<FsListResponse> {
        let absolute = self.resolve_within(&request.path).await?;
        let relative = self.to_relative(&absolute);
        let top_stat = self
            .host_fs
            .stat(&absolute)
            .await
            .map_err(|error| map_fs_error(error, &request.path))?;
        if !top_stat.is_directory {
            return Err(path_not_found_error(&request.path));
        }
        let matcher = if request.follow_gitignore {
            Some(self.matcher().await)
        } else {
            None
        };
        let exclude_globs = request
            .exclude_globs
            .as_deref()
            .map(compile_glob_set)
            .transpose()?;
        let mut items = Vec::new();
        let mut children_by_path = IndexMap::new();
        let mut truncated = false;
        let top_relative = if relative == "." {
            String::new()
        } else {
            relative
        };
        let mut queue = VecDeque::from([(top_relative.clone(), request.depth)]);
        while let Some((parent, depth_remaining)) = queue.pop_front() {
            let entries = match self.host_fs.read_dir(&self.absolute_of(&parent)).await {
                Ok(entries) => entries,
                Err(error) if parent == top_relative => {
                    return Err(map_fs_error(error, &request.path));
                }
                Err(_) => continue,
            };
            let mut visible = Vec::new();
            for entry in entries {
                if !request.show_hidden && is_hidden(&entry.name) {
                    continue;
                }
                let child_relative = if parent.is_empty() {
                    entry.name.clone()
                } else {
                    format!("{parent}/{}", entry.name)
                };
                if matcher.as_ref().is_some_and(|matcher| {
                    matcher.ignores(&child_relative)
                        || matcher.ignores(&format!("{child_relative}/"))
                }) {
                    continue;
                }
                if exclude_globs
                    .as_ref()
                    .is_some_and(|globs| globs.is_match(&child_relative))
                {
                    continue;
                }
                let Ok(stat) = self.host_fs.lstat(&self.absolute_of(&child_relative)).await else {
                    continue;
                };
                visible.push(ListChild {
                    name: entry.name,
                    relative: child_relative,
                    stat,
                });
            }
            sort_children(&mut visible, request.sort);
            let parent_key = if parent.is_empty() {
                ".".into()
            } else {
                parent.clone()
            };
            let mut bucket = Vec::new();
            for child in visible {
                if items.len() >= request.limit && depth_remaining == request.depth {
                    truncated = true;
                    break;
                }
                let entry = build_fs_entry(&child.relative, &child.name, &child.stat, false);
                if depth_remaining == request.depth {
                    items.push(entry.clone());
                }
                bucket.push(entry);
                if child.stat.is_directory && depth_remaining > 1 {
                    queue.push_back((child.relative, depth_remaining - 1));
                }
            }
            if depth_remaining < request.depth {
                children_by_path.insert(parent_key, bucket);
            }
        }
        Ok(FsListResponse {
            items,
            children_by_path: (!children_by_path.is_empty()).then_some(children_by_path),
            truncated,
        })
    }

    async fn read(&self, request: FsReadRequest) -> SessionFsResult<FsReadResponse> {
        let absolute = self.resolve_within(&request.path).await?;
        let relative = self.to_relative(&absolute);
        let stat = self
            .host_fs
            .stat(&absolute)
            .await
            .map_err(|error| map_fs_error(error, &request.path))?;
        if stat.is_directory {
            return Err(is_directory_error(&request.path));
        }
        if stat.size > FS_READ_MAX_BYTES {
            return Err(Box::new(Error2::with_options(
                FS_TOO_LARGE,
                format!(
                    "file too large: {} ({} bytes > {FS_READ_MAX_BYTES})",
                    request.path, stat.size
                ),
                Error2Options {
                    details: Some(Map::from_iter([
                        ("path".into(), Value::String(request.path.clone())),
                        ("size".into(), Value::from(stat.size)),
                    ])),
                    ..Error2Options::default()
                },
            )));
        }
        let sample_size = FS_BINARY_SAMPLE_BYTES.min(stat.size as usize);
        let sample = if sample_size == 0 {
            Vec::new()
        } else {
            self.host_fs
                .read_bytes(&absolute, Some(sample_size))
                .await?
        };
        let binary = detect_binary(&sample);
        if binary && request.encoding == FsReadRequestEncoding::Utf8 {
            return Err(Box::new(Error2::with_options(
                FS_IS_BINARY,
                format!("file is binary: {}", request.path),
                Error2Options {
                    details: Some(Map::from_iter([(
                        "path".into(),
                        Value::String(request.path),
                    )])),
                    ..Error2Options::default()
                },
            )));
        }
        let effective_length = request
            .length
            .min(stat.size.saturating_sub(request.offset as u64) as usize);
        let bytes = if effective_length == 0 {
            Vec::new()
        } else {
            let window = self
                .host_fs
                .read_bytes(&absolute, Some(request.offset + effective_length))
                .await?;
            window
                .get(request.offset..request.offset + effective_length)
                .unwrap_or_default()
                .to_vec()
        };
        let encoding = if request.encoding == FsReadRequestEncoding::Base64
            || (request.encoding == FsReadRequestEncoding::Auto && binary)
        {
            FsReadEncoding::Base64
        } else {
            FsReadEncoding::Utf8
        };
        let content = match encoding {
            FsReadEncoding::Utf8 => String::from_utf8_lossy(&bytes).into_owned(),
            FsReadEncoding::Base64 => STANDARD.encode(&bytes),
        };
        let language_id = (encoding == FsReadEncoding::Utf8)
            .then(|| guess_language_id(&relative))
            .flatten()
            .map(str::to_owned);
        let line_count = (encoding == FsReadEncoding::Utf8).then(|| count_lines(&content) as u64);
        Ok(FsReadResponse {
            path: relative.clone(),
            content,
            encoding,
            size: stat.size,
            truncated: request.offset as u64 + (effective_length as u64) < stat.size,
            etag: build_etag(&stat),
            mime: guess_mime(&relative, binary).into(),
            language_id,
            line_count,
            is_binary: binary,
        })
    }

    async fn list_many(&self, request: FsListManyRequest) -> SessionFsResult<FsListManyResponse> {
        let mut results = IndexMap::new();
        let mut partial_errors = IndexMap::new();
        let mut truncated_paths = Vec::new();
        for path in &request.paths {
            match self
                .list(FsListRequest {
                    path: path.clone(),
                    depth: request.depth,
                    limit: request.limit,
                    show_hidden: request.show_hidden,
                    follow_gitignore: request.follow_gitignore,
                    exclude_globs: request.exclude_globs.clone(),
                    sort: request.sort,
                    include_git_status: request.include_git_status,
                })
                .await
            {
                Ok(response) => {
                    results.insert(path.clone(), response.items);
                    if response.truncated {
                        truncated_paths.push(path.clone());
                    }
                }
                Err(error) if error_code(error.as_ref()) == Some(FS_PATH_ESCAPES) => {
                    return Err(error);
                }
                Err(error) => {
                    partial_errors.insert(path.clone(), to_wire_error(error.as_ref()));
                }
            }
        }
        Ok(FsListManyResponse {
            results,
            truncated_paths: (!truncated_paths.is_empty()).then_some(truncated_paths),
            partial_errors: (!partial_errors.is_empty()).then_some(partial_errors),
        })
    }

    async fn stat(&self, request: FsStatRequest) -> SessionFsResult<FsStatResponse> {
        let absolute = self.resolve_within(&request.path).await?;
        let relative = self.to_relative(&absolute);
        let stat = self
            .host_fs
            .lstat(&absolute)
            .await
            .map_err(|error| map_fs_error(error, &request.path))?;
        let name = if relative == "." {
            self.workspace
                .work_dir()
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned()
        } else {
            absolute
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned()
        };
        Ok(build_fs_entry(&relative, &name, &stat, true))
    }

    async fn stat_many(&self, request: FsStatManyRequest) -> SessionFsResult<FsStatManyResponse> {
        let mut resolved = Vec::new();
        for path in &request.paths {
            let absolute = self.resolve_within(path).await?;
            resolved.push((path.clone(), self.to_relative(&absolute), absolute));
        }
        let mut entries = IndexMap::new();
        for (raw, relative, absolute) in resolved {
            let entry = match self.host_fs.lstat(&absolute).await {
                Ok(stat) => {
                    let name = if relative == "." {
                        self.workspace
                            .work_dir()
                            .file_name()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .into_owned()
                    } else {
                        absolute
                            .file_name()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .into_owned()
                    };
                    Some(build_fs_entry(&relative, &name, &stat, false))
                }
                Err(_) => None,
            };
            entries.insert(raw, entry);
        }
        Ok(FsStatManyResponse { entries })
    }

    async fn mkdir(&self, request: FsMkdirRequest) -> SessionFsResult<FsMkdirResponse> {
        let absolute = self.resolve_within(&request.path).await?;
        let relative = self.to_relative(&absolute);
        if let Err(error) = self.host_fs.create_dir(&absolute, request.recursive).await {
            if error.code() == OS_FS_ALREADY_EXISTS {
                return Err(Box::new(Error2::with_options(
                    FS_ALREADY_EXISTS,
                    format!("path already exists: {}", request.path),
                    Error2Options {
                        details: Some(Map::from_iter([(
                            "path".into(),
                            Value::String(request.path),
                        )])),
                        ..Error2Options::default()
                    },
                )));
            }
            if matches!(error.code(), OS_FS_NOT_FOUND | OS_FS_NOT_DIRECTORY) {
                return Err(Box::new(Error2::with_options(
                    FS_PATH_NOT_FOUND,
                    format!("parent not found: {}", request.path),
                    Error2Options {
                        details: Some(Map::from_iter([(
                            "path".into(),
                            Value::String(request.path),
                        )])),
                        ..Error2Options::default()
                    },
                )));
            }
            return Err(Box::new(error));
        }
        let stat = self.host_fs.lstat(&absolute).await?;
        let name = absolute.file_name().unwrap_or_default().to_string_lossy();
        Ok(build_fs_entry(&relative, &name, &stat, false))
    }

    async fn search(&self, request: FsSearchRequest) -> SessionFsResult<FsSearchResponse> {
        let matcher = if request.follow_gitignore {
            Some(self.matcher().await)
        } else {
            None
        };
        let include_globs = request
            .include_globs
            .as_deref()
            .map(compile_glob_set)
            .transpose()?;
        let exclude_globs = request
            .exclude_globs
            .as_deref()
            .map(compile_glob_set)
            .transpose()?;
        let query_lower = request.query.to_lowercase();
        let mut candidates = self
            .walk_entries(matcher.as_ref())
            .await
            .into_iter()
            .filter_map(|entry| {
                let score = compute_fuzzy_score(&entry.name, &query_lower);
                if score <= 0.0
                    || include_globs
                        .as_ref()
                        .is_some_and(|globs| !globs.is_match(&entry.relative))
                    || exclude_globs
                        .as_ref()
                        .is_some_and(|globs| globs.is_match(&entry.relative))
                {
                    return None;
                }
                Some(FsSearchHit {
                    match_positions: compute_match_positions(&entry.relative, &query_lower),
                    path: entry.relative,
                    name: entry.name,
                    kind: entry.kind,
                    score,
                })
            })
            .collect::<Vec<_>>();
        candidates.sort_by(|left, right| {
            right
                .score
                .partial_cmp(&left.score)
                .unwrap_or(Ordering::Equal)
                .then_with(|| left.path.encode_utf16().cmp(right.path.encode_utf16()))
        });
        let cap = request.limit.min(SEARCH_HARD_CAP);
        let truncated = candidates.len() > cap;
        candidates.truncate(cap);
        Ok(FsSearchResponse {
            items: candidates,
            truncated,
        })
    }

    async fn grep(&self, request: FsGrepRequest) -> SessionFsResult<FsGrepResponse> {
        let started_at = Instant::now();
        let controller = AbortController::new();
        let timeout_controller = controller.clone();
        let timeout = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(GREP_TIMEOUT_MS)).await;
            timeout_controller.abort(None);
        });
        let signal = controller.signal();
        let result = if let Some(resolution) = self.resolve_rg().await {
            self.grep_with_rg(request, signal, started_at, &resolution.path)
                .await
        } else {
            self.telemetry.track(
                "fs_grep_node_fallback",
                Some(&TelemetryProperties::from([(
                    "reason".into(),
                    Some(Value::String("rg_missing".into())),
                )])),
            );
            self.grep_with_node(request, signal, started_at).await
        };
        timeout.abort();
        result
    }

    async fn git_status(
        &self,
        request: FsGitStatusRequest,
    ) -> SessionFsResult<FsGitStatusResponse> {
        let mut filter = None;
        if let Some(paths) = request.paths
            && !paths.is_empty()
        {
            let mut confined = HashSet::new();
            for path in paths {
                let absolute = self.resolve_within(&path).await?;
                confined.insert(self.to_relative(&absolute));
            }
            filter = Some(confined);
        }
        Ok(self
            .git
            .status(
                &self.workspace.work_dir().to_string_lossy(),
                filter.as_ref(),
            )
            .await?)
    }

    async fn diff(&self, request: FsDiffRequest) -> SessionFsResult<FsDiffResponse> {
        let absolute = self.resolve_within(&request.path).await?;
        Ok(self
            .git
            .diff(
                &self.workspace.work_dir().to_string_lossy(),
                &self.to_relative(&absolute),
                &absolute.to_string_lossy(),
            )
            .await?)
    }

    async fn resolve_path(&self, relative_path: &str) -> SessionFsResult<FsPathResolved> {
        let absolute = self.resolve_within(relative_path).await?;
        let relative = self.to_relative(&absolute);
        let stat = self
            .host_fs
            .lstat(&absolute)
            .await
            .map_err(|error| map_fs_error(error, relative_path))?;
        Ok(FsPathResolved {
            absolute: absolute.to_string_lossy().into_owned(),
            relative,
            is_directory: stat.is_directory,
        })
    }

    async fn resolve_download(&self, relative_path: &str) -> SessionFsResult<FsDownloadResolved> {
        let absolute = self.resolve_within(relative_path).await?;
        let relative = self.to_relative(&absolute);
        let stat = self
            .host_fs
            .stat(&absolute)
            .await
            .map_err(|error| map_fs_error(error, relative_path))?;
        if stat.is_directory {
            return Err(is_directory_error(relative_path));
        }
        let sample_size = FS_BINARY_SAMPLE_BYTES.min(stat.size as usize);
        let sample = if sample_size == 0 {
            Vec::new()
        } else {
            self.host_fs
                .read_bytes(&absolute, Some(sample_size))
                .await?
        };
        let binary = detect_binary(&sample);
        Ok(FsDownloadResolved {
            absolute: absolute.to_string_lossy().into_owned(),
            relative: relative.clone(),
            size: stat.size,
            etag: build_etag(&stat),
            mime: guess_mime(&relative, binary).into(),
            modified_at: date_from_millis(stat.modified_millis.unwrap_or(0)),
        })
    }
}

struct SessionRgProbe {
    runner: SessionProcessRunnerHandle,
    cwd: String,
}

#[async_trait]
impl RgProbe for SessionRgProbe {
    async fn exec(&self, args: &[String]) -> Result<i32, Box<dyn std::error::Error + Send + Sync>> {
        Ok(run_command(
            self.runner.0.as_ref(),
            args,
            super::RunCommandOptions {
                cwd: Some(self.cwd.clone()),
                ..Default::default()
            },
        )
        .await?
        .exit_code)
    }
}

struct WalkEntry {
    relative: String,
    name: String,
    kind: FsKind,
}

struct ListChild {
    name: String,
    relative: String,
    stat: HostFileStat,
}

struct RgFileBuffer {
    matches: Vec<FsGrepMatch>,
    pending: Vec<String>,
}

struct RgJsonAccumulator {
    request: FsGrepRequest,
    file_buffers: IndexMap<String, RgFileBuffer>,
    files: Vec<FsGrepFileHit>,
    total_matches: usize,
    files_scanned: usize,
    truncated: bool,
}

impl RgJsonAccumulator {
    fn new(request: FsGrepRequest) -> Self {
        Self {
            request,
            file_buffers: IndexMap::new(),
            files: Vec::new(),
            total_matches: 0,
            files_scanned: 0,
            truncated: false,
        }
    }

    fn capped(&self) -> bool {
        self.total_matches >= self.request.max_total_matches
            || self.files_scanned >= self.request.max_files
    }

    fn feed(&mut self, line: &str) {
        let Ok(record) = serde_json::from_str::<RgJsonRecord>(line) else {
            return;
        };
        let data = record.data.as_ref();
        match record.record_type {
            RgJsonRecordType::Begin => {
                let Some(path) = rg_path(data.and_then(|data| data.path.as_ref())) else {
                    return;
                };
                if self.files_scanned >= self.request.max_files {
                    self.truncated = true;
                    return;
                }
                self.file_buffers.insert(
                    path,
                    RgFileBuffer {
                        matches: Vec::new(),
                        pending: Vec::new(),
                    },
                );
                self.files_scanned += 1;
            }
            RgJsonRecordType::Context => {
                let Some(path) = rg_path(data.and_then(|data| data.path.as_ref())) else {
                    return;
                };
                let Some(buffer) = self.file_buffers.get_mut(&path) else {
                    return;
                };
                buffer.pending.push(
                    strip_trailing_newline(&rg_text(data.and_then(|data| data.lines.as_ref())))
                        .to_owned(),
                );
                let maximum = self.request.context_lines * 2;
                while buffer.pending.len() > maximum {
                    buffer.pending.remove(0);
                }
            }
            RgJsonRecordType::Match => {
                let Some(path) = rg_path(data.and_then(|data| data.path.as_ref())) else {
                    return;
                };
                let Some(buffer) = self.file_buffers.get_mut(&path) else {
                    return;
                };
                if self.total_matches >= self.request.max_total_matches {
                    self.truncated = true;
                    return;
                }
                if buffer.matches.len() >= self.request.max_matches_per_file {
                    return;
                }
                let text =
                    strip_trailing_newline(&rg_text(data.and_then(|data| data.lines.as_ref())))
                        .to_owned();
                let line = data.and_then(|data| data.line_number).unwrap_or(0);
                let col = data
                    .and_then(|data| data.submatches.as_ref())
                    .and_then(|matches| matches.first())
                    .map_or(1, |matched| matched.start as u64 + 1);
                let before_start = buffer
                    .pending
                    .len()
                    .saturating_sub(self.request.context_lines);
                let before = buffer.pending[before_start..].to_vec();
                buffer.pending.clear();
                buffer.matches.push(FsGrepMatch {
                    line,
                    col,
                    text,
                    before,
                    after: Vec::new(),
                });
                self.total_matches += 1;
                if self.total_matches >= self.request.max_total_matches {
                    self.truncated = true;
                }
            }
            RgJsonRecordType::End => {
                let Some(path) = rg_path(data.and_then(|data| data.path.as_ref())) else {
                    return;
                };
                self.finalize(&path);
            }
            RgJsonRecordType::Summary => {}
        }
    }

    fn finish(mut self, aborted: bool, elapsed_ms: u64) -> SessionFsResult<FsGrepResponse> {
        let paths = self.file_buffers.keys().cloned().collect::<Vec<_>>();
        for path in paths {
            self.finalize(&path);
        }
        let mut truncated = self.truncated;
        if aborted {
            if self.total_matches == 0 && self.files_scanned == 0 {
                return Err(grep_timeout_error(elapsed_ms));
            }
            truncated = true;
        }
        Ok(FsGrepResponse {
            files: self.files,
            files_scanned: self.files_scanned,
            truncated,
            elapsed_ms,
        })
    }

    fn finalize(&mut self, path: &str) {
        let Some(mut buffer) = self.file_buffers.shift_remove(path) else {
            return;
        };
        if !buffer.matches.is_empty() && !buffer.pending.is_empty() {
            let after = buffer
                .pending
                .into_iter()
                .take(self.request.context_lines)
                .collect();
            buffer.matches.last_mut().unwrap().after = after;
        }
        if !buffer.matches.is_empty() {
            self.files.push(FsGrepFileHit {
                path: path.into(),
                matches: buffer.matches,
            });
        }
    }
}

fn split_lines_like_js(content: &str) -> Vec<String> {
    content
        .split('\n')
        .map(|line| line.strip_suffix('\r').unwrap_or(line).to_owned())
        .collect()
}

fn elapsed_millis(started_at: Instant) -> u64 {
    started_at
        .elapsed()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

fn is_hidden(name: &str) -> bool {
    name.starts_with('.') || matches!(name, ".DS_Store" | ".AppleDouble" | ".LSOverride")
}

fn sort_children(children: &mut [ListChild], sort: FsListSort) {
    children.sort_by(|left, right| match sort {
        FsListSort::TypeFirst => left
            .stat
            .is_directory
            .cmp(&right.stat.is_directory)
            .reverse()
            .then_with(|| left.name.encode_utf16().cmp(right.name.encode_utf16())),
        FsListSort::NameAsc | FsListSort::MtimeDesc | FsListSort::SizeDesc => {
            left.name.encode_utf16().cmp(right.name.encode_utf16())
        }
        FsListSort::NameDesc => right.name.encode_utf16().cmp(left.name.encode_utf16()),
    });
}

fn build_etag(stat: &HostFileStat) -> String {
    format!(
        "{}-{}-{}",
        radix36(stat.modified_millis.unwrap_or(0).max(0) as u64),
        radix36(stat.size),
        radix36(stat.inode.unwrap_or(0))
    )
}

fn radix36(mut value: u64) -> String {
    if value == 0 {
        return "0".into();
    }
    let mut output = Vec::new();
    while value > 0 {
        let digit = (value % 36) as u8;
        output.push(if digit < 10 {
            b'0' + digit
        } else {
            b'a' + digit - 10
        });
        value /= 36;
    }
    output.reverse();
    String::from_utf8(output).unwrap()
}

fn build_fs_entry(relative: &str, name: &str, stat: &HostFileStat, with_mime: bool) -> FsEntry {
    let kind = if stat.is_symbolic_link {
        FsKind::Symlink
    } else if stat.is_directory {
        FsKind::Directory
    } else {
        FsKind::File
    };
    FsEntry {
        path: relative.into(),
        name: name.into(),
        kind,
        size: (kind == FsKind::File).then_some(stat.size),
        modified_at: date_from_millis(stat.modified_millis.unwrap_or(0))
            .to_rfc3339_opts(SecondsFormat::Millis, true),
        etag: Some(build_etag(stat)),
        mime: (with_mime && kind == FsKind::File).then(|| guess_mime(relative, false).to_owned()),
        language_id: (with_mime && kind == FsKind::File)
            .then(|| guess_language_id(relative))
            .flatten()
            .map(str::to_owned),
        is_binary: None,
        is_symlink_to: None,
        git_status: None,
        child_count: None,
    }
}

fn date_from_millis(millis: i64) -> DateTime<Utc> {
    DateTime::from_timestamp_millis(millis).unwrap_or(DateTime::UNIX_EPOCH)
}

fn detect_binary(bytes: &[u8]) -> bool {
    if bytes.is_empty() {
        return false;
    }
    let mut non_printable = 0;
    for byte in bytes {
        if *byte == 0 {
            return true;
        }
        if matches!(*byte, 9 | 10 | 13) || (32..=126).contains(byte) {
            continue;
        }
        non_printable += 1;
    }
    non_printable as f64 / bytes.len() as f64 > FS_BINARY_NONPRINTABLE_FRACTION
}

fn count_lines(text: &str) -> usize {
    if text.is_empty() {
        return 0;
    }
    let mut count = 1 + text.bytes().filter(|byte| *byte == b'\n').count();
    if text.ends_with('\n') {
        count -= 1;
    }
    count
}

fn is_inside_or_equal(child: &Path, parent: &Path) -> bool {
    child == parent
        || child
            .strip_prefix(parent)
            .is_ok_and(|relative| !relative.as_os_str().is_empty())
}

fn is_missing_path_error(error: &HostFsError) -> bool {
    matches!(error.code(), OS_FS_NOT_FOUND | OS_FS_NOT_DIRECTORY)
}

fn map_fs_error(error: HostFsError, input_path: &str) -> SessionFsError {
    if is_missing_path_error(&error) {
        path_not_found_error(input_path)
    } else {
        Box::new(error)
    }
}

fn path_not_found_error(path: &str) -> SessionFsError {
    Box::new(Error2::with_options(
        FS_PATH_NOT_FOUND,
        format!("path not found: {path}"),
        Error2Options {
            details: Some(Map::from_iter([(
                "path".into(),
                Value::String(path.into()),
            )])),
            ..Error2Options::default()
        },
    ))
}

fn is_directory_error(path: &str) -> SessionFsError {
    Box::new(Error2::with_options(
        FS_IS_DIRECTORY,
        format!("path is a directory: {path}"),
        Error2Options {
            details: Some(Map::from_iter([(
                "path".into(),
                Value::String(path.into()),
            )])),
            ..Error2Options::default()
        },
    ))
}

fn path_escape_error(path: &str, reason: &str, suffix: &str) -> SessionFsError {
    Box::new(Error2::with_options(
        FS_PATH_ESCAPES,
        format!("path \"{path}\" {suffix}"),
        Error2Options {
            details: Some(Map::from_iter([
                ("path".into(), Value::String(path.into())),
                ("reason".into(), Value::String(reason.into())),
            ])),
            ..Error2Options::default()
        },
    ))
}

fn grep_timeout_error(elapsed_ms: u64) -> SessionFsError {
    Box::new(Error2::new(
        FS_GREP_TIMEOUT,
        format!("grep timed out after {elapsed_ms}ms"),
    ))
}

fn error_code<'a>(error: &'a (dyn std::error::Error + 'static)) -> Option<&'a str> {
    error
        .downcast_ref::<Error2>()
        .map(|error| error.code.as_str())
}

fn to_wire_error(error: &(dyn std::error::Error + 'static)) -> FsListManyPartialError {
    let code = match error_code(error) {
        Some(FS_PATH_NOT_FOUND) => WIRE_FS_PATH_NOT_FOUND,
        Some(FS_IS_DIRECTORY) => WIRE_FS_IS_DIRECTORY,
        Some(FS_IS_BINARY) => WIRE_FS_IS_BINARY,
        Some(FS_TOO_LARGE) => WIRE_FS_TOO_LARGE,
        Some(super::FS_TOO_MANY_RESULTS) => WIRE_FS_TOO_MANY_RESULTS,
        _ => WIRE_INTERNAL_ERROR,
    };
    FsListManyPartialError {
        code,
        msg: error.to_string(),
    }
}

fn guess_mime(relative: &str, binary: bool) -> &'static str {
    match extension(relative) {
        "ts" | "tsx" => "text/typescript",
        "js" | "jsx" | "mjs" | "cjs" => "text/javascript",
        "json" => "application/json",
        "md" => "text/markdown",
        "html" => "text/html",
        "css" => "text/css",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "pdf" => "application/pdf",
        "yaml" | "yml" => "text/yaml",
        "toml" => "application/toml",
        "sh" => "text/x-shellscript",
        "py" => "text/x-python",
        "rs" => "text/rust",
        "go" => "text/x-go",
        _ if binary => "application/octet-stream",
        _ => "text/plain",
    }
}

fn guess_language_id(relative: &str) -> Option<&'static str> {
    Some(match extension(relative) {
        "ts" => "typescript",
        "tsx" => "typescriptreact",
        "js" | "mjs" | "cjs" => "javascript",
        "jsx" => "javascriptreact",
        "json" => "json",
        "md" => "markdown",
        "html" => "html",
        "css" => "css",
        "yaml" | "yml" => "yaml",
        "toml" => "toml",
        "sh" => "shellscript",
        "py" => "python",
        "rs" => "rust",
        "go" => "go",
        _ => return None,
    })
}

fn extension(path: &str) -> &str {
    Path::new(path)
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default()
}

pub fn register_session_fs_service() {
    register_scoped_service(
        LifecycleScope::Session,
        SESSION_FS_SERVICE_ID,
        SyncDescriptor::new(|accessor| {
            let workspace = accessor.get(SESSION_WORKSPACE_CONTEXT_ID)?;
            let host_fs = accessor.get(HOST_FILE_SYSTEM_SERVICE_ID)?;
            let runner = accessor.get(SESSION_PROCESS_RUNNER_SERVICE_ID)?;
            let telemetry = accessor.get(TELEMETRY_SERVICE_ID)?;
            let git = accessor.get(FS_GIT_SERVICE_ID)?;
            let service: Arc<dyn SessionFsServiceContract> = Arc::new(SessionFsService::new(
                (*workspace).clone(),
                (*host_fs).clone(),
                (*runner).clone(),
                (*telemetry).clone(),
                (*git).clone(),
            ));
            Ok(SessionFsServiceHandle(service))
        }),
        InstantiationType::Eager,
        "sessionFs",
    );
}

#[cfg(test)]
mod tests {
    use std::{
        path::PathBuf,
        time::{SystemTime, UNIX_EPOCH},
    };
    use std::sync::{Arc};
    use parking_lot::Mutex;

    use async_trait::async_trait;

    use crate::{
        app::{
            git::{FsGitStatus, FsGitStatusResponse, GitServiceContract},
            telemetry::noop_telemetry_service,
        },
        os::backends::node_local::host_fs_service::HostFileSystem,
        session::{
            process::{SessionProcess, SessionProcessRunnerResult},
            workspace_context::{
                PathAccessError, PathAccessOperation, SessionWorkspaceContextContract,
            },
        },
    };

    use super::*;

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> Self {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = std::env::temp_dir()
                .join(format!("kimi-session-fs-{}-{nonce}", std::process::id()));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    struct TestWorkspace(PathBuf);

    impl SessionWorkspaceContextContract for TestWorkspace {
        fn work_dir(&self) -> PathBuf {
            self.0.clone()
        }
        fn additional_dirs(&self) -> Vec<PathBuf> {
            Vec::new()
        }
        fn set_work_dir(&self, _: &str) -> std::io::Result<()> {
            Ok(())
        }
        fn set_additional_dirs(&self, _: &[String]) -> std::io::Result<()> {
            Ok(())
        }
        fn resolve(&self, relative: &str) -> PathBuf {
            if Path::new(relative).is_absolute() {
                PathBuf::from(relative)
            } else {
                self.0.join(relative)
            }
        }
        fn is_within(&self, absolute_path: &str) -> bool {
            Path::new(absolute_path).strip_prefix(&self.0).is_ok()
        }
        fn assert_allowed(
            &self,
            absolute_path: &str,
            operation: PathAccessOperation,
        ) -> Result<PathBuf, PathAccessError> {
            let path = self.resolve(absolute_path);
            if self.is_within(&path.to_string_lossy()) {
                Ok(path)
            } else {
                Err(PathAccessError { path, operation })
            }
        }
        fn add_additional_dir(&self, _: &str) -> std::io::Result<()> {
            Ok(())
        }
        fn remove_additional_dir(&self, _: &str) -> std::io::Result<()> {
            Ok(())
        }
    }

    struct MissingRunner;

    #[async_trait]
    impl crate::session::process::SessionProcessRunnerContract for MissingRunner {
        async fn exec(
            &self,
            _: &[String],
            _: Option<ProcessExecOptions>,
        ) -> SessionProcessRunnerResult<SessionProcess> {
            Err(Box::new(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "rg missing",
            )))
        }
    }

    #[derive(Default)]
    struct TestGit {
        status_calls: Mutex<Vec<(String, Option<HashSet<String>>)>>,
        diff_calls: Mutex<Vec<(String, String, String)>>,
    }

    #[async_trait]
    impl GitServiceContract for TestGit {
        async fn status(
            &self,
            cwd: &str,
            path_filter: Option<&HashSet<String>>,
        ) -> crate::app::git::GitServiceResult<FsGitStatusResponse> {
            self.status_calls
                .lock()
                .push((cwd.into(), path_filter.cloned()));
            Ok(FsGitStatusResponse {
                branch: "main".into(),
                ahead: 0,
                behind: 0,
                entries: IndexMap::from([("src/lib.rs".into(), FsGitStatus::Modified)]),
                additions: 1,
                deletions: 0,
                pull_request: None,
            })
        }

        async fn diff(
            &self,
            cwd: &str,
            relative_path: &str,
            absolute_path: &str,
        ) -> crate::app::git::GitServiceResult<FsDiffResponse> {
            self.diff_calls.lock().push((
                cwd.into(),
                relative_path.into(),
                absolute_path.into(),
            ));
            Ok(FsDiffResponse {
                path: relative_path.into(),
                diff: "diff".into(),
                truncated: false,
            })
        }
    }

    fn service(root: &Path, git: Arc<TestGit>) -> SessionFsService {
        SessionFsService::new(
            SessionWorkspaceContextHandle(Arc::new(TestWorkspace(root.to_path_buf()))),
            HostFileSystemServiceHandle(Arc::new(HostFileSystem)),
            SessionProcessRunnerHandle(Arc::new(MissingRunner)),
            noop_telemetry_service(),
            GitServiceHandle(git),
        )
    }

    fn error2(error: &SessionFsError) -> &Error2 {
        error.downcast_ref::<Error2>().expect("expected Error2")
    }

    #[tokio::test]
    async fn list_read_stat_mkdir_and_search_preserve_source_results() {
        let directory = TestDirectory::new();
        std::fs::create_dir(directory.0.join("src")).unwrap();
        std::fs::write(directory.0.join("src/lib.rs"), "alpha\nbeta\n").unwrap();
        std::fs::write(directory.0.join("README.md"), "readme").unwrap();
        let service = service(&directory.0, Arc::new(TestGit::default()));

        let listing = service
            .list(FsListRequest {
                depth: 2,
                ..FsListRequest::default()
            })
            .await
            .unwrap();
        assert_eq!(
            listing
                .items
                .iter()
                .map(|entry| (entry.name.as_str(), entry.kind))
                .collect::<Vec<_>>(),
            [("src", FsKind::Directory), ("README.md", FsKind::File)]
        );
        assert_eq!(
            listing.children_by_path.unwrap()["src"][0].path,
            "src/lib.rs"
        );

        let read = service
            .read(FsReadRequest {
                path: "src/lib.rs".into(),
                offset: 6,
                length: 4,
                encoding: FsReadRequestEncoding::Auto,
            })
            .await
            .unwrap();
        assert_eq!(read.content, "beta");
        assert!(read.truncated);
        assert_eq!(read.language_id.as_deref(), Some("rust"));

        let stat = service
            .stat(FsStatRequest {
                path: "README.md".into(),
            })
            .await
            .unwrap();
        assert_eq!(stat.mime.as_deref(), Some("text/markdown"));

        let created = service
            .mkdir(FsMkdirRequest {
                path: "nested/new".into(),
                recursive: true,
            })
            .await
            .unwrap();
        assert_eq!(created.kind, FsKind::Directory);

        let search = service
            .search(FsSearchRequest {
                query: "lib".into(),
                include_globs: Some(vec!["src/{lib,main}[.]rs".into()]),
                ..FsSearchRequest::default()
            })
            .await
            .unwrap();
        assert_eq!(search.items[0].path, "src/lib.rs");
    }

    #[tokio::test]
    async fn binary_batch_errors_and_node_grep_match_source_behavior() {
        let directory = TestDirectory::new();
        std::fs::create_dir(directory.0.join("src")).unwrap();
        std::fs::write(directory.0.join("src/a.txt"), "one\nneedle\nthree").unwrap();
        std::fs::write(directory.0.join("binary.bin"), [0, 1, 2]).unwrap();
        let service = service(&directory.0, Arc::new(TestGit::default()));

        let binary = service
            .read(FsReadRequest {
                path: "binary.bin".into(),
                offset: 0,
                length: 10,
                encoding: FsReadRequestEncoding::Auto,
            })
            .await
            .unwrap();
        assert_eq!(binary.encoding, FsReadEncoding::Base64);
        assert!(binary.is_binary);
        let error = service
            .read(FsReadRequest {
                encoding: FsReadRequestEncoding::Utf8,
                ..FsReadRequest {
                    path: "binary.bin".into(),
                    offset: 0,
                    length: 10,
                    encoding: FsReadRequestEncoding::Auto,
                }
            })
            .await
            .unwrap_err();
        assert_eq!(error2(&error).code, FS_IS_BINARY);

        let batch = service
            .list_many(FsListManyRequest {
                paths: vec!["src".into(), "missing".into()],
                ..FsListManyRequest::default()
            })
            .await
            .unwrap();
        assert!(batch.results.contains_key("src"));
        assert_eq!(
            batch.partial_errors.unwrap()["missing"].code,
            WIRE_FS_PATH_NOT_FOUND
        );

        let grep = service
            .grep(FsGrepRequest {
                pattern: "needle".into(),
                context_lines: 1,
                ..FsGrepRequest::default()
            })
            .await
            .unwrap();
        assert_eq!(grep.files[0].matches[0].line, 2);
        assert_eq!(grep.files[0].matches[0].before, ["one"]);
        assert_eq!(grep.files[0].matches[0].after, ["three"]);
    }

    #[tokio::test]
    async fn git_delegation_and_confinement_use_session_paths() {
        let directory = TestDirectory::new();
        std::fs::create_dir(directory.0.join("src")).unwrap();
        std::fs::write(directory.0.join("src/lib.rs"), "x").unwrap();
        let git = Arc::new(TestGit::default());
        let service = service(&directory.0, Arc::clone(&git));

        let status = service
            .git_status(FsGitStatusRequest {
                paths: Some(vec!["src/lib.rs".into()]),
            })
            .await
            .unwrap();
        assert_eq!(status.branch, "main");
        assert_eq!(
            git.status_calls.lock()[0].1,
            Some(HashSet::from(["src/lib.rs".into()]))
        );
        let diff = service
            .diff(FsDiffRequest {
                path: "src/lib.rs".into(),
            })
            .await
            .unwrap();
        assert_eq!(diff.diff, "diff");
        assert_eq!(git.diff_calls.lock()[0].1, "src/lib.rs");

        let escaped = service
            .stat(FsStatRequest {
                path: "../outside".into(),
            })
            .await
            .unwrap_err();
        assert_eq!(error2(&escaped).code, FS_PATH_ESCAPES);
    }

    #[test]
    fn rg_json_accumulator_preserves_context_caps_and_wire_fields() {
        let mut accumulator = RgJsonAccumulator::new(FsGrepRequest {
            pattern: "needle".into(),
            context_lines: 1,
            max_total_matches: 1,
            ..FsGrepRequest::default()
        });
        for record in [
            r#"{"type":"begin","data":{"path":{"text":"./src/a.txt"}}}"#,
            r#"{"type":"context","data":{"path":{"text":"./src/a.txt"},"lines":{"text":"before\n"},"line_number":1}}"#,
            r#"{"type":"match","data":{"path":{"text":"./src/a.txt"},"lines":{"text":"needle\n"},"line_number":2,"submatches":[{"start":3,"end":9}]}}"#,
            r#"{"type":"context","data":{"path":{"text":"./src/a.txt"},"lines":{"text":"after\n"},"line_number":3}}"#,
            r#"{"type":"end","data":{"path":{"text":"./src/a.txt"}}}"#,
        ] {
            accumulator.feed(record);
        }
        assert!(accumulator.capped());
        let response = accumulator.finish(false, 12).unwrap();
        assert!(response.truncated);
        assert_eq!(response.files_scanned, 1);
        assert_eq!(response.files[0].path, "src/a.txt");
        assert_eq!(response.files[0].matches[0].line, 2);
        assert_eq!(response.files[0].matches[0].col, 4);
        assert_eq!(response.files[0].matches[0].before, ["before"]);
        assert_eq!(response.files[0].matches[0].after, ["after"]);
    }
}
