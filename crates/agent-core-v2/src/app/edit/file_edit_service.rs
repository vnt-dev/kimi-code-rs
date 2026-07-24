//! Host-filesystem-backed edit service.
//! Original: `packages/agent-core-v2/src/app/edit/fileEditService.ts`.
use super::{
    EditApplyInput, EditApplyResult, EditService, FILE_EDIT_SERVICE_ID, FileEditInput,
    FileEditResult, FileEditServiceContract, FileEditServiceHandle, TextModel,
};
use crate::{
    _base::di::{
        descriptors::SyncDescriptor,
        instantiation::ServicesAccessorExt,
        scope::{InstantiationType, LifecycleScope, register_scoped_service},
    },
    os::interface::{
        host_file_system::{
            HOST_FILE_SYSTEM_SERVICE_ID, HostFileSystemServiceHandle, ReadTextOptions,
        },
        host_fs_errors::OS_FS_IS_DIRECTORY,
    },
};
use async_trait::async_trait;
use std::{path::Path, sync::Arc};
pub struct FileEditService {
    fs: HostFileSystemServiceHandle,
    editor: EditService,
}
impl FileEditService {
    pub fn new(fs: HostFileSystemServiceHandle) -> Self {
        Self {
            fs,
            editor: EditService,
        }
    }
}
#[async_trait]
impl FileEditServiceContract for FileEditService {
    async fn edit(&self, input: FileEditInput) -> FileEditResult {
        let raw = match self
            .fs
            .read_text(Path::new(&input.path), Some(ReadTextOptions::default()))
            .await
        {
            Ok(raw) => raw,
            Err(error) => return fs_error(&input.display_path, &error),
        };
        let result = self.editor.apply(
            &TextModel::new(&raw),
            &EditApplyInput {
                path: input.display_path.clone(),
                old_string: input.old_string,
                new_string: input.new_string,
                replace_all: input.replace_all,
            },
        );
        let EditApplyResult::Ok { raw_content, count } = result else {
            let EditApplyResult::Err { error } = result else {
                unreachable!()
            };
            return FileEditResult::Err { error };
        };
        match self
            .fs
            .write_text(Path::new(&input.path), &raw_content)
            .await
        {
            Ok(()) => FileEditResult::Ok { count },
            Err(error) => fs_error(&input.display_path, &error),
        }
    }
}
fn fs_error(
    display_path: &str,
    error: &crate::os::interface::host_fs_errors::HostFsError,
) -> FileEditResult {
    if error.code() == OS_FS_IS_DIRECTORY {
        FileEditResult::Err {
            error: format!("{display_path} is not a file."),
        }
    } else {
        FileEditResult::Err {
            error: error.to_string(),
        }
    }
}
pub fn register_file_edit_service() {
    register_scoped_service(
        LifecycleScope::App,
        FILE_EDIT_SERVICE_ID,
        SyncDescriptor::new(|accessor| {
            let service: Arc<dyn FileEditServiceContract> = Arc::new(FileEditService::new(
                (*accessor.get(HOST_FILE_SYSTEM_SERVICE_ID)?).clone(),
            ));
            Ok(FileEditServiceHandle(service))
        }),
        InstantiationType::Eager,
        "edit",
    );
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn directory_error_is_domain_neutral() {
        let error = crate::os::interface::host_fs_errors::HostFsError::with_options(
            OS_FS_IS_DIRECTORY,
            "directory",
            Default::default(),
        );
        assert_eq!(
            fs_error("dir", &error),
            FileEditResult::Err {
                error: "dir is not a file.".into()
            }
        );
    }
}
