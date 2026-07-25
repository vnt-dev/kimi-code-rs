use std::io;
use std::path::{Path, PathBuf};

use thiserror::Error;
use tokio::fs::{self, File, OpenOptions};
use tokio::io::AsyncWriteExt;
use uuid::Uuid;

#[derive(Debug, Error)]
pub enum PrivateFileError {
    #[error(
        "private file {} is too permissive (mode {mode:03o}); expected 0600",
        file_path.display()
    )]
    TooPermissive { file_path: PathBuf, mode: u32 },
    #[error(transparent)]
    Io(#[from] io::Error),
}

impl PrivateFileError {
    pub fn io_kind(&self) -> Option<io::ErrorKind> {
        match self {
            Self::Io(error) => Some(error.kind()),
            Self::TooPermissive { .. } => None,
        }
    }
}

// Original:
//   packages/kap-server/src/services/auth/privateFiles.ts
//   writePrivateFile()
//
// Rust adaptation:
//   Tokio is used for waiting file operations. The temporary file is flushed
//   before replacement and is removed on every error path.
pub async fn write_private_file(
    file_path: impl AsRef<Path>,
    data: impl AsRef<[u8]>,
) -> Result<(), PrivateFileError> {
    let file_path = file_path.as_ref();
    let directory = file_path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(directory).await?;
    set_private_directory_permissions(directory).await?;

    let mut temporary = file_path.as_os_str().to_owned();
    temporary.push(format!(
        ".tmp.{}",
        &Uuid::new_v4().simple().to_string()[..16]
    ));
    let temporary = PathBuf::from(temporary);

    let result = async {
        let mut options = OpenOptions::new();
        options.create_new(true).write(true);
        set_private_file_open_mode(&mut options);
        let mut file = options.open(&temporary).await?;
        set_private_file_permissions(&file).await?;
        file.write_all(data.as_ref()).await?;
        file.sync_all().await?;
        drop(file);
        replace_file(&temporary, file_path).await
    }
    .await;

    if result.is_err() {
        let _ = fs::remove_file(&temporary).await;
    }
    result.map_err(PrivateFileError::Io)
}

pub async fn read_private_file(file_path: impl AsRef<Path>) -> Result<Vec<u8>, PrivateFileError> {
    let file_path = file_path.as_ref();
    let metadata = fs::metadata(file_path).await?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = metadata.permissions().mode() & 0o777;
        if mode & 0o077 != 0 {
            return Err(PrivateFileError::TooPermissive {
                file_path: file_path.to_owned(),
                mode,
            });
        }
    }
    #[cfg(not(unix))]
    let _ = metadata;
    Ok(fs::read(file_path).await?)
}

#[cfg(unix)]
fn set_private_file_open_mode(options: &mut OpenOptions) {
    use std::os::unix::fs::OpenOptionsExt;
    options.mode(0o600);
}

#[cfg(not(unix))]
fn set_private_file_open_mode(_options: &mut OpenOptions) {}

#[cfg(unix)]
async fn set_private_file_permissions(file: &File) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    file.set_permissions(std::fs::Permissions::from_mode(0o600))
        .await
}

#[cfg(not(unix))]
async fn set_private_file_permissions(_file: &File) -> io::Result<()> {
    Ok(())
}

#[cfg(unix)]
async fn set_private_directory_permissions(directory: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(directory, std::fs::Permissions::from_mode(0o700)).await
}

#[cfg(not(unix))]
async fn set_private_directory_permissions(_directory: &Path) -> io::Result<()> {
    Ok(())
}

async fn replace_file(from: &Path, to: &Path) -> io::Result<()> {
    #[cfg(windows)]
    if let Err(error) = fs::remove_file(to).await
        && error.kind() != io::ErrorKind::NotFound
    {
        return Err(error);
    }
    fs::rename(from, to).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn round_trips_text_and_binary_content() {
        let directory = tempfile::tempdir().unwrap();
        let text_path = directory.path().join("nested/secret");
        write_private_file(&text_path, "s3cr3t-value")
            .await
            .unwrap();
        assert_eq!(
            read_private_file(&text_path).await.unwrap(),
            b"s3cr3t-value"
        );

        let binary_path = directory.path().join("binary");
        let binary = [0, 1, 2, 254, 255];
        write_private_file(&binary_path, binary).await.unwrap();
        assert_eq!(read_private_file(&binary_path).await.unwrap(), binary);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn enforces_private_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("secret");
        write_private_file(&path, "value").await.unwrap();
        assert_eq!(
            fs::metadata(&path).await.unwrap().permissions().mode() & 0o777,
            0o600
        );

        fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644))
            .await
            .unwrap();
        assert!(matches!(
            read_private_file(&path).await,
            Err(PrivateFileError::TooPermissive { mode: 0o644, .. })
        ));
    }
}
