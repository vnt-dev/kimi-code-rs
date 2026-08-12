//! System-prompt context assembly from workspace instructions and listings.
//!
//! Original: `packages/agent-core-v2/src/agent/profile/context.ts`.

use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

use futures_util::future::join_all;

use crate::{
    _base::exec_env::decode_text::{TextDecodeErrors, TextEncoding},
    app::agent_profile_catalog::AgentProfileContext,
    os::interface::host_file_system::{HostFileSystemServiceHandle, ReadTextOptions},
};

pub const AGENTS_MD_RECOMMENDED_MAX_BYTES: usize = 32 * 1024;
pub const LIST_DIR_ROOT_WIDTH: usize = 30;
pub const LIST_DIR_CHILD_WIDTH: usize = 10;

#[derive(Clone)]
pub struct ProfileContextDeps {
    pub fs: HostFileSystemServiceHandle,
    pub home_dir: PathBuf,
}

pub type PreparedSystemPromptContext = AgentProfileContext;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PrepareSystemPromptContextOptions {
    pub additional_dirs: Option<Vec<String>>,
}

// Original: prepareSystemPromptContext(). The three independent filesystem
// reads stay concurrent, matching Promise.all's call and result order.
pub async fn prepare_system_prompt_context(
    deps: &ProfileContextDeps,
    work_dir: &Path,
    brand_home: Option<&Path>,
    options: Option<&PrepareSystemPromptContextOptions>,
) -> PreparedSystemPromptContext {
    let additional_dirs = dedupe_dirs(
        options
            .and_then(|options| options.additional_dirs.as_deref())
            .unwrap_or_default(),
    );
    let work_dirs = [work_dir];
    let (cwd_listing, agents_md, additional_dirs_info) = tokio::join!(
        list_directory(
            deps,
            work_dir,
            ListDirectoryOptions {
                collapse_hidden_dirs: true
            }
        ),
        load_agents_md_for_roots(deps, brand_home, &work_dirs),
        load_additional_dirs_info(deps, &additional_dirs),
    );
    AgentProfileContext {
        cwd_listing: Some(cwd_listing),
        agents_md: Some(agents_md.content),
        additional_dirs_info: Some(additional_dirs_info),
        agents_md_warning: agents_md.warning,
        ..AgentProfileContext::default()
    }
}

// Original: loadAgentsMd().
pub async fn load_agents_md(
    deps: &ProfileContextDeps,
    work_dir: &Path,
    brand_home: Option<&Path>,
) -> String {
    load_agents_md_for_roots(deps, brand_home, &[work_dir])
        .await
        .content
}

struct LoadedAgentsMd {
    content: String,
    warning: Option<String>,
}

async fn load_agents_md_for_roots(
    deps: &ProfileContextDeps,
    brand_home: Option<&Path>,
    work_dirs: &[&Path],
) -> LoadedAgentsMd {
    let mut discovered = Vec::new();
    let mut seen = HashSet::new();
    let mut warnings = Vec::new();

    let brand_dir = brand_home
        .map(Path::to_path_buf)
        .unwrap_or_else(|| deps.home_dir.join(".kimi-code"));
    collect_agent_file(
        deps,
        &brand_dir.join("AGENTS.md"),
        &mut seen,
        &mut discovered,
        &mut warnings,
    )
    .await;

    let generic_dir = deps.home_dir.join(".agents");
    for name in ["AGENTS.md", "agents.md"] {
        if collect_agent_file(
            deps,
            &generic_dir.join(name),
            &mut seen,
            &mut discovered,
            &mut warnings,
        )
        .await
        {
            break;
        }
    }

    for work_dir in work_dirs {
        let root_work_dir = path_clean::clean(work_dir);
        let project_root = find_project_root(deps, &root_work_dir).await;
        for directory in dirs_root_to_leaf(&root_work_dir, &project_root) {
            collect_agent_file(
                deps,
                &directory.join(".kimi-code").join("AGENTS.md"),
                &mut seen,
                &mut discovered,
                &mut warnings,
            )
            .await;
            for name in ["AGENTS.md", "agents.md"] {
                if collect_agent_file(
                    deps,
                    &directory.join(name),
                    &mut seen,
                    &mut discovered,
                    &mut warnings,
                )
                .await
                {
                    break;
                }
            }
        }
    }

    let content = render_agent_files(&discovered);
    if content.len() > AGENTS_MD_RECOMMENDED_MAX_BYTES {
        warnings.push(format!(
            "AGENTS.md total {} KB exceeds the recommended {} KB. Large instruction files increase cost and may impact performance; consider trimming.",
            format_kb(content.len()),
            format_kb(AGENTS_MD_RECOMMENDED_MAX_BYTES),
        ));
    }
    LoadedAgentsMd {
        content,
        warning: (!warnings.is_empty()).then(|| warnings.join("\n")),
    }
}

async fn collect_agent_file(
    deps: &ProfileContextDeps,
    path: &Path,
    seen: &mut HashSet<PathBuf>,
    discovered: &mut Vec<AgentFile>,
    warnings: &mut Vec<String>,
) -> bool {
    let Some(file) = read_agent_file(deps, path, warnings).await else {
        return false;
    };
    let key = path_clean::clean(&file.path);
    if !seen.insert(key) {
        return false;
    }
    discovered.push(file);
    true
}

async fn load_additional_dirs_info(
    deps: &ProfileContextDeps,
    additional_dirs: &[String],
) -> String {
    join_all(additional_dirs.iter().map(|directory| async move {
        let listing =
            list_directory(deps, Path::new(directory), ListDirectoryOptions::default()).await;
        format!("### {directory}\n{listing}")
    }))
    .await
    .join("\n\n")
}

async fn find_project_root(deps: &ProfileContextDeps, work_dir: &Path) -> PathBuf {
    let initial = path_clean::clean(work_dir);
    let mut current = initial.clone();
    loop {
        if path_exists(deps, &current.join(".git")).await {
            return current;
        }
        let Some(parent) = current.parent() else {
            return initial;
        };
        if parent == current {
            return initial;
        }
        current = parent.to_path_buf();
    }
}

fn dirs_root_to_leaf(work_dir: &Path, project_root: &Path) -> Vec<PathBuf> {
    let mut directories = Vec::new();
    let mut current = path_clean::clean(work_dir);
    loop {
        directories.push(current.clone());
        if current == project_root {
            break;
        }
        let Some(parent) = current.parent() else {
            break;
        };
        if parent == current {
            break;
        }
        current = parent.to_path_buf();
    }
    directories.reverse();
    directories
}

struct AgentFile {
    path: PathBuf,
    content: String,
}

async fn read_agent_file(
    deps: &ProfileContextDeps,
    path: &Path,
    warnings: &mut Vec<String>,
) -> Option<AgentFile> {
    if !is_file(deps, path).await {
        if entry_exists(deps, path).await {
            warnings.push(format!(
                "Instruction file at {} exists but is not a readable regular file; skipping.",
                path.display()
            ));
        }
        return None;
    }
    let content = match deps
        .fs
        .read_text(
            path,
            Some(ReadTextOptions {
                encoding: TextEncoding::Utf8,
                errors: TextDecodeErrors::Ignore,
            }),
        )
        .await
    {
        Ok(content) => content.trim().to_owned(),
        Err(_) => {
            warnings.push(format!(
                "Instruction file at {} could not be read; skipping.",
                path.display()
            ));
            return None;
        }
    };
    (!content.is_empty()).then(|| AgentFile {
        path: path.to_path_buf(),
        content,
    })
}

async fn path_exists(deps: &ProfileContextDeps, path: &Path) -> bool {
    deps.fs.lstat(path).await.is_ok()
}

async fn entry_exists(deps: &ProfileContextDeps, path: &Path) -> bool {
    path_exists(deps, path).await
}

async fn is_file(deps: &ProfileContextDeps, path: &Path) -> bool {
    deps.fs.stat(path).await.is_ok_and(|stat| stat.is_file)
}

fn render_agent_files(files: &[AgentFile]) -> String {
    files
        .iter()
        .map(|file| format!("<!-- From: {} -->\n{}", file.path.display(), file.content))
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn format_kb(bytes: usize) -> String {
    let kb = bytes as f64 / 1024.0;
    if kb.fract() == 0.0 {
        (kb as usize).to_string()
    } else {
        format!("{kb:.1}")
    }
}

fn dedupe_dirs(directories: &[String]) -> Vec<String> {
    let mut seen = HashSet::new();
    directories
        .iter()
        .filter_map(|directory| {
            let trimmed = directory.trim();
            (!trimmed.is_empty() && seen.insert(trimmed.to_owned())).then(|| trimmed.to_owned())
        })
        .collect()
}

#[derive(Clone, Copy, Default)]
struct ListDirectoryOptions {
    collapse_hidden_dirs: bool,
}

struct DirectoryEntry {
    name: String,
    is_dir: bool,
}

async fn collect_entries(
    deps: &ProfileContextDeps,
    directory: &Path,
    max_width: usize,
) -> (Vec<DirectoryEntry>, usize, bool) {
    let Ok(entries) = deps.fs.read_dir(directory).await else {
        return (Vec::new(), 0, false);
    };
    let mut entries = entries
        .into_iter()
        .map(|entry| DirectoryEntry {
            name: entry.name,
            is_dir: entry.is_directory,
        })
        .collect::<Vec<_>>();
    // Node's localeCompare is locale-dependent; retain its directory-first
    // ordering and use deterministic Unicode scalar ordering for names.
    entries.sort_by(|left, right| {
        right
            .is_dir
            .cmp(&left.is_dir)
            .then_with(|| left.name.cmp(&right.name))
    });
    let total = entries.len();
    entries.truncate(max_width);
    (entries, total, true)
}

async fn list_directory(
    deps: &ProfileContextDeps,
    work_dir: &Path,
    options: ListDirectoryOptions,
) -> String {
    let (entries, total, readable) = collect_entries(deps, work_dir, LIST_DIR_ROOT_WIDTH).await;
    if !readable {
        return "[not readable]".into();
    }
    let remaining = total - entries.len();
    let mut lines = Vec::new();
    for (index, entry) in entries.iter().enumerate() {
        let is_last = index + 1 == entries.len() && remaining == 0;
        let connector = if is_last { "└── " } else { "├── " };
        if !entry.is_dir {
            lines.push(format!("{connector}{}", entry.name));
            continue;
        }
        lines.push(format!("{connector}{}/", entry.name));
        if options.collapse_hidden_dirs && entry.name.starts_with('.') {
            continue;
        }
        let child_prefix = if is_last { "    " } else { "│   " };
        let (children, child_total, child_readable) =
            collect_entries(deps, &work_dir.join(&entry.name), LIST_DIR_CHILD_WIDTH).await;
        if !child_readable {
            lines.push(format!("{child_prefix}└── [not readable]"));
            continue;
        }
        let child_remaining = child_total - children.len();
        for (child_index, child) in children.iter().enumerate() {
            let child_is_last = child_index + 1 == children.len() && child_remaining == 0;
            let child_connector = if child_is_last {
                "└── "
            } else {
                "├── "
            };
            let suffix = if child.is_dir { "/" } else { "" };
            lines.push(format!(
                "{child_prefix}{child_connector}{}{suffix}",
                child.name
            ));
        }
        if child_remaining > 0 {
            lines.push(format!("{child_prefix}└── ... and {child_remaining} more"));
        }
    }
    if remaining > 0 {
        lines.push(format!("└── ... and {remaining} more entries"));
    }
    if lines.is_empty() {
        "(empty directory)".into()
    } else {
        lines.join("\n")
    }
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        sync::Arc,
        time::{SystemTime, UNIX_EPOCH},
    };

    use crate::os::backends::node_local::host_fs_service::HostFileSystem;

    use super::*;

    struct TestDir(PathBuf);

    impl TestDir {
        fn new(label: &str) -> Self {
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = std::env::temp_dir().join(format!("kimi-profile-context-{label}-{nanos}"));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn deps(home_dir: PathBuf) -> ProfileContextDeps {
        ProfileContextDeps {
            fs: crate::os::interface::host_file_system::HostFileSystemServiceHandle(Arc::new(
                HostFileSystem,
            )),
            home_dir,
        }
    }

    #[tokio::test]
    async fn loads_agent_files_in_source_precedence_order() {
        let root = TestDir::new("agents");
        let home = root.0.join("home");
        let brand = root.0.join("brand");
        let project = root.0.join("project");
        let nested = project.join("nested");
        fs::create_dir_all(home.join(".agents")).unwrap();
        fs::create_dir_all(&brand).unwrap();
        fs::create_dir_all(project.join(".git")).unwrap();
        fs::create_dir_all(nested.join(".kimi-code")).unwrap();
        fs::write(brand.join("AGENTS.md"), " brand ").unwrap();
        fs::write(home.join(".agents/AGENTS.md"), "generic").unwrap();
        fs::write(project.join("AGENTS.md"), "project").unwrap();
        fs::write(nested.join(".kimi-code/AGENTS.md"), "nested").unwrap();

        let content = load_agents_md(&deps(home.clone()), &nested, Some(&brand)).await;
        assert_eq!(
            content,
            format!(
                "<!-- From: {} -->\nbrand\n\n<!-- From: {} -->\ngeneric\n\n<!-- From: {} -->\nproject\n\n<!-- From: {} -->\nnested",
                brand.join("AGENTS.md").display(),
                home.join(".agents").join("AGENTS.md").display(),
                project.join("AGENTS.md").display(),
                nested.join(".kimi-code").join("AGENTS.md").display(),
            )
        );
    }

    #[tokio::test]
    async fn lists_hidden_roots_without_descending_and_reports_overflow() {
        let root = TestDir::new("listing");
        fs::create_dir_all(root.0.join(".git")).unwrap();
        fs::write(root.0.join(".git/config"), "x").unwrap();
        fs::create_dir_all(root.0.join("src")).unwrap();
        fs::write(root.0.join("src/main.rs"), "x").unwrap();
        for index in 0..30 {
            fs::write(root.0.join(format!("file-{index:02}")), "x").unwrap();
        }
        let output = list_directory(
            &deps(root.0.clone()),
            &root.0,
            ListDirectoryOptions {
                collapse_hidden_dirs: true,
            },
        )
        .await;
        assert!(output.starts_with("├── .git/\n├── src/\n│   └── main.rs"));
        assert!(!output.contains("config"));
        assert!(output.ends_with("└── ... and 2 more entries"));
    }

    #[test]
    fn dedupe_and_warning_size_match_source_boundaries() {
        assert_eq!(
            dedupe_dirs(&[" /a ".into(), "".into(), "/a".into(), "/b".into()]),
            vec!["/a", "/b"]
        );
        assert_eq!(format_kb(AGENTS_MD_RECOMMENDED_MAX_BYTES), "32");
        assert_eq!(format_kb(AGENTS_MD_RECOMMENDED_MAX_BYTES + 512), "32.5");
    }
}
