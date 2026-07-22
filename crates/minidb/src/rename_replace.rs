use std::{io, path::Path, time::Duration};

#[derive(Debug, Clone, Copy)]
pub struct RenameReplaceOptions {
    pub retries: usize,
    pub base_delay: Duration,
}

impl Default for RenameReplaceOptions {
    fn default() -> Self {
        Self {
            retries: 100,
            base_delay: Duration::from_millis(20),
        }
    }
}

// Original: packages/minidb/src/rename-replace.ts, renameReplace().
pub async fn rename_replace(
    source: impl AsRef<Path>,
    destination: impl AsRef<Path>,
    options: RenameReplaceOptions,
) -> io::Result<()> {
    let source = source.as_ref();
    let destination = destination.as_ref();
    #[cfg(not(windows))]
    {
        let _ = options;
        tokio::fs::rename(source, destination).await
    }
    #[cfg(windows)]
    {
        for attempt in 0..=options.retries {
            match tokio::fs::rename(source, destination).await {
                Ok(()) => return Ok(()),
                Err(error)
                    if error.kind() == io::ErrorKind::PermissionDenied
                        && attempt < options.retries =>
                {
                    let jitter = (attempt as u64).wrapping_mul(17) % 30;
                    tokio::time::sleep(options.base_delay + Duration::from_millis(jitter)).await;
                }
                Err(error) => return Err(error),
            }
        }
        unreachable!("retry loop always returns")
    }
}
