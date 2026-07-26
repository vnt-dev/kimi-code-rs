//! Tokio-backed local host filesystem.
//!
//! Original: `packages/agent-core-v2/src/os/backends/node-local/hostFsService.ts`.

use std::{
    collections::VecDeque,
    error::Error,
    path::{Path, PathBuf},
    sync::Arc,
    time::UNIX_EPOCH,
};

use async_trait::async_trait;
use futures_util::stream;
use tokio::{
    fs::{self, File, OpenOptions},
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
};

use crate::{
    _base::{
        di::{
            descriptors::SyncDescriptor,
            scope::{InstantiationType, LifecycleScope, register_scoped_service},
        },
        exec_env::decode_text::{TextDecodeErrors, TextEncoding, decode_text_with_errors},
    },
    os::interface::{
        host_file_system::{
            HOST_FILE_SYSTEM_SERVICE_ID, HostDirEntry, HostFileStat, HostFileSystemService,
            HostFileSystemServiceHandle, HostLineStream, ReadTextOptions,
        },
        host_fs_errors::{HostFsError, OS_FS_ALREADY_EXISTS, to_host_fs_error},
    },
};

#[derive(Default)]
pub struct HostFileSystem;

/// Original: `hostFsService.ts`, App-scope eager registration.
pub fn register_local_host_file_system_service() {
    register_scoped_service(
        LifecycleScope::App,
        HOST_FILE_SYSTEM_SERVICE_ID,
        SyncDescriptor::new(|_| {
            let service: Arc<dyn HostFileSystemService> = Arc::new(HostFileSystem);
            Ok(HostFileSystemServiceHandle(service))
        }),
        InstantiationType::Eager,
        "hostFs",
    );
}

#[async_trait]
impl HostFileSystemService for HostFileSystem {
    async fn read_text(
        &self,
        path: &Path,
        options: Option<ReadTextOptions>,
    ) -> Result<String, HostFsError> {
        let data = fs::read(path)
            .await
            .map_err(|error| host_error(error, path, "read"))?;
        let options = options.unwrap_or_default();
        decode_text_with_errors(&data, options.encoding, options.errors, false)
            .map_err(|error| host_error(error, path, "read"))
    }

    async fn write_text(&self, path: &Path, data: &str) -> Result<(), HostFsError> {
        fs::write(path, data)
            .await
            .map_err(|error| host_error(error, path, "write"))
    }

    async fn append_text(&self, path: &Path, data: &str) -> Result<(), HostFsError> {
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .await
            .map_err(|error| host_error(error, path, "append"))?;
        file.write_all(data.as_bytes())
            .await
            .map_err(|error| host_error(error, path, "append"))
    }

    async fn read_bytes(&self, path: &Path, count: Option<usize>) -> Result<Vec<u8>, HostFsError> {
        if let Some(count) = count {
            let mut file = File::open(path)
                .await
                .map_err(|error| host_error(error, path, "read"))?;
            let mut data = vec![0; count];
            let read = file
                .read(&mut data)
                .await
                .map_err(|error| host_error(error, path, "read"))?;
            data.truncate(read);
            Ok(data)
        } else {
            fs::read(path)
                .await
                .map_err(|error| host_error(error, path, "read"))
        }
    }

    async fn write_bytes(&self, path: &Path, data: &[u8]) -> Result<(), HostFsError> {
        fs::write(path, data)
            .await
            .map_err(|error| host_error(error, path, "write"))
    }

    fn read_lines(&self, path: &Path, options: Option<ReadTextOptions>) -> HostLineStream {
        let state = LineState::Opening(path.to_path_buf(), options.unwrap_or_default());
        Box::pin(stream::try_unfold(state, |mut state| async move {
            loop {
                match state {
                    LineState::Opening(path, options) if options.encoding == TextEncoding::Utf8 => {
                        let file = File::open(&path)
                            .await
                            .map_err(|error| host_error(error, &path, "read"))?;
                        state = LineState::Utf8 {
                            path,
                            reader: BufReader::new(file),
                            errors: options.errors,
                            first: true,
                        };
                    }
                    LineState::Opening(path, options) => {
                        let data = fs::read(&path)
                            .await
                            .map_err(|error| host_error(error, &path, "read"))?;
                        let text =
                            decode_text_with_errors(&data, options.encoding, options.errors, false)
                                .map_err(|error| host_error(error, &path, "read"))?;
                        state = LineState::Buffered(split_lines(text));
                    }
                    LineState::Utf8 {
                        path,
                        mut reader,
                        errors,
                        first,
                    } => {
                        let mut line = Vec::new();
                        let count = reader
                            .read_until(b'\n', &mut line)
                            .await
                            .map_err(|error| host_error(error, &path, "read"))?;
                        if count == 0 {
                            return Ok(None);
                        }
                        let value =
                            decode_text_with_errors(&line, TextEncoding::Utf8, errors, !first)
                                .map_err(|error| host_error(error, &path, "read"))?;
                        return Ok(Some((
                            value,
                            LineState::Utf8 {
                                path,
                                reader,
                                errors,
                                first: false,
                            },
                        )));
                    }
                    LineState::Buffered(mut lines) => {
                        let Some(line) = lines.pop_front() else {
                            return Ok(None);
                        };
                        return Ok(Some((line, LineState::Buffered(lines))));
                    }
                }
            }
        }))
    }

    async fn create_exclusive(&self, path: &Path, data: &[u8]) -> Result<bool, HostFsError> {
        let mut file = match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .await
        {
            Ok(file) => file,
            Err(error) => {
                let error = host_error(error, path, "create");
                if error.code() == OS_FS_ALREADY_EXISTS {
                    return Ok(false);
                }
                return Err(error);
            }
        };
        file.write_all(data)
            .await
            .map_err(|error| host_error(error, path, "create"))?;
        file.sync_all()
            .await
            .map_err(|error| host_error(error, path, "create"))?;
        Ok(true)
    }

    async fn stat(&self, path: &Path) -> Result<HostFileStat, HostFsError> {
        fs::metadata(path)
            .await
            .map(|metadata| metadata_to_stat(&metadata))
            .map_err(|error| host_error(error, path, "stat"))
    }

    async fn lstat(&self, path: &Path) -> Result<HostFileStat, HostFsError> {
        fs::symlink_metadata(path)
            .await
            .map(|metadata| metadata_to_stat(&metadata))
            .map_err(|error| host_error(error, path, "lstat"))
    }

    async fn read_dir(&self, path: &Path) -> Result<Vec<HostDirEntry>, HostFsError> {
        let mut source = fs::read_dir(path)
            .await
            .map_err(|error| host_error(error, path, "readdir"))?;
        let mut entries = Vec::new();
        while let Some(entry) = source
            .next_entry()
            .await
            .map_err(|error| host_error(error, path, "readdir"))?
        {
            let file_type = entry
                .file_type()
                .await
                .map_err(|error| host_error(error, path, "readdir"))?;
            entries.push(HostDirEntry {
                name: entry.file_name().to_string_lossy().into_owned(),
                is_file: file_type.is_file(),
                is_directory: file_type.is_dir(),
                is_symbolic_link: file_type.is_symlink(),
            });
        }
        Ok(entries)
    }

    async fn create_dir(&self, path: &Path, recursive: bool) -> Result<(), HostFsError> {
        let result = if recursive {
            fs::create_dir_all(path).await
        } else {
            fs::create_dir(path).await
        };
        result.map_err(|error| host_error(error, path, "mkdir"))
    }

    async fn remove(&self, path: &Path) -> Result<(), HostFsError> {
        let metadata = match fs::symlink_metadata(path).await {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(host_error(error, path, "remove")),
        };
        let result = if metadata.is_dir() {
            fs::remove_dir_all(path).await
        } else {
            fs::remove_file(path).await
        };
        result.map_err(|error| host_error(error, path, "remove"))
    }

    async fn real_path(&self, path: &Path) -> Result<String, HostFsError> {
        fs::canonicalize(path)
            .await
            .map(|path| path.to_string_lossy().into_owned())
            .map_err(|error| host_error(error, path, "realpath"))
    }
}

enum LineState {
    Opening(PathBuf, ReadTextOptions),
    Utf8 {
        path: PathBuf,
        reader: BufReader<File>,
        errors: TextDecodeErrors,
        first: bool,
    },
    Buffered(VecDeque<String>),
}

fn split_lines(text: String) -> VecDeque<String> {
    let mut lines = VecDeque::new();
    let mut start = 0;
    for (index, character) in text.char_indices() {
        if character == '\n' {
            lines.push_back(text[start..=index].to_owned());
            start = index + 1;
        }
    }
    if start < text.len() {
        lines.push_back(text[start..].to_owned());
    }
    lines
}

fn host_error(
    error: impl Error + Send + Sync + 'static,
    path: &Path,
    operation: &str,
) -> HostFsError {
    to_host_fs_error(Box::new(error), &path.to_string_lossy(), operation)
}

fn metadata_to_stat(metadata: &std::fs::Metadata) -> HostFileStat {
    HostFileStat {
        is_file: metadata.is_file(),
        is_directory: metadata.is_dir(),
        is_symbolic_link: metadata.file_type().is_symlink(),
        size: metadata.len(),
        modified_millis: metadata
            .modified()
            .ok()
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .map(|duration| duration.as_secs_f64() * 1000.0),
        inode: inode(metadata),
    }
}

#[cfg(unix)]
fn inode(metadata: &std::fs::Metadata) -> Option<u64> {
    use std::os::unix::fs::MetadataExt;
    Some(metadata.ino())
}

#[cfg(not(unix))]
fn inode(_: &std::fs::Metadata) -> Option<u64> {
    None
}

#[cfg(test)]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use futures_util::TryStreamExt;

    use super::*;
    use crate::_base::di::{
        lifecycle::Disposable,
        scope::{Scope, ScopeOptions},
    };

    fn temp_dir() -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("kimi-hostfs-{}-{nonce}", std::process::id()))
    }

    #[test]
    fn app_scope_registration_resolves_host_file_system() {
        register_local_host_file_system_service();
        let app = Scope::create_app(ScopeOptions::default());
        app.get(HOST_FILE_SYSTEM_SERVICE_ID).unwrap();
        app.dispose().unwrap();
    }

    #[tokio::test]
    async fn round_trips_bytes_text_lines_directories_and_exclusive_files() {
        let root = temp_dir();
        let fs_service = HostFileSystem;
        fs_service.create_dir(&root, false).await.unwrap();
        let file = root.join("file.txt");
        fs_service.write_text(&file, "a\nb").await.unwrap();
        fs_service.append_text(&file, "\nc").await.unwrap();
        assert_eq!(fs_service.read_text(&file, None).await.unwrap(), "a\nb\nc");
        assert_eq!(fs_service.read_bytes(&file, Some(2)).await.unwrap(), b"a\n");
        assert_eq!(
            fs_service
                .read_lines(&file, None)
                .try_collect::<Vec<_>>()
                .await
                .unwrap(),
            ["a\n", "b\n", "c"]
        );
        let exclusive = root.join("exclusive");
        assert!(
            fs_service
                .create_exclusive(&exclusive, b"one")
                .await
                .unwrap()
        );
        assert!(
            !fs_service
                .create_exclusive(&exclusive, b"two")
                .await
                .unwrap()
        );
        assert_eq!(fs_service.read_dir(&root).await.unwrap().len(), 2);
        fs_service.remove(&root).await.unwrap();
        fs_service.remove(&root).await.unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn stat_follows_symlinks_while_lstat_observes_the_link() {
        use std::os::unix::fs::symlink;

        let root = temp_dir();
        fs::create_dir(&root).await.unwrap();
        let target = root.join("target.txt");
        fs::write(&target, "hello").await.unwrap();
        let link = root.join("link.txt");
        symlink(&target, &link).unwrap();
        let fs_service = HostFileSystem;
        assert!(fs_service.stat(&link).await.unwrap().is_file);
        assert!(!fs_service.stat(&link).await.unwrap().is_symbolic_link);
        assert!(fs_service.lstat(&link).await.unwrap().is_symbolic_link);
        fs_service.remove(&root).await.unwrap();
    }
}
