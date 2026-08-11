use std::{
    collections::{BTreeMap, HashMap},
    fs::File,
    io::{self, BufRead, BufReader},
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};

use chrono::{DateTime, Duration, Local, NaiveDate, TimeZone, Utc};
use serde::{Deserialize, Serialize};

use crate::_base::utils::fs::atomic_write;
use crate::agent::TurnId;

const CACHE_VERSION: u32 = 1;
const CACHE_RELATIVE_PATH: &str = "cache/usage-statistics-v1.json";
const WIRE_FILENAME: &str = "wire.jsonl";

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopDailyTokenUsage {
    pub date: String,
    pub total_tokens: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopUsageStatistics {
    pub total_tokens: f64,
    pub peak_daily_tokens: f64,
    pub longest_task_ms: f64,
    pub current_streak_days: u32,
    pub longest_streak_days: u32,
    pub days: Vec<DesktopDailyTokenUsage>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
struct UsageStatisticsCache {
    version: u32,
    files: BTreeMap<String, CachedWireUsage>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
struct CachedWireUsage {
    signature: FileSignature,
    daily_tokens: BTreeMap<String, f64>,
    longest_task_ms: f64,
}

#[derive(Debug, Clone, Copy, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
struct FileSignature {
    size: u64,
    modified_secs: u64,
    modified_nanos: u32,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireRecord {
    #[serde(rename = "type")]
    record_type: String,
    #[serde(default)]
    time: Option<i64>,
    #[serde(default)]
    usage: Option<WireTokenUsage>,
    #[serde(default)]
    event: Option<WireLoopEvent>,
    #[serde(default)]
    turn_id: Option<TurnId>,
    #[serde(default)]
    duration_ms: Option<f64>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireTokenUsage {
    #[serde(default)]
    input_other: f64,
    #[serde(default)]
    output: f64,
    #[serde(default)]
    input_cache_read: f64,
    #[serde(default)]
    input_cache_creation: f64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireLoopEvent {
    #[serde(default)]
    turn_id: Option<TurnId>,
}

#[derive(Debug)]
struct ScanOutcome {
    statistics: DesktopUsageStatistics,
    cache_contents: Option<Vec<u8>>,
    #[cfg(test)]
    reparsed_files: usize,
}

pub async fn collect_usage_statistics(home_dir: PathBuf) -> Result<DesktopUsageStatistics, String> {
    let cache_path = home_dir.join(CACHE_RELATIVE_PATH);
    let today = Local::now().date_naive();
    let outcome = tokio::task::spawn_blocking(move || scan_usage_statistics(&home_dir, today))
        .await
        .map_err(|error| format!("usage statistics task failed: {error}"))??;

    if let Some(contents) = outcome.cache_contents {
        if let Some(parent) = cache_path.parent()
            && let Err(error) = tokio::fs::create_dir_all(parent).await
        {
            eprintln!("failed to create usage statistics cache directory: {error}");
            return Ok(outcome.statistics);
        }
        if let Err(error) = atomic_write(&cache_path, contents, Some(0o600)).await {
            eprintln!("failed to write usage statistics cache: {error}");
        }
    }

    Ok(outcome.statistics)
}

fn scan_usage_statistics(home_dir: &Path, today: NaiveDate) -> Result<ScanOutcome, String> {
    let sessions_dir = home_dir.join("sessions");
    let cache_path = home_dir.join(CACHE_RELATIVE_PATH);
    let old_cache = load_cache(&cache_path);
    let old_files = old_cache
        .as_ref()
        .map(|cache| &cache.files)
        .cloned()
        .unwrap_or_default();
    let mut files = BTreeMap::new();
    #[cfg(test)]
    let mut reparsed_files = 0;

    for discovered in discover_wire_files(&sessions_dir).map_err(|error| error.to_string())? {
        if old_files
            .get(&discovered.relative_path)
            .is_some_and(|cached| cached.signature == discovered.signature)
        {
            files.insert(
                discovered.relative_path.clone(),
                old_files[&discovered.relative_path].clone(),
            );
            continue;
        }

        #[cfg(test)]
        {
            reparsed_files += 1;
        }
        match parse_stable_wire_file(&discovered.path, discovered.signature) {
            Ok(Some(cached)) => {
                files.insert(discovered.relative_path, cached);
            }
            Ok(None) => {
                eprintln!(
                    "usage statistics skipped a wire file that kept changing: {}",
                    discovered.path.display()
                );
            }
            Err(error) => {
                eprintln!(
                    "usage statistics failed to read {}: {error}",
                    discovered.path.display()
                );
            }
        }
    }

    let cache = UsageStatisticsCache {
        version: CACHE_VERSION,
        files,
    };
    let statistics = aggregate_statistics(&cache.files, today);
    let cache_contents = if old_cache.as_ref() == Some(&cache) {
        None
    } else {
        Some(
            serde_json::to_vec(&cache)
                .map_err(|error| format!("failed to encode usage statistics cache: {error}"))?,
        )
    };

    Ok(ScanOutcome {
        statistics,
        cache_contents,
        #[cfg(test)]
        reparsed_files,
    })
}

fn load_cache(path: &Path) -> Option<UsageStatisticsCache> {
    let contents = std::fs::read(path).ok()?;
    let cache: UsageStatisticsCache = serde_json::from_slice(&contents).ok()?;
    (cache.version == CACHE_VERSION).then_some(cache)
}

struct DiscoveredWireFile {
    path: PathBuf,
    relative_path: String,
    signature: FileSignature,
}

fn discover_wire_files(sessions_dir: &Path) -> io::Result<Vec<DiscoveredWireFile>> {
    if !sessions_dir.exists() {
        return Ok(Vec::new());
    }

    let mut pending = vec![sessions_dir.to_owned()];
    let mut discovered = Vec::new();
    while let Some(directory) = pending.pop() {
        let entries = match std::fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(error) if directory != sessions_dir => {
                eprintln!(
                    "usage statistics could not read directory {}: {error}",
                    directory.display()
                );
                continue;
            }
            Err(error) => return Err(error),
        };
        for entry in entries.flatten() {
            let file_type = match entry.file_type() {
                Ok(file_type) => file_type,
                Err(_) => continue,
            };
            if file_type.is_dir() {
                pending.push(entry.path());
                continue;
            }
            if !file_type.is_file() || entry.file_name() != WIRE_FILENAME {
                continue;
            }
            let path = entry.path();
            let Ok(metadata) = entry.metadata() else {
                continue;
            };
            let relative_path = normalized_relative_path(sessions_dir, &path);
            discovered.push(DiscoveredWireFile {
                path,
                relative_path,
                signature: signature_from_metadata(&metadata),
            });
        }
    }
    discovered.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
    Ok(discovered)
}

fn normalized_relative_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

fn signature_from_metadata(metadata: &std::fs::Metadata) -> FileSignature {
    let modified = metadata
        .modified()
        .ok()
        .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
        .unwrap_or_default();
    FileSignature {
        size: metadata.len(),
        modified_secs: modified.as_secs(),
        modified_nanos: modified.subsec_nanos(),
    }
}

fn file_signature(path: &Path) -> io::Result<FileSignature> {
    std::fs::metadata(path).map(|metadata| signature_from_metadata(&metadata))
}

fn parse_stable_wire_file(
    path: &Path,
    initial_signature: FileSignature,
) -> io::Result<Option<CachedWireUsage>> {
    let mut expected_signature = initial_signature;
    for _ in 0..2 {
        let mut parsed = parse_wire_file(path)?;
        let actual_signature = file_signature(path)?;
        if actual_signature == expected_signature {
            parsed.signature = actual_signature;
            return Ok(Some(parsed));
        }
        expected_signature = actual_signature;
    }
    Ok(None)
}

fn parse_wire_file(path: &Path) -> io::Result<CachedWireUsage> {
    let reader = BufReader::new(File::open(path)?);
    let mut daily_tokens = BTreeMap::<String, f64>::new();
    let mut turn_spans = HashMap::<TurnId, (i64, i64)>::new();
    let mut ended_durations = HashMap::<TurnId, f64>::new();

    for line in reader.lines() {
        let Ok(line) = line else {
            continue;
        };
        let Ok(record) = serde_json::from_str::<WireRecord>(&line) else {
            continue;
        };
        match record.record_type.as_str() {
            "usage.record" => {
                let (Some(time), Some(usage)) = (record.time, record.usage) else {
                    continue;
                };
                let Some(date) = local_date_key(time) else {
                    continue;
                };
                let total = usage.grand_total();
                if total > 0.0 && total.is_finite() {
                    *daily_tokens.entry(date).or_default() += total;
                }
            }
            "context.append_loop_event" => {
                let (Some(time), Some(turn_id)) =
                    (record.time, record.event.and_then(|event| event.turn_id))
                else {
                    continue;
                };
                let span = turn_spans.entry(turn_id).or_insert((time, time));
                span.0 = span.0.min(time);
                span.1 = span.1.max(time);
            }
            "turn.ended" => {
                let (Some(turn_id), Some(duration_ms)) = (record.turn_id, record.duration_ms)
                else {
                    continue;
                };
                if duration_ms >= 0.0 && duration_ms.is_finite() {
                    ended_durations
                        .entry(turn_id)
                        .and_modify(|value| *value = value.max(duration_ms))
                        .or_insert(duration_ms);
                }
            }
            _ => {}
        }
    }

    let longest_task_ms = turn_spans
        .iter()
        .map(|(turn_id, (first, last))| {
            ended_durations
                .get(turn_id)
                .copied()
                .unwrap_or_else(|| last.saturating_sub(*first) as f64)
        })
        .chain(ended_durations.values().copied())
        .fold(0.0, f64::max);

    Ok(CachedWireUsage {
        signature: file_signature(path)?,
        daily_tokens,
        longest_task_ms,
    })
}

impl WireTokenUsage {
    fn grand_total(&self) -> f64 {
        [
            self.input_other,
            self.output,
            self.input_cache_read,
            self.input_cache_creation,
        ]
        .into_iter()
        .filter(|value| value.is_finite() && *value > 0.0)
        .sum()
    }
}

fn local_date_key(timestamp_ms: i64) -> Option<String> {
    let utc: DateTime<Utc> = Utc.timestamp_millis_opt(timestamp_ms).single()?;
    Some(
        utc.with_timezone(&Local)
            .date_naive()
            .format("%Y-%m-%d")
            .to_string(),
    )
}

fn aggregate_statistics(
    files: &BTreeMap<String, CachedWireUsage>,
    today: NaiveDate,
) -> DesktopUsageStatistics {
    let mut daily_tokens = BTreeMap::<String, f64>::new();
    let mut longest_task_ms = 0.0_f64;
    for cached in files.values() {
        longest_task_ms = longest_task_ms.max(cached.longest_task_ms);
        for (date, total) in &cached.daily_tokens {
            *daily_tokens.entry(date.clone()).or_default() += total;
        }
    }

    let total_tokens = daily_tokens.values().sum();
    let peak_daily_tokens = daily_tokens.values().copied().fold(0.0, f64::max);
    let active_dates = daily_tokens
        .iter()
        .filter(|(_, total)| **total > 0.0)
        .filter_map(|(date, _)| NaiveDate::parse_from_str(date, "%Y-%m-%d").ok())
        .collect::<Vec<_>>();
    let (current_streak_days, longest_streak_days) = streaks(&active_dates, today);
    let days = daily_tokens
        .into_iter()
        .map(|(date, total_tokens)| DesktopDailyTokenUsage { date, total_tokens })
        .collect();

    DesktopUsageStatistics {
        total_tokens,
        peak_daily_tokens,
        longest_task_ms,
        current_streak_days,
        longest_streak_days,
        days,
    }
}

fn streaks(active_dates: &[NaiveDate], today: NaiveDate) -> (u32, u32) {
    if active_dates.is_empty() {
        return (0, 0);
    }

    let mut dates = active_dates.to_vec();
    dates.sort_unstable();
    dates.dedup();

    let mut longest = 1_u32;
    let mut run = 1_u32;
    for pair in dates.windows(2) {
        if pair[1].signed_duration_since(pair[0]) == Duration::days(1) {
            run += 1;
            longest = longest.max(run);
        } else {
            run = 1;
        }
    }

    let latest = *dates.last().unwrap();
    let yesterday = today.checked_sub_signed(Duration::days(1));
    if latest != today && Some(latest) != yesterday {
        return (0, longest);
    }
    let mut current = 1_u32;
    for pair in dates.windows(2).rev() {
        if pair[1].signed_duration_since(pair[0]) != Duration::days(1) {
            break;
        }
        current += 1;
    }
    (current, longest)
}

#[cfg(test)]
mod tests {
    use std::{fs, io::Write};

    use chrono::{Datelike, LocalResult};
    use serde_json::json;
    use uuid::Uuid;

    use super::*;

    fn test_home() -> PathBuf {
        let path = std::env::temp_dir().join(format!("kimi-usage-statistics-{}", Uuid::new_v4()));
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn local_millis(date: NaiveDate, hour: u32) -> i64 {
        match Local.with_ymd_and_hms(date.year(), date.month(), date.day(), hour, 0, 0) {
            LocalResult::Single(value) => value.timestamp_millis(),
            LocalResult::Ambiguous(value, _) => value.timestamp_millis(),
            LocalResult::None => panic!("test date must exist in the local timezone"),
        }
    }

    fn write_wire(home: &Path, agent: &str, records: &[serde_json::Value]) -> PathBuf {
        let path = home
            .join("sessions/workspace/session/agents")
            .join(agent)
            .join(WIRE_FILENAME);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut file = File::create(&path).unwrap();
        for record in records {
            writeln!(file, "{record}").unwrap();
        }
        path
    }

    fn persist_cache(home: &Path, outcome: &ScanOutcome) {
        let path = home.join(CACHE_RELATIVE_PATH);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, outcome.cache_contents.as_ref().unwrap()).unwrap();
    }

    #[test]
    fn aggregates_main_and_subagents_without_merging_reused_turn_ids() {
        let home = test_home();
        let today = NaiveDate::from_ymd_opt(2026, 8, 11).unwrap();
        let yesterday = today - Duration::days(1);
        let today_ms = local_millis(today, 12);
        let yesterday_ms = local_millis(yesterday, 12);
        write_wire(
            &home,
            "main",
            &[
                json!({"type":"usage.record","time":today_ms,"usage":{"inputOther":1,"output":2,"inputCacheRead":3,"inputCacheCreation":4}}),
                json!({"type":"context.append_loop_event","time":1_000,"event":{"turnId":"0"}}),
                json!({"type":"context.append_loop_event","time":5_000,"event":{"turnId":"0"}}),
                json!({"type":"turn.ended","turnId":"0","durationMs":15_000}),
            ],
        );
        write_wire(
            &home,
            "agent-0",
            &[
                json!({"type":"usage.record","time":yesterday_ms,"usage":{"inputOther":10,"output":20,"inputCacheRead":30,"inputCacheCreation":40}}),
                json!({"type":"context.append_loop_event","time":1_000,"event":{"turnId":"0"}}),
                json!({"type":"context.append_loop_event","time":12_000,"event":{"turnId":"0"}}),
            ],
        );

        let outcome = scan_usage_statistics(&home, today).unwrap();
        assert_eq!(outcome.reparsed_files, 2);
        assert_eq!(outcome.statistics.total_tokens, 110.0);
        assert_eq!(outcome.statistics.peak_daily_tokens, 100.0);
        assert_eq!(outcome.statistics.longest_task_ms, 15_000.0);
        assert_eq!(outcome.statistics.current_streak_days, 2);
        assert_eq!(outcome.statistics.longest_streak_days, 2);
        assert_eq!(outcome.statistics.days.len(), 2);
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn reuses_unchanged_files_and_invalidates_only_changed_cache_entries() {
        let home = test_home();
        let today = NaiveDate::from_ymd_opt(2026, 8, 11).unwrap();
        let timestamp = local_millis(today, 12);
        let main = write_wire(
            &home,
            "main",
            &[json!({"type":"usage.record","time":timestamp,"usage":{"output":5}})],
        );
        let child = write_wire(
            &home,
            "agent-0",
            &[json!({"type":"usage.record","time":timestamp,"usage":{"output":7}})],
        );
        let first = scan_usage_statistics(&home, today).unwrap();
        assert_eq!(first.reparsed_files, 2);
        persist_cache(&home, &first);

        let unchanged = scan_usage_statistics(&home, today).unwrap();
        assert_eq!(unchanged.reparsed_files, 0);
        assert!(unchanged.cache_contents.is_none());

        fs::write(
            &child,
            format!(
                "{}\n{}\n",
                json!({"type":"usage.record","time":timestamp,"usage":{"output":7}}),
                json!({"type":"usage.record","time":timestamp,"usage":{"output":11}})
            ),
        )
        .unwrap();
        let changed = scan_usage_statistics(&home, today).unwrap();
        assert_eq!(changed.reparsed_files, 1);
        assert_eq!(changed.statistics.total_tokens, 23.0);
        persist_cache(&home, &changed);

        fs::remove_file(main).unwrap();
        let deleted = scan_usage_statistics(&home, today).unwrap();
        assert_eq!(deleted.reparsed_files, 0);
        assert_eq!(deleted.statistics.total_tokens, 18.0);
        assert!(deleted.cache_contents.is_some());
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn skips_malformed_lines_and_rebuilds_a_corrupt_cache() {
        let home = test_home();
        let today = NaiveDate::from_ymd_opt(2026, 8, 11).unwrap();
        let timestamp = local_millis(today, 12);
        let wire = write_wire(
            &home,
            "main",
            &[json!({"type":"usage.record","time":timestamp,"usage":{"output":9}})],
        );
        let mut file = fs::OpenOptions::new().append(true).open(wire).unwrap();
        writeln!(file, "not-json").unwrap();
        let cache = home.join(CACHE_RELATIVE_PATH);
        fs::create_dir_all(cache.parent().unwrap()).unwrap();
        fs::write(cache, b"not-json").unwrap();

        let outcome = scan_usage_statistics(&home, today).unwrap();
        assert_eq!(outcome.reparsed_files, 1);
        assert_eq!(outcome.statistics.total_tokens, 9.0);
        assert!(outcome.cache_contents.is_some());
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn current_streak_expires_but_longest_streak_is_preserved() {
        let dates = [
            NaiveDate::from_ymd_opt(2026, 8, 1).unwrap(),
            NaiveDate::from_ymd_opt(2026, 8, 2).unwrap(),
            NaiveDate::from_ymd_opt(2026, 8, 3).unwrap(),
        ];
        assert_eq!(
            streaks(&dates, NaiveDate::from_ymd_opt(2026, 8, 11).unwrap()),
            (0, 3)
        );
    }
}
