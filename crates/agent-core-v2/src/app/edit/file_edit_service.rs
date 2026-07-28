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
            Err(error) => {
                if error.code() == OS_FS_IS_DIRECTORY
                    || self
                        .fs
                        .stat(Path::new(&input.path))
                        .await
                        .is_ok_and(|stat| stat.is_directory)
                {
                    return not_a_file(&input.display_path);
                }
                return fs_error(&input.display_path, &error);
            }
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
        let (raw_content, count) = match result {
            EditApplyResult::Ok { raw_content, count } => (raw_content, count),
            EditApplyResult::Err { error } => return FileEditResult::Err { error },
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
        not_a_file(display_path)
    } else {
        FileEditResult::Err {
            error: error.to_string(),
        }
    }
}

fn not_a_file(display_path: &str) -> FileEditResult {
    FileEditResult::Err {
        error: format!("{display_path} is not a file."),
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
    use std::{path::PathBuf, sync::Arc};

    use super::*;

    use crate::os::{
        backends::node_local::host_fs_service::HostFileSystem,
        interface::host_file_system::HostFileSystemService,
    };

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!("kimi-edit-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn join(&self, path: &str) -> PathBuf {
            self.0.join(path)
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn service() -> FileEditService {
        let fs: Arc<dyn HostFileSystemService> = Arc::new(HostFileSystem);
        FileEditService::new(HostFileSystemServiceHandle(fs))
    }

    fn input(path: &Path, old_string: &str, new_string: &str) -> FileEditInput {
        FileEditInput {
            path: path.to_string_lossy().into_owned(),
            display_path: "sample.txt".into(),
            old_string: old_string.into(),
            new_string: new_string.into(),
            replace_all: false,
        }
    }

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

    #[tokio::test]
    async fn edits_literal_replacement_text_and_unicode() {
        let directory = TestDirectory::new();
        let path = directory.join("sample.txt");
        std::fs::write(&path, "Hello 世界! alpha beta gamma").unwrap();

        let result = service()
            .edit(input(&path, "世界! alpha beta", "地球! $& $$ $` $'"))
            .await;

        assert_eq!(result, FileEditResult::Ok { count: 1 });
        assert_eq!(
            std::fs::read_to_string(path).unwrap(),
            "Hello 地球! $& $$ $` $' gamma"
        );
    }

    #[tokio::test]
    async fn normalizes_only_pure_crlf_files_for_matching() {
        let directory = TestDirectory::new();
        let crlf = directory.join("crlf.txt");
        std::fs::write(&crlf, "alpha\r\nbeta\r\ngamma\r\n").unwrap();

        let result = service()
            .edit(input(&crlf, "alpha\nbeta", "one\r\ntwo"))
            .await;
        assert_eq!(result, FileEditResult::Ok { count: 1 });
        assert_eq!(
            std::fs::read_to_string(&crlf).unwrap(),
            "one\r\ntwo\r\ngamma\r\n"
        );

        let mixed = directory.join("mixed.txt");
        let mixed_content = "alpha\r\nbeta\ngamma\r\n";
        std::fs::write(&mixed, mixed_content).unwrap();
        let result = service()
            .edit(input(&mixed, "alpha\nbeta", "one\ntwo"))
            .await;
        assert!(matches!(result, FileEditResult::Err { error } if error.contains("not found")));
        assert_eq!(std::fs::read_to_string(mixed).unwrap(), mixed_content);
    }

    #[tokio::test]
    async fn replace_all_and_ambiguous_single_edit_follow_source_rules() {
        let directory = TestDirectory::new();
        let path = directory.join("sample.txt");
        std::fs::write(&path, "a b a").unwrap();

        let ambiguous = service().edit(input(&path, "a", "x")).await;
        assert!(
            matches!(ambiguous, FileEditResult::Err { error } if error.contains("not unique") && error.contains("replace_all=true"))
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "a b a");

        let mut replace_all = input(&path, "a", "$&");
        replace_all.replace_all = true;
        assert_eq!(
            service().edit(replace_all).await,
            FileEditResult::Ok { count: 2 }
        );
        assert_eq!(std::fs::read_to_string(path).unwrap(), "$& b $&");
    }

    #[tokio::test]
    async fn rejects_non_utf8_without_changing_bytes() {
        let directory = TestDirectory::new();
        let path = directory.join("binary.txt");
        let original = [0x68, 0x69, 0x20, 0xff, 0x0a, 0x66, 0x6f, 0x6f];
        std::fs::write(&path, original).unwrap();

        let result = service().edit(input(&path, "foo", "bar")).await;

        assert!(matches!(result, FileEditResult::Err { .. }));
        assert_eq!(std::fs::read(path).unwrap(), original);
    }

    #[tokio::test]
    async fn maps_real_directory_reads_to_not_a_file() {
        let directory = TestDirectory::new();
        let result = service().edit(input(&directory.0, "old", "new")).await;

        assert_eq!(
            result,
            FileEditResult::Err {
                error: "sample.txt is not a file.".into()
            }
        );
    }
}
