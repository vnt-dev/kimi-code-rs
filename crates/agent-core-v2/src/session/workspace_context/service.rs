//! Session workspace context implementation.
//!
//! Original: `packages/agent-core-v2/src/session/workspaceContext/workspaceContextService.ts`.

use std::{
    path::{Component, Path, PathBuf},
    sync::{Arc, RwLock},
};

use crate::{
    _base::di::{
        descriptors::SyncDescriptor,
        errors::DiError,
        instantiation::ServicesAccessorExt,
        scope::{InstantiationType, LifecycleScope, register_scoped_service},
    },
    session::session_context::{SESSION_CONTEXT_ID, SessionContext},
};

use super::contract::{
    PathAccessError, PathAccessOperation, SESSION_WORKSPACE_CONTEXT_ID,
    SessionWorkspaceContextContract, SessionWorkspaceContextHandle,
};

struct WorkspaceState {
    work_dir: PathBuf,
    additional_dirs: Vec<PathBuf>,
}

pub struct SessionWorkspaceContextService {
    state: RwLock<WorkspaceState>,
}

impl SessionWorkspaceContextService {
    // Original: SessionWorkspaceContextService.constructor().
    pub fn new(context: &SessionContext) -> std::io::Result<Self> {
        Ok(Self {
            state: RwLock::new(WorkspaceState {
                work_dir: resolve_from_process(&context.cwd)?,
                additional_dirs: Vec::new(),
            }),
        })
    }

    fn read_state(&self) -> std::sync::RwLockReadGuard<'_, WorkspaceState> {
        self.state
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn write_state(&self) -> std::sync::RwLockWriteGuard<'_, WorkspaceState> {
        self.state
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl SessionWorkspaceContextContract for SessionWorkspaceContextService {
    fn work_dir(&self) -> PathBuf {
        self.read_state().work_dir.clone()
    }

    fn additional_dirs(&self) -> Vec<PathBuf> {
        self.read_state().additional_dirs.clone()
    }

    // Original: setWorkDir().
    fn set_work_dir(&self, work_dir: &str) -> std::io::Result<()> {
        self.write_state().work_dir = resolve_from_process(work_dir)?;
        Ok(())
    }

    // Original: setAdditionalDirs().
    fn set_additional_dirs(&self, dirs: &[String]) -> std::io::Result<()> {
        let mut resolved = Vec::new();
        for dir in dirs {
            let dir = resolve_from_process(dir)?;
            if !resolved.contains(&dir) {
                resolved.push(dir);
            }
        }
        self.write_state().additional_dirs = resolved;
        Ok(())
    }

    fn resolve(&self, relative: &str) -> PathBuf {
        let state = self.read_state();
        if Path::new(relative).is_absolute() {
            normalize_lexical(Path::new(relative))
        } else {
            normalize_lexical(&state.work_dir.join(relative))
        }
    }

    // Original: isWithin(). This intentionally remains a lexical boundary and
    // retains the source's `relative.startsWith("..")` behavior.
    fn is_within(&self, absolute_path: &str) -> bool {
        let target = resolve_from_process(absolute_path)
            .unwrap_or_else(|_| normalize_lexical(Path::new(absolute_path)));
        let state = self.read_state();
        is_within_directory(&target, &state.work_dir)
            || state
                .additional_dirs
                .iter()
                .any(|directory| is_within_directory(&target, directory))
    }

    // Original: assertAllowed().
    fn assert_allowed(
        &self,
        absolute_path: &str,
        operation: PathAccessOperation,
    ) -> Result<PathBuf, PathAccessError> {
        let target = self.resolve(absolute_path);
        if self.is_within(&target.to_string_lossy()) {
            Ok(target)
        } else {
            Err(PathAccessError {
                path: target,
                operation,
            })
        }
    }

    fn add_additional_dir(&self, dir: &str) -> std::io::Result<()> {
        let directory = resolve_from_process(dir)?;
        let mut state = self.write_state();
        if !state.additional_dirs.contains(&directory) {
            state.additional_dirs.push(directory);
        }
        Ok(())
    }

    fn remove_additional_dir(&self, dir: &str) -> std::io::Result<()> {
        let directory = resolve_from_process(dir)?;
        self.write_state()
            .additional_dirs
            .retain(|existing| existing != &directory);
        Ok(())
    }
}

fn resolve_from_process(path: &str) -> std::io::Result<PathBuf> {
    let path = Path::new(path);
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    Ok(normalize_lexical(&absolute))
}

fn normalize_lexical(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if matches!(
                    normalized.components().next_back(),
                    Some(Component::Normal(_))
                ) {
                    normalized.pop();
                }
            }
            Component::Prefix(_) | Component::RootDir | Component::Normal(_) => {
                normalized.push(component.as_os_str());
            }
        }
    }
    normalized
}

fn is_within_directory(target: &Path, directory: &Path) -> bool {
    if target == directory {
        return true;
    }
    let Ok(relative) = target.strip_prefix(directory) else {
        return false;
    };
    let relative = relative.to_string_lossy();
    !relative.is_empty() && !relative.starts_with("..") && !relative.starts_with(['/', '\\'])
}

pub fn register_session_workspace_context() {
    register_scoped_service(
        LifecycleScope::Session,
        SESSION_WORKSPACE_CONTEXT_ID,
        SyncDescriptor::new(|accessor| {
            let context = accessor.get(SESSION_CONTEXT_ID)?;
            let service = SessionWorkspaceContextService::new(&context)
                .map_err(|error| DiError::Factory(error.to_string()))?;
            let service: Arc<dyn SessionWorkspaceContextContract> = Arc::new(service);
            Ok(SessionWorkspaceContextHandle(service))
        }),
        InstantiationType::Eager,
        "workspaceContext",
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::session_context::{SessionContextInput, make_session_context};

    fn context(cwd: &str) -> SessionContext {
        make_session_context(SessionContextInput {
            session_id: "s".into(),
            workspace_id: "w".into(),
            session_dir: "/sessions/s".into(),
            session_scope: "sessions/s".into(),
            cwd: cwd.into(),
            meta_scope: None,
        })
    }

    #[test]
    fn resolves_mutates_and_deduplicates_workspace_roots_lexically() {
        let root = std::env::temp_dir().join("workspace-context-repo");
        let nested = root.join("nested").join("..");
        let service =
            SessionWorkspaceContextService::new(&context(&nested.to_string_lossy())).unwrap();
        assert_eq!(service.work_dir(), root);
        assert_eq!(service.resolve("src/../README.md"), root.join("README.md"));
        let absolute_outside = std::env::temp_dir().join("workspace-context-outside");
        assert_eq!(
            service.resolve(&absolute_outside.to_string_lossy()),
            absolute_outside
        );
        let shared = std::env::temp_dir().join("workspace-context-shared");
        let other = std::env::temp_dir().join("workspace-context-other");
        service
            .set_additional_dirs(&[
                shared.to_string_lossy().into_owned(),
                shared.join(".").to_string_lossy().into_owned(),
            ])
            .unwrap();
        service
            .add_additional_dir(&other.to_string_lossy())
            .unwrap();
        service
            .add_additional_dir(&other.to_string_lossy())
            .unwrap();
        assert_eq!(service.additional_dirs(), [shared.clone(), other.clone()]);
        service
            .remove_additional_dir(&shared.to_string_lossy())
            .unwrap();
        assert_eq!(service.additional_dirs(), [other]);
    }

    #[test]
    fn enforces_workspace_and_additional_directory_boundaries() {
        let root = std::env::temp_dir().join("workspace-context-boundary-repo");
        let shared = std::env::temp_dir().join("workspace-context-boundary-shared");
        let outside = std::env::temp_dir().join("workspace-context-boundary-outside");
        let sibling = std::env::temp_dir().join("workspace-context-boundary-repository");
        let service =
            SessionWorkspaceContextService::new(&context(&root.to_string_lossy())).unwrap();
        service
            .add_additional_dir(&shared.to_string_lossy())
            .unwrap();
        assert!(service.is_within(&root.to_string_lossy()));
        assert!(service.is_within(&root.join("src/lib.rs").to_string_lossy()));
        assert!(service.is_within(&shared.join("file").to_string_lossy()));
        assert!(!service.is_within(&sibling.join("file").to_string_lossy()));
        assert!(!service.is_within(&outside.to_string_lossy()));
        assert_eq!(
            service
                .assert_allowed("src/lib.rs", PathAccessOperation::Read)
                .unwrap(),
            root.join("src/lib.rs")
        );
        let error = service
            .assert_allowed(&outside.to_string_lossy(), PathAccessOperation::Execute)
            .unwrap_err();
        assert_eq!(error.path, outside);
        assert_eq!(error.operation, PathAccessOperation::Execute);
    }
}
