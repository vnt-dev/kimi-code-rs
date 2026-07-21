use std::collections::HashMap;

use chrono::{DateTime, NaiveDate, SecondsFormat, TimeDelta, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use super::{
    select::select_update_target,
    types::{RolloutBatch, UpdateManifest, UpdateTarget},
};
use crate::utils::{paths::get_update_rollout_log_file, persistence::append_jsonl_line};

pub const MAX_ROLLOUT_DELAY_SECONDS: u64 = 86_400;
const ROLLOUT_LOG_MAX_BYTES: u64 = 256 * 1024;

// Original:
//   apps/kimi-code/src/cli/update/rollout.ts
//   rolloutBucket()
pub fn rollout_bucket(device_id: &str, version: &str) -> u8 {
    let digest = Sha256::digest(format!("{device_id}:{version}").as_bytes());
    let prefix = u32::from_be_bytes([digest[0], digest[1], digest[2], digest[3]]);
    (prefix % 100) as u8
}

// Original:
//   apps/kimi-code/src/cli/update/rollout.ts
//   rolloutDelayForBucket()
pub fn rollout_delay_for_bucket(rollout: &[RolloutBatch], bucket: u8) -> u64 {
    let mut cumulative = 0_u64;
    for batch in rollout {
        cumulative = cumulative.saturating_add(u64::from(batch.percent));
        if u64::from(bucket) < cumulative {
            return batch.delay_seconds.min(MAX_ROLLOUT_DELAY_SECONDS);
        }
    }
    if rollout.is_empty() {
        0
    } else {
        MAX_ROLLOUT_DELAY_SECONDS
    }
}

pub fn rollout_delay_seconds(manifest: &UpdateManifest, device_id: &str) -> u64 {
    rollout_delay_for_bucket(
        &manifest.rollout,
        rollout_bucket(device_id, &manifest.version),
    )
}

pub fn is_rollout_eligible(manifest: &UpdateManifest, device_id: &str, now: DateTime<Utc>) -> bool {
    let Some(published_at) = parse_javascript_date(&manifest.published_at) else {
        return true;
    };
    let delay = TimeDelta::seconds(rollout_delay_seconds(manifest, device_id) as i64);
    now >= published_at + delay
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PassiveUpdateReason {
    NoLatest,
    NotNewer,
    NoManifest,
    Held,
    Eligible,
    Experimental,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PassiveUpdateDecision {
    pub target: Option<UpdateTarget>,
    pub reason: PassiveUpdateReason,
    pub bucket: Option<u8>,
    pub delay_seconds: Option<u64>,
    pub eligible_at: Option<String>,
}

// Original:
//   apps/kimi-code/src/cli/update/rollout.ts
//   decidePassiveUpdateTarget()
pub fn decide_passive_update_target(
    current_version: &str,
    latest: Option<&str>,
    manifest: Option<&UpdateManifest>,
    device_id: &str,
    now: DateTime<Utc>,
    bypass_rollout: bool,
) -> PassiveUpdateDecision {
    if bypass_rollout {
        return ungated_decision(current_version, latest, PassiveUpdateReason::Experimental);
    }

    let Some(manifest) = manifest else {
        return ungated_decision(current_version, latest, PassiveUpdateReason::NoManifest);
    };
    let Some(target) = select_update_target(current_version, Some(&manifest.version)) else {
        return empty_decision(PassiveUpdateReason::NotNewer);
    };

    let bucket = rollout_bucket(device_id, &manifest.version);
    let delay_seconds = rollout_delay_for_bucket(&manifest.rollout, bucket);
    let published_at = parse_javascript_date(&manifest.published_at);
    let eligible_at = published_at.map(|timestamp| {
        (timestamp + TimeDelta::seconds(delay_seconds as i64))
            .to_rfc3339_opts(SecondsFormat::Millis, true)
    });
    let eligible = is_rollout_eligible(manifest, device_id, now);
    PassiveUpdateDecision {
        target: eligible.then_some(target),
        reason: if eligible {
            PassiveUpdateReason::Eligible
        } else {
            PassiveUpdateReason::Held
        },
        bucket: Some(bucket),
        delay_seconds: Some(delay_seconds),
        eligible_at,
    }
}

pub fn select_passive_update_target(
    current_version: &str,
    latest: Option<&str>,
    manifest: Option<&UpdateManifest>,
    device_id: &str,
    now: DateTime<Utc>,
) -> Option<UpdateTarget> {
    decide_passive_update_target(current_version, latest, manifest, device_id, now, false).target
}

pub fn is_rollout_bypassed_by_experimental_env(env: &HashMap<String, String>) -> bool {
    env.get("KIMI_CODE_EXPERIMENTAL_FLAG").is_some_and(|value| {
        matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        )
    })
}

// Original:
//   apps/kimi-code/src/cli/update/rollout.ts
//   appendRolloutDecisionLog()
pub async fn append_rollout_decision_log(entry: &Map<String, Value>) {
    let Ok(file_path) = get_update_rollout_log_file() else {
        return;
    };
    append_rollout_decision_log_to(entry, &file_path).await;
}

pub async fn append_rollout_decision_log_to(
    entry: &Map<String, Value>,
    file_path: &std::path::Path,
) {
    let result = async {
        if let Some(parent) = file_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let size = match tokio::fs::metadata(file_path).await {
            Ok(metadata) => metadata.len(),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => 0,
            Err(error) => return Err(error.into()),
        };
        if size > ROLLOUT_LOG_MAX_BYTES {
            let line = serde_json::to_string(entry)? + "\n";
            tokio::fs::write(file_path, line).await?;
        } else {
            append_jsonl_line(
                file_path,
                |value| match value {
                    Value::Object(_) => Ok(value),
                    _ => Err("rollout log entry must be an object".to_owned()),
                },
                entry,
            )
            .await?;
        }
        Ok::<(), crate::utils::persistence::PersistenceError>(())
    }
    .await;
    let _ = result;
}

fn ungated_decision(
    current_version: &str,
    latest: Option<&str>,
    visible_reason: PassiveUpdateReason,
) -> PassiveUpdateDecision {
    let Some(latest) = latest else {
        return empty_decision(PassiveUpdateReason::NoLatest);
    };
    let target = select_update_target(current_version, Some(latest));
    PassiveUpdateDecision {
        reason: if target.is_some() {
            visible_reason
        } else {
            PassiveUpdateReason::NotNewer
        },
        target,
        bucket: None,
        delay_seconds: None,
        eligible_at: None,
    }
}

fn empty_decision(reason: PassiveUpdateReason) -> PassiveUpdateDecision {
    PassiveUpdateDecision {
        target: None,
        reason,
        bucket: None,
        delay_seconds: None,
        eligible_at: None,
    }
}

fn parse_javascript_date(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .map(|timestamp| timestamp.to_utc())
        .ok()
        .or_else(|| {
            NaiveDate::parse_from_str(value, "%Y-%m-%d")
                .ok()?
                .and_hms_opt(0, 0, 0)
                .map(|timestamp| timestamp.and_utc())
        })
}

#[cfg(test)]
mod tests {
    use std::{
        path::{Path, PathBuf},
        sync::atomic::{AtomicU64, Ordering},
    };

    use chrono::TimeZone;

    use super::*;

    static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

    const PUBLISHED_AT: &str = "2026-06-12T00:00:00.000Z";

    fn standard_rollout() -> Vec<RolloutBatch> {
        vec![
            RolloutBatch {
                percent: 30,
                delay_seconds: 0,
            },
            RolloutBatch {
                percent: 30,
                delay_seconds: 43_200,
            },
            RolloutBatch {
                percent: 40,
                delay_seconds: 86_400,
            },
        ]
    }

    fn manifest(rollout: Vec<RolloutBatch>) -> UpdateManifest {
        UpdateManifest {
            version: "2.0.0".to_owned(),
            published_at: PUBLISHED_AT.to_owned(),
            rollout,
        }
    }

    fn seconds_after_publish(seconds: i64) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 6, 12, 0, 0, 0)
            .single()
            .expect("valid timestamp")
            + TimeDelta::seconds(seconds)
    }

    fn temp_file() -> PathBuf {
        let id = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir()
            .join(format!("kimi-rollout-log-{}-{id}", std::process::id()))
            .join("updates")
            .join("rollout.log")
    }

    async fn cleanup(file: &Path) {
        if let Some(root) = file.parent().and_then(Path::parent) {
            let _ = tokio::fs::remove_dir_all(root).await;
        }
    }

    #[test]
    fn bucket_matches_original_pinned_sha256_vectors() {
        assert_eq!(rollout_bucket("device-a", "1.0.0"), 65);
        assert_eq!(rollout_bucket("device-b", "1.0.0"), 76);
        assert_eq!(rollout_bucket("fixed-device", "2.0.0"), 26);
        assert_eq!(rollout_bucket("device-a", "1.0.1"), 79);
    }

    #[test]
    fn maps_batch_boundaries_and_clamps_delays() {
        let rollout = standard_rollout();
        assert_eq!(rollout_delay_for_bucket(&rollout, 0), 0);
        assert_eq!(rollout_delay_for_bucket(&rollout, 29), 0);
        assert_eq!(rollout_delay_for_bucket(&rollout, 30), 43_200);
        assert_eq!(rollout_delay_for_bucket(&rollout, 59), 43_200);
        assert_eq!(rollout_delay_for_bucket(&rollout, 60), 86_400);
        assert_eq!(rollout_delay_for_bucket(&rollout, 99), 86_400);
        assert_eq!(
            rollout_delay_for_bucket(
                &[RolloutBatch {
                    percent: 100,
                    delay_seconds: 999_999,
                }],
                50,
            ),
            MAX_ROLLOUT_DELAY_SECONDS
        );
        assert_eq!(rollout_delay_for_bucket(&[], 99), 0);
        assert_eq!(
            rollout_delay_for_bucket(
                &[RolloutBatch {
                    percent: 30,
                    delay_seconds: 0,
                }],
                30,
            ),
            MAX_ROLLOUT_DELAY_SECONDS
        );
    }

    #[test]
    fn applies_eligibility_at_the_exact_delay_boundary_and_fails_open() {
        let delayed = manifest(vec![RolloutBatch {
            percent: 100,
            delay_seconds: 43_200,
        }]);
        assert!(!is_rollout_eligible(
            &delayed,
            "device-a",
            seconds_after_publish(43_200) - TimeDelta::milliseconds(1)
        ));
        assert!(is_rollout_eligible(
            &delayed,
            "device-a",
            seconds_after_publish(43_200)
        ));
        let mut invalid = delayed;
        invalid.published_at = "not-a-date".to_owned();
        assert!(is_rollout_eligible(
            &invalid,
            "device-a",
            seconds_after_publish(-999_999)
        ));
    }

    #[test]
    fn decides_held_eligible_and_legacy_visibility() {
        let held = manifest(vec![RolloutBatch {
            percent: 100,
            delay_seconds: 86_400,
        }]);
        let decision = decide_passive_update_target(
            "1.0.0",
            Some("2.0.0"),
            Some(&held),
            "device-a",
            seconds_after_publish(60),
            false,
        );
        assert_eq!(decision.reason, PassiveUpdateReason::Held);
        assert_eq!(decision.target, None);
        assert_eq!(decision.delay_seconds, Some(86_400));
        assert_eq!(
            decision.eligible_at.as_deref(),
            Some("2026-06-13T00:00:00.000Z")
        );

        let eligible = decide_passive_update_target(
            "1.0.0",
            Some("2.0.0"),
            Some(&held),
            "device-a",
            seconds_after_publish(86_400),
            false,
        );
        assert_eq!(eligible.reason, PassiveUpdateReason::Eligible);
        assert_eq!(eligible.target.expect("target").version, "2.0.0");

        let legacy = decide_passive_update_target(
            "1.0.0",
            Some("2.0.0"),
            None,
            "device-a",
            seconds_after_publish(60),
            false,
        );
        assert_eq!(legacy.reason, PassiveUpdateReason::NoManifest);
        assert_eq!(legacy.target.expect("target").version, "2.0.0");
    }

    #[test]
    fn experimental_bypass_keeps_no_latest_and_not_newer_cases() {
        let now = seconds_after_publish(60);
        let held = manifest(vec![RolloutBatch {
            percent: 100,
            delay_seconds: 86_400,
        }]);
        assert_eq!(
            decide_passive_update_target(
                "1.0.0",
                Some("2.0.0"),
                Some(&held),
                "device-a",
                now,
                true,
            )
            .reason,
            PassiveUpdateReason::Experimental
        );
        assert_eq!(
            decide_passive_update_target(
                "2.0.0",
                Some("2.0.0"),
                Some(&held),
                "device-a",
                now,
                true,
            )
            .reason,
            PassiveUpdateReason::NotNewer
        );
        assert_eq!(
            decide_passive_update_target("1.0.0", None, None, "device-a", now, true,).reason,
            PassiveUpdateReason::NoLatest
        );
    }

    #[test]
    fn recognizes_only_trimmed_truthy_experimental_values() {
        for value in ["1", "true", "YES", " on "] {
            let env = HashMap::from([("KIMI_CODE_EXPERIMENTAL_FLAG".to_owned(), value.to_owned())]);
            assert!(is_rollout_bypassed_by_experimental_env(&env));
        }
        for value in ["", "0", "off"] {
            let env = HashMap::from([("KIMI_CODE_EXPERIMENTAL_FLAG".to_owned(), value.to_owned())]);
            assert!(!is_rollout_bypassed_by_experimental_env(&env));
        }
    }

    #[tokio::test]
    async fn appends_one_compact_json_line_per_rollout_decision() {
        let file = temp_file();
        append_rollout_decision_log_to(
            &serde_json::from_value(serde_json::json!({
                "phase": "startup-cache", "reason": "held"
            }))
            .expect("entry"),
            &file,
        )
        .await;
        append_rollout_decision_log_to(
            &serde_json::from_value(serde_json::json!({
                "phase": "prompt-refresh", "reason": "eligible"
            }))
            .expect("entry"),
            &file,
        )
        .await;

        let content = tokio::fs::read_to_string(&file).await.expect("rollout log");
        let lines = content.lines().collect::<Vec<_>>();
        assert_eq!(lines.len(), 2);
        assert_eq!(
            serde_json::from_str::<Value>(lines[0]).expect("first line")["reason"],
            "held"
        );
        assert_eq!(
            serde_json::from_str::<Value>(lines[1]).expect("second line")["reason"],
            "eligible"
        );
        cleanup(&file).await;
    }

    #[tokio::test]
    async fn resets_an_oversized_log_and_swallows_io_failures() {
        let file = temp_file();
        tokio::fs::create_dir_all(file.parent().expect("parent"))
            .await
            .expect("parent");
        tokio::fs::write(&file, vec![b'x'; 300 * 1024])
            .await
            .expect("oversized log");
        let entry =
            serde_json::from_value(serde_json::json!({ "reason": "eligible" })).expect("entry");
        append_rollout_decision_log_to(&entry, &file).await;
        let content = tokio::fs::read_to_string(&file).await.expect("reset log");
        assert!(content.len() < 1_024);
        assert_eq!(
            serde_json::from_str::<Value>(content.trim()).expect("reset entry")["reason"],
            "eligible"
        );

        let blocked_path = file.join("not-a-directory").join("rollout.log");
        append_rollout_decision_log_to(&entry, &blocked_path).await;
        cleanup(&file).await;
    }
}
