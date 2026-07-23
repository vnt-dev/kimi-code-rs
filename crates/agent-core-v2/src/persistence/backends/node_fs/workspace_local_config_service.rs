//! Node-local `.kimi-code/local.toml` workspace configuration backend.
//!
//! Original:
//! `packages/agent-core-v2/src/persistence/backends/node-fs/workspaceLocalConfigService.ts`.

use std::{
    error::Error,
    path::{Component, Path, PathBuf},
    sync::Arc,
};

use async_trait::async_trait;
use serde_json::{Map, Value};

use crate::{
    _base::{
        di::{
            descriptors::SyncDescriptor,
            instantiation::ServicesAccessorExt,
            scope::{InstantiationType, LifecycleScope, register_scoped_service},
        },
        errors::errors::{Error2, Error2Options, ErrorCause},
        exec_env::decode_text::{TextDecodeErrors, TextEncoding},
    },
    app::{
        bootstrap::{BOOTSTRAP_SERVICE_ID, BootstrapServiceContract},
        config::{CONFIG_INVALID, ensure_config_errors_registered},
        workspace_local_config::{
            WORKSPACE_LOCAL_CONFIG_SERVICE_ID, WorkspaceAdditionalDirsLoadResult,
            WorkspaceLocalConfigResult, WorkspaceLocalConfigServiceContract,
            WorkspaceLocalConfigServiceHandle,
        },
    },
    os::interface::{
        host_file_system::{HOST_FILE_SYSTEM_SERVICE_ID, HostFileSystemService, ReadTextOptions},
        host_fs_errors::{HostFsError, OS_FS_NOT_DIRECTORY, OS_FS_NOT_FOUND},
    },
    persistence::interface::storage::{
        STORAGE_DECODE_FAILED, StorageError, ensure_storage_errors_registered, to_storage_io_error,
    },
};

pub struct FileWorkspaceLocalConfigService {
    bootstrap: Arc<dyn BootstrapServiceContract>,
    fs: Arc<dyn HostFileSystemService>,
}

impl FileWorkspaceLocalConfigService {
    pub fn new(
        bootstrap: Arc<dyn BootstrapServiceContract>,
        fs: Arc<dyn HostFileSystemService>,
    ) -> Self {
        ensure_config_errors_registered();
        ensure_storage_errors_registered();
        Self { bootstrap, fs }
    }

    fn workspace_local_config_path(project_root: &str) -> String {
        path_to_string(&normalize_path(
            &Path::new(project_root)
                .join(".kimi-code")
                .join("local.toml"),
        ))
    }

    // Original: FileWorkspaceLocalConfigService.findProjectRoot(). Any `.git`
    // filesystem entry marks a project root; failure to find one returns the
    // normalized initial directory rather than an error.
    async fn find_project_root(&self, work_dir: &str) -> String {
        let initial = path_to_string(&normalize_path(Path::new(work_dir)));
        let mut current = initial.clone();
        loop {
            if self.path_exists(&Path::new(&current).join(".git")).await {
                return current;
            }
            let parent = dirname(&current);
            if parent == current {
                return initial;
            }
            current = parent;
        }
    }

    // Original: FileWorkspaceLocalConfigService.readWorkspaceLocalToml().
    async fn read_workspace_local_toml(
        &self,
        config_path: &str,
    ) -> WorkspaceLocalConfigResult<Option<WorkspaceLocalTomlFile>> {
        let text = match self
            .fs
            .read_text(
                Path::new(config_path),
                Some(ReadTextOptions {
                    encoding: TextEncoding::Utf8,
                    errors: TextDecodeErrors::Replace,
                }),
            )
            .await
        {
            Ok(text) => text,
            Err(error) if is_path_missing(&error) => return Ok(None),
            Err(error) => {
                return Err(Box::new(to_storage_io_error(
                    Box::new(error),
                    config_path,
                    "read",
                )));
            }
        };
        if text.trim().is_empty() {
            return Ok(Some(WorkspaceLocalTomlFile::default()));
        }

        let raw = toml::from_str::<toml::Table>(&text).map_err(|error| {
            let cause: Arc<dyn Error + Send + Sync> = Arc::new(error);
            Box::new(StorageError::with_options(
                STORAGE_DECODE_FAILED,
                format!("Invalid TOML in {config_path}"),
                Error2Options {
                    details: Some(Map::from_iter([
                        ("path".into(), Value::String(config_path.into())),
                        ("format".into(), Value::String("toml".into())),
                    ])),
                    cause: Some(ErrorCause::Error(cause)),
                    ..Error2Options::default()
                },
            )) as Box<dyn Error + Send + Sync>
        })?;
        let additional_dirs = parse_workspace_local_toml(&raw)?;
        Ok(Some(WorkspaceLocalTomlFile {
            raw,
            additional_dirs,
        }))
    }

    async fn resolve_additional_dirs_internal(
        &self,
        base_dir: &str,
        additional_dirs: &[String],
    ) -> WorkspaceLocalConfigResult<Vec<String>> {
        let mut resolved = Vec::new();
        for additional_dir in normalize_additional_dirs(additional_dirs) {
            let additional_dir = self
                .resolve_additional_dir(base_dir, &additional_dir)
                .await?;
            if !has_same_additional_dir(&resolved, &additional_dir) {
                resolved.push(additional_dir);
            }
        }
        Ok(resolved)
    }

    fn resolve_existing_additional_dirs(
        &self,
        project_root: &str,
        additional_dirs: &[String],
    ) -> Vec<String> {
        let mut resolved = Vec::new();
        for additional_dir in normalize_additional_dirs(additional_dirs) {
            let additional_dir = self.resolve_path(project_root, &additional_dir);
            if !has_same_additional_dir(&resolved, &additional_dir) {
                resolved.push(additional_dir);
            }
        }
        resolved
    }

    async fn resolve_additional_dir(
        &self,
        base_dir: &str,
        additional_dir: &str,
    ) -> WorkspaceLocalConfigResult<String> {
        let normalized = normalize_additional_dir_input(additional_dir)?;
        let resolved = self.resolve_path(base_dir, &normalized);
        self.assert_directory(&resolved).await?;
        Ok(resolved)
    }

    fn resolve_path(&self, base_dir: &str, additional_dir: &str) -> String {
        let expanded = self.expand_home(additional_dir);
        let expanded = Path::new(&expanded);
        let path = if expanded.is_absolute() {
            expanded.to_path_buf()
        } else {
            let base = Path::new(base_dir);
            if base.is_absolute() {
                base.join(expanded)
            } else {
                self.bootstrap.cwd().join(base).join(expanded)
            }
        };
        path_to_string(&normalize_path(&path))
    }

    fn expand_home(&self, value: &str) -> String {
        if value == "~" {
            return path_to_string(self.bootstrap.os_home_dir());
        }
        if let Some(relative) = value.strip_prefix("~/") {
            return path_to_string(&self.bootstrap.os_home_dir().join(relative));
        }
        value.into()
    }

    async fn assert_directory(&self, path: &str) -> WorkspaceLocalConfigResult<()> {
        let stat = match self.fs.stat(Path::new(path)).await {
            Ok(stat) => stat,
            Err(error) if is_path_missing(&error) => return Err(config_directory_error()),
            Err(error) => {
                return Err(Box::new(to_storage_io_error(Box::new(error), path, "stat")));
            }
        };
        if !stat.is_directory {
            return Err(config_directory_error());
        }
        Ok(())
    }

    async fn path_exists(&self, path: &Path) -> bool {
        self.fs.lstat(path).await.is_ok()
    }
}

#[async_trait]
impl WorkspaceLocalConfigServiceContract for FileWorkspaceLocalConfigService {
    // Original: FileWorkspaceLocalConfigService.readAdditionalDirs().
    async fn read_additional_dirs(
        &self,
        work_dir: &str,
    ) -> WorkspaceLocalConfigResult<WorkspaceAdditionalDirsLoadResult> {
        let project_root = self.find_project_root(work_dir).await;
        let config_path = Self::workspace_local_config_path(&project_root);
        let file = self.read_workspace_local_toml(&config_path).await?;
        let additional_dirs = match file.and_then(|file| file.additional_dirs) {
            None => Vec::new(),
            Some(dirs) => {
                self.resolve_additional_dirs_internal(&project_root, &dirs)
                    .await?
            }
        };
        Ok(WorkspaceAdditionalDirsLoadResult {
            project_root,
            config_path,
            additional_dirs,
        })
    }

    // Original: FileWorkspaceLocalConfigService.resolveAdditionalDirs().
    async fn resolve_additional_dirs(
        &self,
        base_dir: &str,
        additional_dirs: &[String],
    ) -> WorkspaceLocalConfigResult<Vec<String>> {
        self.resolve_additional_dirs_internal(base_dir, additional_dirs)
            .await
    }

    // Original: FileWorkspaceLocalConfigService.appendAdditionalDir().
    async fn append_additional_dir(
        &self,
        work_dir: &str,
        input_path: &str,
    ) -> WorkspaceLocalConfigResult<WorkspaceAdditionalDirsLoadResult> {
        let project_root = self.find_project_root(work_dir).await;
        let config_path = Self::workspace_local_config_path(&project_root);
        let additional_dir = self.resolve_additional_dir(work_dir, input_path).await?;
        let mut file = self
            .read_workspace_local_toml(&config_path)
            .await?
            .unwrap_or_default();
        let existing = self.resolve_existing_additional_dirs(
            &project_root,
            file.additional_dirs.as_deref().unwrap_or_default(),
        );
        if has_same_additional_dir(&existing, &additional_dir) {
            return Ok(WorkspaceAdditionalDirsLoadResult {
                project_root,
                config_path,
                additional_dirs: existing,
            });
        }

        let mut workspace = file
            .raw
            .get("workspace")
            .and_then(toml::Value::as_table)
            .cloned()
            .unwrap_or_default();
        let mut written_dirs = existing.clone();
        written_dirs.push(additional_dir);
        workspace.insert(
            "additional_dir".into(),
            toml::Value::Array(
                written_dirs
                    .iter()
                    .map(|path| toml::Value::String(path.clone()))
                    .collect(),
            ),
        );
        file.raw
            .insert("workspace".into(), toml::Value::Table(workspace));
        let directory = dirname(&config_path);
        let text = toml::to_string(&file.raw)?;
        let text = format!("{}\n", text.trim_end_matches('\n'));
        if let Err(error) = self.fs.create_dir(Path::new(&directory), true).await {
            return Err(Box::new(to_storage_io_error(
                Box::new(error),
                &config_path,
                "write",
            )));
        }
        if let Err(error) = self.fs.write_text(Path::new(&config_path), &text).await {
            return Err(Box::new(to_storage_io_error(
                Box::new(error),
                &config_path,
                "write",
            )));
        }

        Ok(WorkspaceAdditionalDirsLoadResult {
            project_root,
            config_path,
            additional_dirs: written_dirs,
        })
    }
}

#[derive(Default)]
struct WorkspaceLocalTomlFile {
    raw: toml::Table,
    additional_dirs: Option<Vec<String>>,
}

fn parse_workspace_local_toml(
    raw: &toml::Table,
) -> WorkspaceLocalConfigResult<Option<Vec<String>>> {
    let Some(workspace) = raw.get("workspace") else {
        return Ok(None);
    };
    let Some(workspace) = workspace.as_table() else {
        return Err(config_validation_error("workspace must be a table"));
    };
    let Some(toml::Value::Array(values)) = workspace.get("additional_dir") else {
        return Err(config_validation_error(
            "workspace.additional_dir must be an array of strings",
        ));
    };
    let mut additional_dirs = Vec::with_capacity(values.len());
    for value in values {
        let Some(value) = value.as_str() else {
            return Err(config_validation_error(
                "workspace.additional_dir must be an array of strings",
            ));
        };
        additional_dirs.push(value.into());
    }
    Ok(Some(additional_dirs))
}

fn normalize_additional_dirs(additional_dirs: &[String]) -> Vec<String> {
    let mut normalized = Vec::new();
    for additional_dir in additional_dirs {
        let additional_dir = path_to_string(&normalize_path(Path::new(additional_dir)));
        if !normalized.contains(&additional_dir) {
            normalized.push(additional_dir);
        }
    }
    normalized
}

fn normalize_additional_dir_input(additional_dir: &str) -> WorkspaceLocalConfigResult<String> {
    let trimmed = additional_dir.trim();
    if trimmed.is_empty() {
        return Err(config_directory_error());
    }
    Ok(path_to_string(&normalize_path(Path::new(trimmed))))
}

fn has_same_additional_dir(dirs: &[String], target: &str) -> bool {
    let target = normalize_path(Path::new(target));
    dirs.iter()
        .any(|dir| normalize_path(Path::new(dir)) == target)
}

fn config_directory_error() -> Box<dyn Error + Send + Sync> {
    config_invalid("workspace.additional_dir must exist and be a directory")
}

fn config_invalid(message: &str) -> Box<dyn Error + Send + Sync> {
    ensure_config_errors_registered();
    Box::new(Error2::new(CONFIG_INVALID, message))
}

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
struct WorkspaceLocalValidationError(String);

fn config_validation_error(message: &str) -> Box<dyn Error + Send + Sync> {
    ensure_config_errors_registered();
    let cause: Arc<dyn Error + Send + Sync> =
        Arc::new(WorkspaceLocalValidationError(message.into()));
    Box::new(Error2::with_options(
        CONFIG_INVALID,
        message,
        Error2Options {
            cause: Some(ErrorCause::Error(cause)),
            ..Error2Options::default()
        },
    ))
}

fn is_path_missing(error: &HostFsError) -> bool {
    matches!(error.code(), OS_FS_NOT_FOUND | OS_FS_NOT_DIRECTORY)
}

fn dirname(path: &str) -> String {
    let path = Path::new(path);
    match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => path_to_string(parent),
        _ if path.has_root() => path_to_string(path),
        _ => ".".into(),
    }
}

fn normalize_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(Path::new(std::path::MAIN_SEPARATOR_STR)),
            Component::CurDir => {}
            Component::ParentDir => {
                let can_pop = normalized
                    .file_name()
                    .is_some_and(|name| name != std::ffi::OsStr::new(".."));
                if can_pop {
                    normalized.pop();
                } else if !normalized.has_root() {
                    normalized.push("..");
                }
            }
            Component::Normal(component) => normalized.push(component),
        }
    }
    if normalized.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        normalized
    }
}

fn path_to_string(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

pub fn register_workspace_local_config_service() {
    ensure_config_errors_registered();
    ensure_storage_errors_registered();
    register_scoped_service(
        LifecycleScope::App,
        WORKSPACE_LOCAL_CONFIG_SERVICE_ID,
        SyncDescriptor::new(|accessor| {
            let bootstrap = accessor.get(BOOTSTRAP_SERVICE_ID)?;
            let fs = accessor.get(HOST_FILE_SYSTEM_SERVICE_ID)?;
            let service: Arc<dyn WorkspaceLocalConfigServiceContract> = Arc::new(
                FileWorkspaceLocalConfigService::new(Arc::clone(&bootstrap.0), Arc::clone(&fs.0)),
            );
            Ok(WorkspaceLocalConfigServiceHandle(service))
        }),
        InstantiationType::Eager,
        "workspaceLocalConfig",
    );
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use crate::{
        app::bootstrap::{BootstrapOptions, BootstrapService},
        os::backends::node_local::host_fs_service::HostFileSystem,
        persistence::interface::storage::{STORAGE_DECODE_FAILED, StorageError},
    };

    use super::*;

    fn test_service(root: &Path) -> FileWorkspaceLocalConfigService {
        let bootstrap: Arc<dyn BootstrapServiceContract> =
            Arc::new(BootstrapService::new(BootstrapOptions {
                home_dir: root.join("kimi-home"),
                config_path: root.join("kimi-home/config.toml"),
                os_home_dir: root.join("os-home"),
                platform: "linux".into(),
                arch: "x64".into(),
                cwd: root.into(),
                env: HashMap::new(),
                client_version: "test".into(),
            }));
        FileWorkspaceLocalConfigService::new(bootstrap, Arc::new(HostFileSystem))
    }

    async fn temp_tree() -> PathBuf {
        let root = std::env::temp_dir().join(format!("workspace-local-{}", uuid::Uuid::new_v4()));
        tokio::fs::create_dir_all(&root).await.unwrap();
        root
    }

    #[tokio::test]
    async fn discovers_git_root_resolves_deduplicates_and_expands_home() {
        let root = temp_tree().await;
        let repo = root.join("repo");
        let nested = repo.join("nested");
        let shared = root.join("shared");
        let home_extra = root.join("os-home/extra");
        for path in [&nested, &repo.join(".git"), &shared, &home_extra] {
            tokio::fs::create_dir_all(path).await.unwrap();
        }
        let config = repo.join(".kimi-code/local.toml");
        tokio::fs::create_dir_all(config.parent().unwrap())
            .await
            .unwrap();
        tokio::fs::write(
            &config,
            "[workspace]\nadditional_dir = [\"../shared\", \"../shared\", \"~/extra\"]\n",
        )
        .await
        .unwrap();
        let service = test_service(&root);

        let result = service
            .read_additional_dirs(nested.to_str().unwrap())
            .await
            .unwrap();
        assert_eq!(result.project_root, path_to_string(&repo));
        assert_eq!(
            result.additional_dirs,
            [path_to_string(&shared), path_to_string(&home_extra)]
        );
        tokio::fs::remove_dir_all(root).await.unwrap();
    }

    #[tokio::test]
    async fn appends_absolute_paths_preserves_unknown_toml_and_avoids_duplicates() {
        let root = temp_tree().await;
        let repo = root.join("repo");
        let first = root.join("first");
        let second = root.join("second");
        for path in [&repo.join(".git"), &first, &second] {
            tokio::fs::create_dir_all(path).await.unwrap();
        }
        let config = repo.join(".kimi-code/local.toml");
        tokio::fs::create_dir_all(config.parent().unwrap())
            .await
            .unwrap();
        tokio::fs::write(
            &config,
            "zeta = \"first\"\nalpha = \"second\"\n[workspace]\nadditional_dir = [\"../first\"]\n",
        )
        .await
        .unwrap();
        let service = test_service(&root);

        let result = service
            .append_additional_dir(repo.to_str().unwrap(), second.to_str().unwrap())
            .await
            .unwrap();
        assert_eq!(
            result.additional_dirs,
            [path_to_string(&first), path_to_string(&second)]
        );
        let duplicate = service
            .append_additional_dir(repo.to_str().unwrap(), second.to_str().unwrap())
            .await
            .unwrap();
        assert_eq!(duplicate.additional_dirs, result.additional_dirs);
        let written = tokio::fs::read_to_string(config).await.unwrap();
        assert!(written.contains("zeta = \"first\""));
        assert!(written.find("zeta").unwrap() < written.find("alpha").unwrap());
        assert_eq!(written.matches(&path_to_string(&second)).count(), 1);
        tokio::fs::remove_dir_all(root).await.unwrap();
    }

    #[tokio::test]
    async fn rejects_missing_non_directory_and_invalid_workspace_values() {
        let root = temp_tree().await;
        let repo = root.join("repo");
        tokio::fs::create_dir_all(repo.join(".git")).await.unwrap();
        let service = test_service(&root);

        let error = service
            .resolve_additional_dirs(repo.to_str().unwrap(), &["missing".into()])
            .await
            .unwrap_err();
        assert_eq!(error.downcast_ref::<Error2>().unwrap().code, CONFIG_INVALID);

        let file = repo.join("file.txt");
        tokio::fs::write(&file, b"x").await.unwrap();
        let error = service
            .resolve_additional_dirs(
                repo.to_str().unwrap(),
                &[file.to_string_lossy().into_owned()],
            )
            .await
            .unwrap_err();
        assert_eq!(error.downcast_ref::<Error2>().unwrap().code, CONFIG_INVALID);

        let config = repo.join(".kimi-code/local.toml");
        tokio::fs::create_dir_all(config.parent().unwrap())
            .await
            .unwrap();
        tokio::fs::write(&config, "workspace = \"wrong\"\n")
            .await
            .unwrap();
        let error = service
            .read_additional_dirs(repo.to_str().unwrap())
            .await
            .unwrap_err();
        let error = error.downcast_ref::<Error2>().unwrap();
        assert_eq!(error.code, CONFIG_INVALID);
        assert!(error.source().is_some());
        tokio::fs::remove_dir_all(root).await.unwrap();
    }

    #[tokio::test]
    async fn malformed_toml_maps_to_storage_decode_failed() {
        let root = temp_tree().await;
        let repo = root.join("repo");
        tokio::fs::create_dir_all(repo.join(".git")).await.unwrap();
        let config = repo.join(".kimi-code/local.toml");
        tokio::fs::create_dir_all(config.parent().unwrap())
            .await
            .unwrap();
        tokio::fs::write(&config, "[workspace\n").await.unwrap();
        let service = test_service(&root);

        let error = service
            .read_additional_dirs(repo.to_str().unwrap())
            .await
            .unwrap_err();
        assert_eq!(
            error.downcast_ref::<StorageError>().unwrap().code(),
            STORAGE_DECODE_FAILED
        );
        tokio::fs::remove_dir_all(root).await.unwrap();
    }
}
