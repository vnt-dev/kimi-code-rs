use std::{
    collections::VecDeque,
    io::Write,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use futures_util::future::BoxFuture;
use tokio::{
    fs::{self, OpenOptions},
    io::AsyncWriteExt,
    sync::Mutex as AsyncMutex,
};

use super::{
    contract::{LogEntry, LogWriter},
    formatter::{FormatOptions, format_entry},
};
use crate::_base::utils::fs::sync_dir;

pub const PENDING_MAX: usize = 1000;
const STDERR_NOTICE_INTERVAL: Duration = Duration::from_secs(30);

#[derive(Clone, Debug)]
pub struct RotatingFileWriterOptions {
    pub path: PathBuf,
    pub max_bytes: u64,
    pub files: usize,
}

struct PendingState {
    lines: VecDeque<String>,
    dropped: usize,
    closed: bool,
    last_stderr_notice: Option<Instant>,
}

struct IoState {
    current_bytes: Option<u64>,
    directory_synced: bool,
}

struct RotatingInner {
    options: RotatingFileWriterOptions,
    pending: Mutex<PendingState>,
    io: AsyncMutex<IoState>,
    drain_scheduled: AtomicBool,
}

#[derive(Clone)]
pub struct RotatingFileWriter {
    inner: Arc<RotatingInner>,
}

impl RotatingFileWriter {
    pub fn new(options: RotatingFileWriterOptions) -> Self {
        Self {
            inner: Arc::new(RotatingInner {
                options,
                pending: Mutex::new(PendingState {
                    lines: VecDeque::new(),
                    dropped: 0,
                    closed: false,
                    last_stderr_notice: None,
                }),
                io: AsyncMutex::new(IoState {
                    current_bytes: None,
                    directory_synced: false,
                }),
                drain_scheduled: AtomicBool::new(false),
            }),
        }
    }

    // Original: RotatingFileWriter.enqueue(); oldest entries are dropped at the cap.
    pub fn enqueue(&self, line: impl Into<String>) {
        {
            let mut pending = self.inner.pending.lock().unwrap();
            if pending.closed {
                return;
            }
            if pending.lines.len() >= PENDING_MAX {
                pending.lines.pop_front();
                pending.dropped += 1;
            }
            pending.lines.push_back(line.into());
        }
        self.schedule_drain();
    }

    fn schedule_drain(&self) {
        if self.inner.drain_scheduled.swap(true, Ordering::AcqRel) {
            return;
        }
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            self.inner.drain_scheduled.store(false, Ordering::Release);
            return;
        };
        let writer = self.clone();
        runtime.spawn(async move {
            let _ = writer.drain().await;
            writer.inner.drain_scheduled.store(false, Ordering::Release);
        });
    }

    pub async fn flush(&self) -> bool {
        self.drain().await
    }

    pub async fn close(&self) {
        {
            let mut pending = self.inner.pending.lock().unwrap();
            if pending.closed {
                return;
            }
            pending.closed = true;
        }
        let _ = self.flush().await;
    }

    pub fn flush_sync(&self) {
        let (lines, notice) = {
            let mut pending = self.inner.pending.lock().unwrap();
            if pending.closed || pending.lines.is_empty() {
                return;
            }
            let lines = pending.lines.drain(..).collect::<String>();
            let notice = take_dropped_notice(&mut pending);
            (lines, notice)
        };
        let result = (|| -> std::io::Result<()> {
            let directory = parent_directory(&self.inner.options.path);
            std::fs::create_dir_all(directory)?;
            let mut file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&self.inner.options.path)?;
            file.write_all(lines.as_bytes())?;
            file.write_all(notice.as_bytes())
        })();
        if let Err(error) = result {
            self.note_failure(&error);
        }
    }

    async fn drain(&self) -> bool {
        let mut io = self.inner.io.lock().await;
        let lines = {
            let mut pending = self.inner.pending.lock().unwrap();
            if pending.lines.is_empty() {
                return true;
            }
            let notice = take_dropped_notice(&mut pending);
            let mut lines = pending.lines.drain(..).collect::<Vec<_>>();
            if !notice.is_empty() {
                lines.push(notice);
            }
            lines
        };
        let result = self.append_lines(&lines, &mut io).await;
        match result {
            Ok(()) => true,
            Err(error) => {
                self.note_failure(&error);
                self.restore_pending(lines);
                false
            }
        }
    }

    async fn append_lines(&self, lines: &[String], io: &mut IoState) -> std::io::Result<()> {
        let directory = parent_directory(&self.inner.options.path);
        fs::create_dir_all(directory).await?;
        if io.current_bytes.is_none() {
            io.current_bytes = Some(stat_size(&self.inner.options.path).await?);
        }
        let mut chunk = String::new();
        let mut chunk_bytes = 0_u64;
        for line in lines {
            let line_bytes = line.len() as u64;
            let current = io.current_bytes.unwrap_or(0);
            if chunk_bytes > 0
                && (chunk_bytes + line_bytes > self.inner.options.max_bytes
                    || current + chunk_bytes + line_bytes > self.inner.options.max_bytes)
            {
                self.append_chunk(&chunk, io).await?;
                chunk.clear();
                chunk_bytes = 0;
            }
            if chunk_bytes == 0
                && io.current_bytes.unwrap_or(0) > 0
                && io.current_bytes.unwrap_or(0) + line_bytes > self.inner.options.max_bytes
            {
                self.rotate(io).await?;
            }
            chunk.push_str(line);
            chunk_bytes += line_bytes;
        }
        if chunk_bytes > 0 {
            self.append_chunk(&chunk, io).await?;
        }
        if !io.directory_synced {
            sync_dir(directory).await?;
            io.directory_synced = true;
        }
        Ok(())
    }

    async fn append_chunk(&self, chunk: &str, io: &mut IoState) -> std::io::Result<()> {
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.inner.options.path)
            .await?;
        file.write_all(chunk.as_bytes()).await?;
        file.sync_all().await?;
        drop(file);
        *io.current_bytes.get_or_insert(0) += chunk.len() as u64;
        if io.current_bytes.unwrap_or(0) >= self.inner.options.max_bytes {
            self.rotate(io).await?;
        }
        Ok(())
    }

    async fn rotate(&self, io: &mut IoState) -> std::io::Result<()> {
        let path = &self.inner.options.path;
        let files = self.inner.options.files;
        if files >= 3 {
            for index in (1..=files - 2).rev() {
                rename_if_exists(&numbered(path, index), &numbered(path, index + 1)).await?;
            }
        }
        rename_if_exists(path, &numbered(path, 1)).await?;
        remove_if_exists(&numbered(path, files)).await?;
        io.current_bytes = Some(0);
        io.directory_synced = false;
        Ok(())
    }

    fn restore_pending(&self, lines: Vec<String>) {
        let mut pending = self.inner.pending.lock().unwrap();
        let mut restored = lines.into_iter().collect::<VecDeque<_>>();
        restored.append(&mut pending.lines);
        let overflow = restored.len().saturating_sub(PENDING_MAX);
        pending.dropped += overflow;
        restored.drain(..overflow);
        pending.lines = restored;
    }

    fn note_failure(&self, error: &std::io::Error) {
        let mut pending = self.inner.pending.lock().unwrap();
        let now = Instant::now();
        if pending
            .last_stderr_notice
            .is_some_and(|last| now.duration_since(last) < STDERR_NOTICE_INTERVAL)
        {
            return;
        }
        pending.last_stderr_notice = Some(now);
        eprintln!("[logger] write failed: {:?}", error.kind());
    }
}

fn take_dropped_notice(pending: &mut PendingState) -> String {
    if pending.dropped == 0 {
        return String::new();
    }
    let notice = format!("... dropped {} entries ...\n", pending.dropped);
    pending.dropped = 0;
    notice
}

fn parent_directory(path: &Path) -> &Path {
    path.parent().unwrap_or_else(|| Path::new("."))
}

fn numbered(path: &Path, number: usize) -> PathBuf {
    let mut value = path.as_os_str().to_owned();
    value.push(format!(".{number}"));
    value.into()
}

async fn stat_size(path: &Path) -> std::io::Result<u64> {
    match fs::metadata(path).await {
        Ok(metadata) => Ok(metadata.len()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(0),
        Err(error) => Err(error),
    }
}

async fn rename_if_exists(from: &Path, to: &Path) -> std::io::Result<()> {
    match fs::rename(from, to).await {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

async fn remove_if_exists(path: &Path) -> std::io::Result<()> {
    match fs::remove_file(path).await {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

#[derive(Clone)]
pub struct FileLogWriter {
    sink: RotatingFileWriter,
    format: FormatOptions,
}

impl FileLogWriter {
    pub fn new(options: RotatingFileWriterOptions, format: FormatOptions) -> Self {
        Self {
            sink: RotatingFileWriter::new(options),
            format,
        }
    }
}

impl LogWriter for FileLogWriter {
    fn write(&self, entry: LogEntry) {
        let formatted = format_entry(&entry, &self.format);
        if !formatted.dropped {
            self.sink.enqueue(formatted.text + "\n");
        }
    }

    fn flush(&self) -> BoxFuture<'_, std::io::Result<()>> {
        Box::pin(async move {
            let _ = self.sink.flush().await;
            Ok(())
        })
    }

    fn close(&self) -> BoxFuture<'_, std::io::Result<()>> {
        Box::pin(async move {
            self.sink.close().await;
            Ok(())
        })
    }

    fn flush_sync(&self) -> std::io::Result<()> {
        self.sink.flush_sync();
        Ok(())
    }
}

#[derive(Default)]
pub struct MemoryLogWriter {
    entries: Mutex<Vec<LogEntry>>,
}

impl MemoryLogWriter {
    pub fn entries(&self) -> Vec<LogEntry> {
        self.entries.lock().unwrap().clone()
    }
}

impl LogWriter for MemoryLogWriter {
    fn write(&self, entry: LogEntry) {
        self.entries.lock().unwrap().push(entry);
    }
}

#[derive(Default)]
pub struct ConsoleLogWriter;

impl LogWriter for ConsoleLogWriter {
    fn write(&self, entry: LogEntry) {
        let text = format_entry(&entry, &FormatOptions::default()).text;
        match entry.level {
            super::contract::LogLevel::Error | super::contract::LogLevel::Warn => {
                eprintln!("{text}")
            }
            _ => println!("{text}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use uuid::Uuid;

    use super::*;

    #[tokio::test]
    async fn rotates_at_size_boundary_and_keeps_newest_generations() {
        let directory = std::env::temp_dir().join(format!("kimi-log-{}", Uuid::new_v4()));
        let path = directory.join("kimi.log");
        let writer = RotatingFileWriter::new(RotatingFileWriterOptions {
            path: path.clone(),
            max_bytes: 6,
            files: 3,
        });
        writer.enqueue("one\n");
        writer.enqueue("two\n");
        writer.enqueue("three\n");
        assert!(writer.flush().await);
        assert_eq!(
            fs::read_to_string(numbered(&path, 1)).await.unwrap(),
            "three\n"
        );
        assert_eq!(
            fs::read_to_string(numbered(&path, 2)).await.unwrap(),
            "two\n"
        );
        fs::remove_dir_all(directory).await.unwrap();
    }

    #[test]
    fn synchronous_flush_preserves_pending_lines() {
        let directory = std::env::temp_dir().join(format!("kimi-log-{}", Uuid::new_v4()));
        let path = directory.join("kimi.log");
        let writer = RotatingFileWriter::new(RotatingFileWriterOptions {
            path: path.clone(),
            max_bytes: 100,
            files: 2,
        });
        writer.enqueue("line\n");
        writer.flush_sync();
        assert_eq!(std::fs::read_to_string(path).unwrap(), "line\n");
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn pending_cap_drops_oldest_and_emits_one_notice() {
        let directory = std::env::temp_dir().join(format!("kimi-log-{}", Uuid::new_v4()));
        let path = directory.join("kimi.log");
        let writer = RotatingFileWriter::new(RotatingFileWriterOptions {
            path: path.clone(),
            max_bytes: u64::MAX,
            files: 2,
        });
        for index in 0..=PENDING_MAX {
            writer.enqueue(format!("entry-{index}\n"));
        }
        writer.flush_sync();
        let content = std::fs::read_to_string(path).unwrap();
        assert!(!content.contains("entry-0\n"));
        assert!(content.contains("entry-1\n"));
        assert!(content.ends_with("... dropped 1 entries ...\n"));
        std::fs::remove_dir_all(directory).unwrap();
    }
}
