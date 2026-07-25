use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use kimi_code_protocol::WsEventEnvelope;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::fs::{self, OpenOptions};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::Notify;
use tokio::task::JoinHandle;
use ulid::Ulid;

const JOURNAL_VERSION: u64 = 1;

pub type EventEnvelope = WsEventEnvelope<Value>;

#[derive(Debug, Clone, PartialEq)]
pub struct JournalEntry {
    pub seq: u64,
    pub envelope: EventEnvelope,
}

pub trait JournalLogger: Send + Sync {
    fn warn(&self, file_path: &Path, message: &str, error: Option<&dyn std::fmt::Display>);
}

#[derive(Debug)]
struct NoopLogger;

impl JournalLogger for NoopLogger {
    fn warn(&self, _file_path: &Path, _message: &str, _error: Option<&dyn std::fmt::Display>) {}
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind")]
enum JournalLine {
    #[serde(rename = "journal_header")]
    Header {
        version: u64,
        epoch: String,
        created_at: i64,
    },
    #[serde(rename = "event")]
    Event { seq: u64, envelope: EventEnvelope },
}

#[derive(Debug)]
struct PendingState {
    lines: Vec<String>,
    header_pending: bool,
    flushing: bool,
    closed: bool,
}

pub struct SessionEventJournal {
    file_path: PathBuf,
    logger: Arc<dyn JournalLogger>,
    pub epoch: String,
    seq: AtomicU64,
    pending: Mutex<PendingState>,
    worker_notify: Notify,
    flush_notify: Notify,
    worker: tokio::sync::Mutex<Option<JoinHandle<()>>>,
}

impl std::fmt::Debug for SessionEventJournal {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SessionEventJournal")
            .field("file_path", &self.file_path)
            .field("epoch", &self.epoch)
            .field("seq", &self.seq())
            .finish_non_exhaustive()
    }
}

impl SessionEventJournal {
    pub async fn open(file_path: impl AsRef<Path>) -> io::Result<Arc<Self>> {
        Self::open_with_logger(file_path, Arc::new(NoopLogger)).await
    }

    // Original: sessionEventJournal.ts, SessionEventJournal.open().
    pub async fn open_with_logger(
        file_path: impl AsRef<Path>,
        logger: Arc<dyn JournalLogger>,
    ) -> io::Result<Arc<Self>> {
        let file_path = file_path.as_ref().to_owned();
        let mut epoch = None;
        let mut last_seq = 0;
        let mut saw_any_line = false;

        match read_lines(&file_path).await {
            Ok(lines) => {
                for raw in lines {
                    saw_any_line = true;
                    let Some(parsed) = parse_journal_line(&raw) else {
                        continue;
                    };
                    match parsed {
                        JournalLine::Header {
                            epoch: line_epoch, ..
                        } if epoch.is_none() => epoch = Some(line_epoch),
                        JournalLine::Event { seq, .. } => last_seq = last_seq.max(seq),
                        JournalLine::Header { .. } => {}
                    }
                }
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => {
                logger.warn(
                    &file_path,
                    "event journal unreadable; starting a fresh epoch",
                    Some(&error),
                );
            }
        }

        let is_fresh = epoch.is_none();
        if is_fresh && saw_any_line {
            logger.warn(
                &file_path,
                "event journal missing header; rotating to a fresh epoch",
                None,
            );
        }
        let journal = Arc::new(Self {
            file_path,
            logger,
            epoch: epoch.unwrap_or_else(|| format!("ep_{}", Ulid::new())),
            seq: AtomicU64::new(if is_fresh { 0 } else { last_seq }),
            pending: Mutex::new(PendingState {
                lines: Vec::new(),
                header_pending: is_fresh,
                flushing: false,
                closed: false,
            }),
            worker_notify: Notify::new(),
            flush_notify: Notify::new(),
            worker: tokio::sync::Mutex::new(None),
        });
        let worker_journal = Arc::clone(&journal);
        *journal.worker.lock().await = Some(tokio::spawn(async move {
            worker_journal.flush_worker().await;
        }));
        Ok(journal)
    }

    pub fn seq(&self) -> u64 {
        self.seq.load(Ordering::SeqCst)
    }

    /// Reserve the next durable sequence number.
    pub fn next_seq(&self) -> u64 {
        self.seq.fetch_add(1, Ordering::SeqCst) + 1
    }

    // Original: SessionEventJournal.append(). This remains synchronous:
    // serialization is in-memory and file I/O is owned by the async worker.
    pub fn append(&self, seq: u64, envelope: EventEnvelope) {
        let line = JournalLine::Event { seq, envelope };
        let serialized = serde_json::to_string(&line)
            .expect("event envelopes accepted by the wire contract must serialize");
        let mut pending = self
            .pending
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        pending.lines.push(serialized);
        drop(pending);
        self.worker_notify.notify_one();
    }

    pub async fn read_since(
        &self,
        from_seq_exclusive: u64,
        limit: usize,
    ) -> io::Result<Vec<JournalEntry>> {
        self.flush().await;
        let lines = match read_lines(&self.file_path).await {
            Ok(lines) => lines,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error),
        };
        Ok(lines
            .into_iter()
            .filter_map(|line| match parse_journal_line(&line) {
                Some(JournalLine::Event { seq, envelope }) if seq > from_seq_exclusive => {
                    Some(JournalEntry { seq, envelope })
                }
                _ => None,
            })
            .take(limit)
            .collect())
    }

    pub async fn flush(&self) {
        loop {
            let notified = self.flush_notify.notified();
            let is_flushed = {
                let pending = self
                    .pending
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                pending.lines.is_empty() && !pending.flushing
            };
            if is_flushed {
                return;
            }
            self.worker_notify.notify_one();
            notified.await;
        }
    }

    pub async fn close(&self) {
        self.flush().await;
        {
            let mut pending = self
                .pending
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            pending.closed = true;
        }
        self.worker_notify.notify_one();
        if let Some(worker) = self.worker.lock().await.take() {
            let _ = worker.await;
        }
    }

    async fn flush_worker(self: Arc<Self>) {
        loop {
            let notified = self.worker_notify.notified();
            let batch = {
                let mut pending = self
                    .pending
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                if pending.lines.is_empty() {
                    pending.flushing = false;
                    self.flush_notify.notify_waiters();
                    if pending.closed {
                        return;
                    }
                    None
                } else {
                    pending.flushing = true;
                    let mut lines = Vec::new();
                    if pending.header_pending {
                        let header = JournalLine::Header {
                            version: JOURNAL_VERSION,
                            epoch: self.epoch.clone(),
                            created_at: now_millis(),
                        };
                        if let Ok(header) = serde_json::to_string(&header) {
                            lines.push(header);
                        }
                        pending.header_pending = false;
                    }
                    lines.append(&mut pending.lines);
                    Some(lines)
                }
            };

            let Some(lines) = batch else {
                notified.await;
                continue;
            };
            if let Err(error) = append_lines(&self.file_path, &lines).await {
                self.logger.warn(
                    &self.file_path,
                    "event journal write failed; events remain live-only this round",
                    Some(&error),
                );
            }
            {
                let mut pending = self
                    .pending
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                pending.flushing = false;
            }
            self.flush_notify.notify_waiters();
        }
    }
}

pub fn session_journal_path(events_dir: impl AsRef<Path>, session_id: &str) -> PathBuf {
    events_dir.as_ref().join(format!("{session_id}.jsonl"))
}

fn parse_journal_line(raw: &str) -> Option<JournalLine> {
    let trimmed = raw.strip_suffix('\r').unwrap_or(raw);
    if trimmed.is_empty() {
        return None;
    }
    match serde_json::from_str::<JournalLine>(trimmed).ok()? {
        JournalLine::Header { ref epoch, .. } if epoch.is_empty() => None,
        JournalLine::Event { seq: 0, .. } => None,
        line => Some(line),
    }
}

async fn read_lines(file_path: &Path) -> io::Result<Vec<String>> {
    let file = fs::File::open(file_path).await?;
    let mut lines = BufReader::new(file).lines();
    let mut output = Vec::new();
    while let Some(line) = lines.next_line().await? {
        output.push(line);
    }
    Ok(output)
}

async fn append_lines(file_path: &Path, lines: &[String]) -> io::Result<()> {
    fs::create_dir_all(file_path.parent().unwrap_or_else(|| Path::new("."))).await?;
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(file_path)
        .await?;
    file.write_all(lines.join("\n").as_bytes()).await?;
    file.write_all(b"\n").await
}

fn now_millis() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

#[cfg(test)]
mod tests {
    use kimi_code_protocol::{IsoDateTime, now_iso_date_time};

    use super::*;

    fn envelope(seq: u64) -> EventEnvelope {
        EventEnvelope {
            event_type: "turn.started".into(),
            seq,
            epoch: None,
            volatile: None,
            offset: None,
            session_id: None,
            timestamp: now_iso_date_time(),
            payload: serde_json::json!({ "seq": seq }),
        }
    }

    #[tokio::test]
    async fn assigns_monotonic_sequences_and_reads_pages() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("session.jsonl");
        let journal = SessionEventJournal::open(&path).await.unwrap();
        assert!(journal.epoch.starts_with("ep_"));
        for seq in 1..=5 {
            assert_eq!(journal.next_seq(), seq);
            journal.append(seq, envelope(seq));
        }
        assert_eq!(journal.seq(), 5);
        let page = journal.read_since(2, 2).await.unwrap();
        assert_eq!(
            page.iter().map(|entry| entry.seq).collect::<Vec<_>>(),
            [3, 4]
        );
        journal.close().await;
    }

    #[tokio::test]
    async fn recovers_epoch_and_sequence_across_reopen() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("session.jsonl");
        let first = SessionEventJournal::open(&path).await.unwrap();
        let epoch = first.epoch.clone();
        for seq in 1..=2 {
            first.next_seq();
            first.append(seq, envelope(seq));
        }
        first.close().await;

        let second = SessionEventJournal::open(&path).await.unwrap();
        assert_eq!(second.epoch, epoch);
        assert_eq!(second.seq(), 2);
        assert_eq!(second.next_seq(), 3);
        second.close().await;
    }

    #[tokio::test]
    async fn corrupt_header_rotates_epoch_and_torn_lines_are_ignored() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("session.jsonl");
        fs::write(&path, "not json\n").await.unwrap();
        let journal = SessionEventJournal::open(&path).await.unwrap();
        assert!(journal.epoch.starts_with("ep_"));
        assert_eq!(journal.seq(), 0);
        journal.append(journal.next_seq(), envelope(1));
        journal.close().await;

        // A corrupt trailing line does not prevent recovery of the valid
        // header and event written after the original corrupt line.
        let mut file = OpenOptions::new().append(true).open(&path).await.unwrap();
        file.write_all(b"{\"kind\":\"event\"").await.unwrap();
        drop(file);
        let reopened = SessionEventJournal::open(&path).await.unwrap();
        assert_eq!(reopened.seq(), 1);
        reopened.close().await;
    }

    #[tokio::test]
    async fn empty_fresh_journal_reads_empty_without_creating_file() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("session.jsonl");
        let journal = SessionEventJournal::open(&path).await.unwrap();
        assert!(journal.read_since(0, 100).await.unwrap().is_empty());
        journal.close().await;
        assert!(!path.exists());
    }

    #[test]
    fn event_envelope_timestamp_type_remains_protocol_owned() {
        let _: IsoDateTime = envelope(1).timestamp;
    }
}
