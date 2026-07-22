use std::time::{Duration, SystemTime};

use chrono::{DateTime, Utc};
use semver::Version;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::tui::types::{BannerDisplay, BannerState};

use super::state::BannerDisplayState;

pub const KIMI_CODE_TIPS_BANNER_URL: &str = "https://cdn.kimi.com/kimi-code-tips/tips.json";
pub const DEFAULT_COOLDOWN_TTL_HOURS: f64 = 24.0;
const BANNER_TIMEOUT: Duration = Duration::from_secs(3);
const HOUR_MILLISECONDS: i64 = 60 * 60 * 1_000;

pub struct BannerProviderLoadOptions<'a> {
    pub state: Option<&'a BannerDisplayState>,
    pub now: DateTime<Utc>,
    pub random: &'a mut dyn FnMut() -> f64,
}

#[derive(Debug, Clone)]
pub struct BannerProvider {
    client_version: String,
    url: String,
    client: reqwest::Client,
}

impl BannerProvider {
    pub fn new(client_version: impl Into<String>) -> Self {
        Self::with_url(client_version, KIMI_CODE_TIPS_BANNER_URL)
    }

    pub fn with_url(client_version: impl Into<String>, url: impl Into<String>) -> Self {
        Self {
            client_version: client_version.into(),
            url: url.into(),
            client: reqwest::Client::new(),
        }
    }

    // Original:
    //   apps/kimi-code/src/tui/banner/banner-provider.ts
    //   BannerProvider.load()
    pub async fn load(&self) -> Option<BannerState> {
        let now = DateTime::<Utc>::from(SystemTime::now());
        let mut random = runtime_random;
        self.load_with(BannerProviderLoadOptions {
            state: None,
            now,
            random: &mut random,
        })
        .await
    }

    pub async fn load_with(&self, options: BannerProviderLoadOptions<'_>) -> Option<BannerState> {
        let response = self
            .client
            .get(&self.url)
            .timeout(BANNER_TIMEOUT)
            .send()
            .await
            .ok()?;
        if !response.status().is_success() {
            return None;
        }
        let json = response.json::<Value>().await.ok()?;
        match options.state {
            Some(state) => select_displayable_banner(
                &json,
                &self.client_version,
                options.now,
                options.random,
                state,
            ),
            None => select_banner_state(&json, &self.client_version, options.now, options.random),
        }
    }
}

// Original: selectBannerState()
pub fn select_banner_state(
    json: &Value,
    client_version: &str,
    now: DateTime<Utc>,
    random: &mut dyn FnMut() -> f64,
) -> Option<BannerState> {
    pick_active_banner(json, client_version, now)
        .or_else(|| pick_random_candidate(pick_fallback_candidates(json, client_version), random))
}

// Original: selectDisplayableBanner()
pub fn select_displayable_banner(
    json: &Value,
    client_version: &str,
    now: DateTime<Utc>,
    random: &mut dyn FnMut() -> f64,
    state: &BannerDisplayState,
) -> Option<BannerState> {
    let active = pick_active_banner(json, client_version, now);
    if active
        .as_ref()
        .is_some_and(|banner| should_display_banner(banner, state, now))
    {
        return active;
    }
    let candidates = pick_fallback_candidates(json, client_version)
        .into_iter()
        .filter(|banner| should_display_banner(banner, state, now))
        .collect();
    pick_random_candidate(candidates, random)
}

// Original: shouldDisplayBanner()
pub fn should_display_banner(
    banner: &BannerState,
    state: &BannerDisplayState,
    now: DateTime<Utc>,
) -> bool {
    if banner.display == BannerDisplay::Always {
        return true;
    }
    let Some(last_shown_at) = state
        .shown
        .get(&banner.key)
        .and_then(|record| parse_date(&Value::String(record.last_shown_at.clone())))
    else {
        return true;
    };
    if banner.display == BannerDisplay::Once {
        return false;
    }
    let ttl = banner
        .ttl_hours
        .filter(|hours| hours.is_finite() && *hours > 0.0)
        .unwrap_or(DEFAULT_COOLDOWN_TTL_HOURS);
    now.signed_duration_since(last_shown_at).num_milliseconds() as f64
        >= ttl * HOUR_MILLISECONDS as f64
}

fn pick_active_banner(
    json: &Value,
    client_version: &str,
    now: DateTime<Utc>,
) -> Option<BannerState> {
    let object = json.as_object()?;
    if object.get("banner_enabled").and_then(Value::as_bool) != Some(true)
        || !meets_version(json, client_version)
    {
        return None;
    }
    let start = object.get("banner_start_time").and_then(parse_date);
    let end = object.get("banner_end_time").and_then(parse_date);
    if start.is_some_and(|start| now < start) || end.is_some_and(|end| now > end) {
        return None;
    }
    let main_text = normalize_text(object.get("banner_maintext"))?;
    let display = parse_banner_display(object.get("banner_display"));
    Some(to_banner_state(BannerCandidate {
        id: object.get("banner_id"),
        tag: object.get("banner_title"),
        main_text,
        sub_text: object.get("banner_subtext"),
        display,
        ttl_hours: (display == BannerDisplay::Cooldown)
            .then(|| parse_ttl_hours(object.get("banner_display_ttl_hours"))),
        start_time: object.get("banner_start_time"),
        end_time: object.get("banner_end_time"),
    }))
}

fn pick_fallback_candidates(json: &Value, client_version: &str) -> Vec<BannerState> {
    let Some(object) = json.as_object() else {
        return Vec::new();
    };
    if object
        .get("banner_fallback_enabled")
        .and_then(Value::as_bool)
        != Some(true)
    {
        return Vec::new();
    }
    object
        .get("banner_fallback_list")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|item| {
            let item_object = item.as_object()?;
            if item_object.get("enabled").and_then(Value::as_bool) != Some(true)
                || !meets_version(item, client_version)
            {
                return None;
            }
            let main_text = normalize_text(item_object.get("banner_maintext"))?;
            let display = parse_banner_display(item_object.get("banner_display"));
            Some(to_banner_state(BannerCandidate {
                id: item_object.get("banner_id"),
                tag: item_object.get("banner_title"),
                main_text,
                sub_text: item_object.get("banner_subtext"),
                display,
                ttl_hours: (display == BannerDisplay::Cooldown)
                    .then(|| parse_ttl_hours(item_object.get("banner_display_ttl_hours"))),
                start_time: None,
                end_time: None,
            }))
        })
        .collect()
}

struct BannerCandidate<'a> {
    id: Option<&'a Value>,
    tag: Option<&'a Value>,
    main_text: String,
    sub_text: Option<&'a Value>,
    display: BannerDisplay,
    ttl_hours: Option<f64>,
    start_time: Option<&'a Value>,
    end_time: Option<&'a Value>,
}

fn to_banner_state(candidate: BannerCandidate<'_>) -> BannerState {
    let tag = normalize_text(candidate.tag);
    let sub_text = normalize_text(candidate.sub_text);
    let start_time = normalize_text(candidate.start_time);
    let end_time = normalize_text(candidate.end_time);
    let key = normalize_text(candidate.id).unwrap_or_else(|| {
        hash_banner_identity(
            tag.as_deref(),
            &candidate.main_text,
            sub_text.as_deref(),
            start_time.as_deref(),
            end_time.as_deref(),
            candidate.display,
            candidate.ttl_hours,
        )
    });
    BannerState {
        key,
        tag,
        main_text: candidate.main_text,
        sub_text,
        display: candidate.display,
        ttl_hours: candidate.ttl_hours,
    }
}

fn meets_version(json: &Value, client_version: &str) -> bool {
    meets_version_constraint(json.get("banner_min_version"), client_version, |a, b| {
        a >= b
    }) && meets_version_constraint(json.get("banner_max_version"), client_version, |a, b| a < b)
        && meets_version_constraint(json.get("banner_version"), client_version, |a, b| a == b)
}

fn meets_version_constraint(
    constraint: Option<&Value>,
    client_version: &str,
    compare: impl FnOnce(&Version, &Version) -> bool,
) -> bool {
    let Some(constraint) = constraint else {
        return true;
    };
    if constraint.is_null() {
        return true;
    }
    let Some(target) = constraint.as_str() else {
        return true;
    };
    if target.is_empty() {
        return true;
    }
    let (Ok(current), Ok(target)) = (Version::parse(client_version), Version::parse(target)) else {
        return false;
    };
    compare(&current, &target)
}

fn parse_date(value: &Value) -> Option<DateTime<Utc>> {
    let raw = value.as_str()?;
    if raw.is_empty() {
        return None;
    }
    let normalized = if raw.ends_with('Z') || has_numeric_offset(raw) {
        raw.to_owned()
    } else {
        format!("{raw}Z")
    };
    DateTime::parse_from_rfc3339(&normalized)
        .ok()
        .map(|date| date.with_timezone(&Utc))
}

fn has_numeric_offset(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() >= 6
        && matches!(bytes[bytes.len() - 6], b'+' | b'-')
        && bytes[bytes.len() - 3] == b':'
        && bytes[bytes.len() - 5..bytes.len() - 3]
            .iter()
            .all(u8::is_ascii_digit)
        && bytes[bytes.len() - 2..].iter().all(u8::is_ascii_digit)
}

fn normalize_text(value: Option<&Value>) -> Option<String> {
    let trimmed = value?.as_str()?.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_owned())
}

fn parse_banner_display(value: Option<&Value>) -> BannerDisplay {
    match value.and_then(Value::as_str) {
        Some("once") => BannerDisplay::Once,
        Some("cooldown") => BannerDisplay::Cooldown,
        _ => BannerDisplay::Always,
    }
}

fn parse_ttl_hours(value: Option<&Value>) -> f64 {
    value
        .and_then(Value::as_f64)
        .filter(|hours| hours.is_finite() && *hours > 0.0)
        .unwrap_or(DEFAULT_COOLDOWN_TTL_HOURS)
}

fn hash_banner_identity(
    tag: Option<&str>,
    main_text: &str,
    sub_text: Option<&str>,
    start_time: Option<&str>,
    end_time: Option<&str>,
    display: BannerDisplay,
    ttl_hours: Option<f64>,
) -> String {
    let display = match display {
        BannerDisplay::Always => "always",
        BannerDisplay::Once => "once",
        BannerDisplay::Cooldown => "cooldown",
    };
    let raw = serde_json::to_string(&json!([
        tag.unwrap_or(""),
        main_text,
        sub_text.unwrap_or(""),
        start_time.unwrap_or(""),
        end_time.unwrap_or(""),
        display,
        ttl_hours.map_or(Value::String(String::new()), Value::from),
    ]))
    .expect("banner hash input is serializable");
    let digest = Sha256::digest(raw.as_bytes());
    let mut output = String::with_capacity(32);
    for byte in digest.iter().take(16) {
        use std::fmt::Write as _;
        write!(&mut output, "{byte:02x}").expect("writing to String cannot fail");
    }
    output
}

fn pick_random_candidate(
    candidates: Vec<BannerState>,
    random: &mut dyn FnMut() -> f64,
) -> Option<BannerState> {
    if candidates.is_empty() {
        return None;
    }
    let index = (random() * candidates.len() as f64).floor();
    if !index.is_finite() || index < 0.0 {
        return None;
    }
    candidates.into_iter().nth(index as usize)
}

fn runtime_random() -> f64 {
    let value = uuid::Uuid::new_v4().as_u128();
    value as f64 / u128::MAX as f64
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use chrono::TimeZone;

    use super::*;
    use crate::tui::banner::state::BannerDisplayRecord;

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 6, 15, 4, 0, 0)
            .single()
            .expect("date")
    }

    fn select(value: Value, version: &str, random: f64) -> Option<BannerState> {
        let mut rng = || random;
        select_banner_state(&value, version, now(), &mut rng)
    }

    #[test]
    fn selects_active_banner_and_normalizes_content() {
        let banner = select(
            json!({
                "banner_enabled": true,
                "banner_title": "  New  ",
                "banner_maintext": " Active ",
                "banner_subtext": " Details "
            }),
            "0.14.0",
            0.0,
        )
        .expect("banner");
        assert_eq!(banner.tag.as_deref(), Some("New"));
        assert_eq!(banner.main_text, "Active");
        assert_eq!(banner.sub_text.as_deref(), Some("Details"));
        assert_eq!(banner.display, BannerDisplay::Always);
        assert_eq!(banner.ttl_hours, None);
    }

    #[test]
    fn applies_time_window_and_semver_constraints() {
        let windowed = json!({
            "banner_enabled": true,
            "banner_maintext": "Upgrade",
            "banner_start_time": "2026-06-01T00:00:00+08:00",
            "banner_end_time": "2026-06-30T00:00:00+08:00",
            "banner_min_version": "0.13.0",
            "banner_max_version": "0.15.0"
        });
        assert!(select(windowed.clone(), "0.12.9", 0.0).is_none());
        assert!(select(windowed.clone(), "0.13.0", 0.0).is_some());
        assert!(select(windowed, "0.15.0", 0.0).is_none());
        assert!(select(
            json!({"banner_enabled": true, "banner_maintext": "Broken", "banner_version": "bad"}),
            "0.14.0",
            0.0,
        )
        .is_none());
    }

    #[test]
    fn picks_only_enabled_matching_fallback_by_random_index() {
        let banner = select(
            json!({
                "banner_enabled": false,
                "banner_fallback_enabled": true,
                "banner_fallback_list": [
                    {"enabled": true, "banner_maintext": "First"},
                    {"enabled": false, "banner_maintext": "Hidden"},
                    {"enabled": true, "banner_maintext": "Second", "banner_version": "0.14.0"}
                ]
            }),
            "0.14.0",
            0.75,
        )
        .expect("fallback");
        assert_eq!(banner.main_text, "Second");
    }

    #[test]
    fn explicit_id_wins_and_derived_identity_is_stable_and_content_sensitive() {
        let explicit = select(
            json!({"banner_enabled": true, "banner_id": " stable ", "banner_maintext": "Text"}),
            "0.14.0",
            0.0,
        )
        .expect("explicit");
        assert_eq!(explicit.key, "stable");
        let first = select(
            json!({"banner_enabled": true, "banner_maintext": "Text"}),
            "0.14.0",
            0.0,
        )
        .expect("first");
        let second = select(
            json!({"banner_enabled": true, "banner_maintext": "Text"}),
            "0.14.0",
            0.0,
        )
        .expect("second");
        let changed = select(
            json!({"banner_enabled": true, "banner_maintext": "Changed"}),
            "0.14.0",
            0.0,
        )
        .expect("changed");
        assert_eq!(first.key, second.key);
        assert_ne!(first.key, changed.key);
        assert_eq!(first.key.len(), 32);
        assert_eq!(first.key, "c6ad5830f162b5ac3b1cf9d1843a4ec2");
    }

    fn display_state(key: &str, timestamp: &str) -> BannerDisplayState {
        BannerDisplayState {
            version: 1,
            shown: HashMap::from([(
                key.to_owned(),
                BannerDisplayRecord {
                    last_shown_at: timestamp.to_owned(),
                },
            )]),
        }
    }

    #[test]
    fn once_and_cooldown_display_rules_match_saved_state() {
        let once = BannerState {
            key: "once".to_owned(),
            tag: None,
            main_text: "Once".to_owned(),
            sub_text: None,
            display: BannerDisplay::Once,
            ttl_hours: None,
        };
        assert!(should_display_banner(
            &once,
            &BannerDisplayState {
                version: 1,
                shown: HashMap::new()
            },
            now()
        ));
        assert!(!should_display_banner(
            &once,
            &display_state("once", "2026-06-14T04:00:00Z"),
            now()
        ));
        let cooldown = BannerState {
            key: "cool".to_owned(),
            display: BannerDisplay::Cooldown,
            ttl_hours: Some(2.0),
            ..once
        };
        assert!(!should_display_banner(
            &cooldown,
            &display_state("cool", "2026-06-15T03:00:01Z"),
            now()
        ));
        assert!(should_display_banner(
            &cooldown,
            &display_state("cool", "2026-06-15T02:00:00Z"),
            now()
        ));
    }

    #[test]
    fn displayable_selection_skips_suppressed_active_and_fallbacks() {
        let value = json!({
            "banner_enabled": true,
            "banner_id": "active",
            "banner_maintext": "Active",
            "banner_display": "once",
            "banner_fallback_enabled": true,
            "banner_fallback_list": [
                {"enabled": true, "banner_id": "seen", "banner_maintext": "Seen", "banner_display": "once"},
                {"enabled": true, "banner_id": "fresh", "banner_maintext": "Fresh", "banner_display": "once"}
            ]
        });
        let state = BannerDisplayState {
            version: 1,
            shown: HashMap::from([
                (
                    "active".to_owned(),
                    BannerDisplayRecord {
                        last_shown_at: "2026-06-14T00:00:00Z".to_owned(),
                    },
                ),
                (
                    "seen".to_owned(),
                    BannerDisplayRecord {
                        last_shown_at: "2026-06-14T00:00:00Z".to_owned(),
                    },
                ),
            ]),
        };
        let mut rng = || 0.0;
        let banner = select_displayable_banner(&value, "0.14.0", now(), &mut rng, &state)
            .expect("fresh fallback");
        assert_eq!(banner.key, "fresh");
    }
}
