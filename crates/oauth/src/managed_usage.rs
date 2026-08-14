use std::{collections::HashMap, time::SystemTime};

use chrono::DateTime;
use serde_json::{Map, Value};

use super::api_error::read_api_error_message;

const MANAGED_PREFIX: &str = "managed:";
const KIMI_CODE_PLATFORM_ID: &str = "kimi-code";
const FIXED_POINT_CENTS: u64 = 1_000_000;

pub const DEFAULT_KIMI_CODE_BASE_URL: &str = "https://api.kimi.com/coding/v1";

#[derive(Debug, Clone, PartialEq)]
pub struct UsageRow {
    pub label: String,
    pub used: u64,
    pub limit: u64,
    pub reset_hint: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BoosterWalletInfo {
    pub balance_cents: i64,
    pub total_cents: i64,
    pub monthly_charge_limit_enabled: bool,
    pub monthly_charge_limit_cents: i64,
    pub monthly_used_cents: i64,
    pub currency: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ParsedManagedUsage {
    pub summary: Option<UsageRow>,
    pub limits: Vec<UsageRow>,
    pub extra_usage: Option<BoosterWalletInfo>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum FetchManagedUsageResult {
    Ok {
        parsed: ParsedManagedUsage,
    },
    Error {
        status: Option<u16>,
        message: String,
    },
}

// Original:
//   packages/oauth/src/managed-usage.ts
//   isManagedKimiCode()
pub fn is_managed_kimi_code(provider_key: Option<&str>) -> bool {
    provider_key
        .and_then(|key| key.strip_prefix(MANAGED_PREFIX))
        .is_some_and(|platform| platform == KIMI_CODE_PLATFORM_ID)
}

// Original: kimiCodeBaseUrl()
pub fn kimi_code_base_url() -> String {
    kimi_code_base_url_from(&std::env::vars().collect())
}

pub fn kimi_code_base_url_from(environment: &HashMap<String, String>) -> String {
    environment
        .get("KIMI_CODE_BASE_URL")
        .map_or(DEFAULT_KIMI_CODE_BASE_URL, String::as_str)
        .trim_end_matches('/')
        .to_owned()
}

// Original: kimiCodeUsageUrl()
pub fn kimi_code_usage_url() -> String {
    format!("{}/usages", kimi_code_base_url())
}

// Original: isManagedKimiCodeBaseUrl()
pub fn is_managed_kimi_code_base_url(base_url: Option<&str>) -> bool {
    is_managed_kimi_code_base_url_for(base_url, &kimi_code_base_url())
}

pub fn is_managed_kimi_code_base_url_for(base_url: Option<&str>, managed_base_url: &str) -> bool {
    let managed = parse_normalized_url(managed_base_url);
    let candidate = base_url.and_then(parse_normalized_url);
    managed.is_some() && managed == candidate
}

fn parse_normalized_url(value: &str) -> Option<String> {
    let url = url::Url::parse(value).ok()?;
    let origin = url.origin().ascii_serialization().to_lowercase();
    Some(format!("{origin}{}", url.path().trim_end_matches('/')))
}

// Original: parseManagedUsagePayload()
pub fn parse_managed_usage_payload(payload: &Value) -> ParsedManagedUsage {
    let Some(record) = payload.as_object() else {
        return ParsedManagedUsage {
            summary: None,
            limits: Vec::new(),
            extra_usage: None,
        };
    };
    let summary = record
        .get("usage")
        .and_then(|value| to_usage_row(value, "Weekly limit"));
    let limits = record
        .get("limits")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .enumerate()
        .filter_map(|(index, item)| {
            let item = item.as_object()?;
            let detail = item
                .get("detail")
                .and_then(Value::as_object)
                .unwrap_or(item);
            let empty_window = Map::new();
            let window = item
                .get("window")
                .and_then(Value::as_object)
                .unwrap_or(&empty_window);
            let label = limit_label(item, detail, window, index);
            to_usage_row(&Value::Object(detail.clone()), &label)
        })
        .collect();
    let extra_usage = record.get("boosterWallet").and_then(parse_booster_wallet);
    ParsedManagedUsage {
        summary,
        limits,
        extra_usage,
    }
}

fn to_usage_row(raw: &Value, default_label: &str) -> Option<UsageRow> {
    let record = raw.as_object()?;
    let limit = to_int(record.get("limit"));
    let used = to_int(record.get("used")).or_else(|| {
        let remaining = to_int(record.get("remaining"))?;
        Some(limit?.saturating_sub(remaining))
    });
    if used.is_none() && limit.is_none() {
        return None;
    }
    let label = string_field(record, "name")
        .or_else(|| string_field(record, "title"))
        .unwrap_or_else(|| default_label.to_owned());
    Some(UsageRow {
        label,
        used: used.unwrap_or(0),
        limit: limit.unwrap_or(0),
        reset_hint: reset_hint_from(record),
    })
}

fn limit_label(
    item: &Map<String, Value>,
    detail: &Map<String, Value>,
    window: &Map<String, Value>,
    index: usize,
) -> String {
    for key in ["name", "title", "scope"] {
        if let Some(value) = item
            .get(key)
            .or_else(|| detail.get(key))
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
        {
            return value.to_owned();
        }
    }
    let duration = to_int(
        window
            .get("duration")
            .or_else(|| item.get("duration"))
            .or_else(|| detail.get("duration")),
    );
    let time_unit = window
        .get("timeUnit")
        .or_else(|| item.get("timeUnit"))
        .or_else(|| detail.get("timeUnit"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    if let Some(duration) = duration {
        if time_unit.contains("MINUTE") {
            if duration >= 60 && duration % 60 == 0 {
                return format!("{}h limit", duration / 60);
            }
            return format!("{}m limit", duration);
        }
        if time_unit.contains("HOUR") {
            return format!("{}h limit", duration);
        }
        if time_unit.contains("DAY") {
            return format!("{}d limit", duration);
        }
        return format!("{}s limit", duration);
    }
    format!("Limit #{}", index + 1)
}

fn reset_hint_from(record: &Map<String, Value>) -> Option<String> {
    for key in ["reset_at", "resetAt", "reset_time", "resetTime"] {
        if let Some(value) = record
            .get(key)
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
        {
            return Some(format_reset_time(value));
        }
    }
    for key in ["reset_in", "resetIn", "ttl", "window"] {
        if let Some(seconds) = to_int(record.get(key)).filter(|seconds| *seconds > 0) {
            return Some(format!("resets in {}", format_duration(seconds)));
        }
    }
    None
}

fn parse_booster_wallet(raw: &Value) -> Option<BoosterWalletInfo> {
    let record = raw.as_object()?;
    let balance = record.get("balance")?.as_object()?;
    if balance.get("type")?.as_str()? != "BOOSTER" {
        return None;
    }
    let amount = to_int(balance.get("amount"))?;
    if amount == 0 {
        return None;
    }
    let monthly_limit = parse_money(record.get("monthlyChargeLimit"));
    let monthly_used = parse_money(record.get("monthlyUsed"));
    let currency = monthly_limit
        .as_ref()
        .filter(|money| !money.currency.is_empty())
        .or_else(|| {
            monthly_used
                .as_ref()
                .filter(|money| !money.currency.is_empty())
        })
        .map(|money| money.currency.clone())
        .unwrap_or_else(|| "USD".to_owned());
    Some(BoosterWalletInfo {
        balance_cents: to_int(balance.get("amountLeft"))
            .map(fixed_point_to_cents)
            .unwrap_or(0),
        total_cents: fixed_point_to_cents(amount),
        monthly_charge_limit_enabled: record
            .get("monthlyChargeLimitEnabled")
            .and_then(Value::as_bool)
            == Some(true),
        monthly_charge_limit_cents: monthly_limit.as_ref().map_or(0, |money| money.cents),
        monthly_used_cents: monthly_used.as_ref().map_or(0, |money| money.cents),
        currency,
    })
}

struct Money {
    cents: i64,
    currency: String,
}

fn parse_money(raw: Option<&Value>) -> Option<Money> {
    let record = raw?.as_object()?;
    Some(Money {
        cents: to_int(record.get("priceInCents"))? as i64,
        currency: string_field(record, "currency").unwrap_or_default(),
    })
}

fn fixed_point_to_cents(value: u64) -> i64 {
    if value == 0 {
        0
    } else if value < FIXED_POINT_CENTS {
        1
    } else {
        ((value + 500_000) / FIXED_POINT_CENTS) as i64
    }
}

// Original: formatResetTime()
pub fn format_reset_time(value: &str) -> String {
    let now_millis = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64;
    format_reset_time_at(value, now_millis)
}

pub fn format_reset_time_at(value: &str, now_millis: i64) -> String {
    let normalized = trim_iso_fraction_to_millis(value);
    let Ok(parsed) = DateTime::parse_from_rfc3339(&normalized) else {
        return format!("resets at {value}");
    };
    let diff_seconds = (parsed.timestamp_millis() - now_millis).div_euclid(1_000);
    if diff_seconds <= 0 {
        "reset".to_owned()
    } else {
        format!("resets in {}", format_duration(diff_seconds as u64))
    }
}

fn trim_iso_fraction_to_millis(value: &str) -> String {
    let Some(without_z) = value.strip_suffix('Z') else {
        return value.to_owned();
    };
    let Some((base, fraction)) = without_z.split_once('.') else {
        return value.to_owned();
    };
    format!("{base}.{}Z", fraction.chars().take(3).collect::<String>())
}

// Original: formatDuration()
pub fn format_duration(total_seconds: u64) -> String {
    if total_seconds == 0 {
        return "0s".to_owned();
    }
    let days = total_seconds / 86_400;
    let hours = (total_seconds % 86_400) / 3_600;
    let minutes = (total_seconds % 3_600) / 60;
    let secs = total_seconds % 60;
    let mut parts = Vec::new();
    if days != 0 {
        parts.push(format!("{days}d"));
    }
    if hours != 0 {
        parts.push(format!("{hours}h"));
    }
    if minutes != 0 {
        parts.push(format!("{minutes}m"));
    }
    if secs != 0 && parts.is_empty() {
        parts.push(format!("{secs}s"));
    }
    if parts.is_empty() {
        "0s".to_owned()
    } else {
        parts.join(" ")
    }
}

fn to_int(value: Option<&Value>) -> Option<u64> {
    let number = match value? {
        Value::Number(number) => number.as_u64().or_else(|| {
            number
                .as_f64()
                .filter(|number| number.is_finite() && *number >= 0.0)
                .map(|number| number.trunc() as u64)
        })?,
        Value::String(value) if value.trim().is_empty() => 0,
        Value::String(value) => value.trim().parse().ok()?,
        _ => return None,
    };
    Some(number)
}

fn string_field(record: &Map<String, Value>, key: &str) -> Option<String> {
    record.get(key)?.as_str().map(str::to_owned)
}

// Original: fetchManagedUsage()
pub async fn fetch_managed_usage(
    url: &str,
    access_token: &str,
    timeout: Option<std::time::Duration>,
) -> FetchManagedUsageResult {
    let response = reqwest::Client::new()
        .get(url)
        .timeout(timeout.unwrap_or(std::time::Duration::from_secs(8)))
        .header("Authorization", format!("Bearer {access_token}"))
        .header("Accept", "application/json")
        .send()
        .await;
    let response = match response {
        Ok(response) => response,
        Err(error) if error.is_timeout() => {
            return FetchManagedUsageResult::Error {
                status: None,
                message: "Failed to fetch usage: request timed out.".to_owned(),
            };
        }
        Err(error) => {
            return FetchManagedUsageResult::Error {
                status: None,
                message: format!("Failed to fetch usage: {error}"),
            };
        }
    };

    if !response.status().is_success() {
        let status = response.status().as_u16();
        let hint = match status {
            401 => "Authorization failed. Please check your API key (try /login).".to_owned(),
            404 => "Usage endpoint not available. Try Kimi For Coding.".to_owned(),
            _ => format!("Failed to fetch usage: HTTP {status}"),
        };
        return FetchManagedUsageResult::Error {
            status: Some(status),
            message: read_api_error_message(response, &hint).await,
        };
    }

    match response.json::<Value>().await {
        Ok(payload) => FetchManagedUsageResult::Ok {
            parsed: parse_managed_usage_payload(&payload),
        },
        Err(error) => FetchManagedUsageResult::Error {
            status: None,
            message: format!("Failed to fetch usage: {error}"),
        },
    }
}

#[cfg(test)]
mod tests {
    use std::{
        io::{Read, Write},
        net::TcpListener,
        sync::{Arc, Mutex},
        thread,
        time::Duration,
    };

    use super::*;

    #[test]
    fn managed_provider_and_base_url_matching_is_strict() {
        assert!(is_managed_kimi_code(Some("managed:kimi-code")));
        for value in [None, Some(""), Some("managed:moonshot-ai"), Some("openai")] {
            assert!(!is_managed_kimi_code(value));
        }
        let environment = HashMap::from([(
            "KIMI_CODE_BASE_URL".to_owned(),
            "https://GW.example.com/coding/v1///".to_owned(),
        )]);
        assert_eq!(
            kimi_code_base_url_from(&environment),
            "https://GW.example.com/coding/v1"
        );
        assert!(is_managed_kimi_code_base_url_for(
            Some("https://gw.EXAMPLE.com/coding/v1/"),
            &kimi_code_base_url_from(&environment)
        ));
        assert!(!is_managed_kimi_code_base_url_for(
            Some("https://gw.example.com/CODING/v1"),
            "https://gw.example.com/coding/v1"
        ));
        assert!(!is_managed_kimi_code_base_url_for(
            Some("not a url"),
            DEFAULT_KIMI_CODE_BASE_URL
        ));
    }

    #[test]
    fn parses_summary_remaining_limits_and_reset_hints() {
        let parsed = parse_managed_usage_payload(&serde_json::json!({
            "usage": { "remaining": "200", "limit": 1000, "resetIn": 3661 },
            "limits": [
                { "detail": { "used": 1, "limit": 100 }, "window": { "duration": 300, "timeUnit": "MINUTE" } },
                { "name": "Daily cap", "detail": { "used": 2, "limit": 50 }, "window": { "duration": 24, "timeUnit": "HOUR" } },
                { "detail": { "used": 3 } }
            ]
        }));
        assert_eq!(
            parsed.summary,
            Some(UsageRow {
                label: "Weekly limit".to_owned(),
                used: 800,
                limit: 1_000,
                reset_hint: Some("resets in 1h 1m".to_owned())
            })
        );
        assert_eq!(
            parsed
                .limits
                .iter()
                .map(|row| row.label.as_str())
                .collect::<Vec<_>>(),
            ["5h limit", "Daily cap", "Limit #3"]
        );
    }

    #[test]
    fn parses_booster_wallet_fixed_point_money_and_currency_priority() {
        let parsed = parse_managed_usage_payload(&serde_json::json!({
            "boosterWallet": {
                "balance": { "type": "BOOSTER", "amount": "20000000000", "amountLeft": "500000" },
                "monthlyChargeLimitEnabled": true,
                "monthlyChargeLimit": { "currency": "CNY", "priceInCents": "20000" },
                "monthlyUsed": { "currency": "USD", "priceInCents": 5000 }
            }
        }));
        assert_eq!(
            parsed.extra_usage,
            Some(BoosterWalletInfo {
                balance_cents: 1,
                total_cents: 20_000,
                monthly_charge_limit_enabled: true,
                monthly_charge_limit_cents: 20_000,
                monthly_used_cents: 5_000,
                currency: "CNY".to_owned()
            })
        );
        for payload in [
            serde_json::json!({}),
            serde_json::json!({ "boosterWallet": { "balance": { "type": "OTHER", "amount": 100 } } }),
            serde_json::json!({ "boosterWallet": { "balance": { "type": "BOOSTER", "amount": 0 } } }),
        ] {
            assert_eq!(parse_managed_usage_payload(&payload).extra_usage, None);
        }
    }

    #[test]
    fn formats_durations_like_the_original_display() {
        for (seconds, expected) in [
            (0, "0s"),
            (45, "45s"),
            (60, "1m"),
            (3_661, "1h 1m"),
            (90_061, "1d 1h 1m"),
        ] {
            assert_eq!(format_duration(seconds), expected);
        }
    }

    #[test]
    fn formats_reset_times_and_trims_nanosecond_precision() {
        let now = DateTime::parse_from_rfc3339("2026-07-21T00:00:00Z")
            .expect("test timestamp")
            .timestamp_millis();
        assert_eq!(
            format_reset_time_at("2026-07-21T01:01:01.123456789Z", now),
            "resets in 1h 1m"
        );
        assert_eq!(format_reset_time_at("2026-07-20T23:00:00Z", now), "reset");
        assert_eq!(
            format_reset_time_at("not-a-date", now),
            "resets at not-a-date"
        );
    }

    fn fake_http_server(
        status: u16,
        body: &str,
        delay: Duration,
    ) -> (String, Arc<Mutex<String>>, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake usage server");
        let address = listener.local_addr().expect("fake usage address");
        let request = Arc::new(Mutex::new(String::new()));
        let recorded = Arc::clone(&request);
        let body = body.to_owned();
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept usage request");
            let mut bytes = Vec::new();
            let mut buffer = [0_u8; 4_096];
            loop {
                let count = stream.read(&mut buffer).expect("read usage request");
                if count == 0 {
                    break;
                }
                bytes.extend_from_slice(&buffer[..count]);
                if bytes.windows(4).any(|window| window == b"\r\n\r\n") {
                    break;
                }
            }
            *recorded.lock().expect("request lock") = String::from_utf8_lossy(&bytes).into_owned();
            thread::sleep(delay);
            let response = format!(
                "HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(response.as_bytes());
        });
        (format!("http://{address}/usages"), request, handle)
    }

    #[tokio::test]
    async fn fetch_sends_auth_and_accept_then_parses_success() {
        let (url, request, handle) =
            fake_http_server(200, r#"{"usage":{"used":40,"limit":1000}}"#, Duration::ZERO);
        let result = fetch_managed_usage(&url, "secret", None).await;
        handle.join().expect("usage server thread");
        let FetchManagedUsageResult::Ok { parsed } = result else {
            panic!("expected parsed usage")
        };
        assert_eq!(parsed.summary.expect("summary").used, 40);
        let request = request.lock().expect("request lock").to_ascii_lowercase();
        assert!(request.starts_with("get /usages http/1.1"));
        assert!(request.contains("authorization: bearer secret"));
        assert!(request.contains("accept: application/json"));
        assert!(!request.contains("x-msh-"));
    }

    #[tokio::test]
    async fn fetch_prefers_api_errors_and_preserves_status() {
        for (status, body, expected) in [
            (401, r#"{"message":"token revoked"}"#, "token revoked"),
            (
                403,
                r#"{"error":{"message":"account disabled"}}"#,
                "account disabled",
            ),
            (
                404,
                "",
                "Usage endpoint not available. Try Kimi For Coding.",
            ),
        ] {
            let (url, _, handle) = fake_http_server(status, body, Duration::ZERO);
            let result = fetch_managed_usage(&url, "secret", None).await;
            handle.join().expect("usage server thread");
            assert_eq!(
                result,
                FetchManagedUsageResult::Error {
                    status: Some(status),
                    message: expected.to_owned()
                }
            );
        }
    }

    #[tokio::test]
    async fn fetch_classifies_timeout_without_an_http_status() {
        let (url, _, handle) = fake_http_server(
            200,
            r#"{"usage":{"used":1,"limit":2}}"#,
            Duration::from_millis(100),
        );
        let result = fetch_managed_usage(&url, "secret", Some(Duration::from_millis(10))).await;
        handle.join().expect("usage server thread");
        assert_eq!(
            result,
            FetchManagedUsageResult::Error {
                status: None,
                message: "Failed to fetch usage: request timed out.".to_owned()
            }
        );
    }
}
