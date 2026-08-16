//! Persisted wire-log activity scanner.
//! Original: `packages/agent-core-v2/src/app/sessionExport/wire-scan.ts`.
use super::{SessionWireScan, check};
use serde_json::Value;
use std::{
    io,
    path::{Path, PathBuf},
};
use tokio::{
    fs,
    io::{AsyncBufReadExt, BufReader},
};
use tokio_util::sync::CancellationToken;
const WIRE_FILENAME: &str = "wire.jsonl";
pub async fn scan_session_wire(
    session_dir: &Path,
    cancellation: Option<&CancellationToken>,
) -> io::Result<SessionWireScan> {
    check(cancellation)?;
    let files = collect_wire_files(session_dir, cancellation).await?;
    let mut output = SessionWireScan::default();
    for file in files {
        check(cancellation)?;
        let scan = scan_wire_file(&file, cancellation).await?;
        output.first_activity_ms = min(output.first_activity_ms, scan.first_activity_ms);
        output.last_activity_ms = max(output.last_activity_ms, scan.last_activity_ms);
        output.last_user_message_ms = max(output.last_user_message_ms, scan.last_user_message_ms);
        if output.first_user_input.is_none() {
            output.first_user_input = scan.first_user_input;
        }
    }
    Ok(output)
}
async fn collect_wire_files(
    session_dir: &Path,
    cancellation: Option<&CancellationToken>,
) -> io::Result<Vec<PathBuf>> {
    let mut files = vec![session_dir.join(WIRE_FILENAME)];
    let agents = session_dir.join("agents");
    if let Err(error) = collect_recursive(&agents, &mut files, cancellation).await
        && error.kind() != io::ErrorKind::NotFound
    {
        return Err(error);
    }
    Ok(files)
}
async fn collect_recursive(
    root: &Path,
    files: &mut Vec<PathBuf>,
    cancellation: Option<&CancellationToken>,
) -> io::Result<()> {
    let mut stack = vec![root.to_path_buf()];
    while let Some(directory) = stack.pop() {
        check(cancellation)?;
        let mut entries = fs::read_dir(directory).await?;
        while let Some(entry) = entries.next_entry().await? {
            check(cancellation)?;
            let kind = entry.file_type().await?;
            if kind.is_dir() {
                stack.push(entry.path())
            } else if kind.is_file() && entry.file_name() == WIRE_FILENAME {
                files.push(entry.path())
            }
        }
    }
    Ok(())
}
async fn scan_wire_file(
    path: &Path,
    cancellation: Option<&CancellationToken>,
) -> io::Result<SessionWireScan> {
    let file = match fs::File::open(path).await {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(SessionWireScan::default());
        }
        Err(error) => return Err(error),
    };
    let mut lines = BufReader::new(file).lines();
    let mut scan = SessionWireScan::default();
    while let Some(line) = lines.next_line().await? {
        check(cancellation)?;
        let Ok(value) = serde_json::from_str::<Value>(line.trim()) else {
            continue;
        };
        let Some(record) = value.as_object() else {
            continue;
        };
        let time = record
            .get("time")
            .and_then(Value::as_f64)
            .and_then(normalize_timestamp_ms);
        scan.first_activity_ms = min(scan.first_activity_ms, time);
        scan.last_activity_ms = max(scan.last_activity_ms, time);
        if record.get("type").and_then(Value::as_str) == Some("turn_begin") {
            scan.last_user_message_ms = max(scan.last_user_message_ms, time);
            if scan.first_user_input.is_none() {
                scan.first_user_input = record
                    .get("userInput")
                    .and_then(Value::as_str)
                    .filter(|value| !value.trim().is_empty())
                    .map(str::to_owned)
            }
        }
    }
    Ok(scan)
}
pub fn normalize_timestamp_ms(value: f64) -> Option<i64> {
    if !value.is_finite() || value <= 0.0 {
        None
    } else if value > 1e12 {
        Some(value.floor() as i64)
    } else {
        Some((value * 1000.0).floor() as i64)
    }
}
fn min(a: Option<i64>, b: Option<i64>) -> Option<i64> {
    match (a, b) {
        (None, b) => b,
        (a, None) => a,
        (Some(a), Some(b)) => Some(a.min(b)),
    }
}
fn max(a: Option<i64>, b: Option<i64>) -> Option<i64> {
    match (a, b) {
        (None, b) => b,
        (a, None) => a,
        (Some(a), Some(b)) => Some(a.max(b)),
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_seconds_and_milliseconds() {
        assert_eq!(normalize_timestamp_ms(1.25), Some(1250));
        assert_eq!(
            normalize_timestamp_ms(1_700_000_000_000.9),
            Some(1_700_000_000_000)
        );
        assert_eq!(normalize_timestamp_ms(0.0), None);
    }

    #[tokio::test]
    async fn scans_root_and_agent_wire_logs() {
        let session_dir =
            std::env::temp_dir().join(format!("kimi-wire-scan-{}", uuid::Uuid::new_v4()));
        let agent_dir = session_dir.join("agents").join("child");
        fs::create_dir_all(&agent_dir).await.unwrap();
        fs::write(
            session_dir.join(WIRE_FILENAME),
            "not json\n{\"time\": 2, \"type\": \"turn_begin\", \"userInput\": \"  \"}\n",
        )
        .await
        .unwrap();
        fs::write(
            agent_dir.join(WIRE_FILENAME),
            "{\"time\": 3, \"type\": \"turn_begin\", \"userInput\": \"first question\"}\n{\"time\": 4}\n",
        )
        .await
        .unwrap();

        let scan = scan_session_wire(&session_dir, None).await.unwrap();

        assert_eq!(scan.first_activity_ms, Some(2_000));
        assert_eq!(scan.last_activity_ms, Some(4_000));
        assert_eq!(scan.last_user_message_ms, Some(3_000));
        assert_eq!(scan.first_user_input.as_deref(), Some("first question"));
        fs::remove_dir_all(session_dir).await.unwrap();
    }
}
