//! Managed-platform profile fetch / parse.
//!
//! Original: `packages/oauth/src/managed-userinfo.ts`.
//!
//! Only `managed:kimi-code` is supported today. The platform exposes a
//! `/me` endpoint whose snake_case payload is normalized into the
//! structured [`ManagedUserInfo`] domain model; presentation is left to
//! the consumer.

use serde_json::{Map, Value};

use super::{api_error::read_api_error_message, managed_usage::kimi_code_base_url};

// Original: kimiCodeUserInfoUrl()
//
// The cloud path stays `/me` (owned by the backend); only the local
// naming moved to "userinfo".
pub fn kimi_code_user_info_url() -> String {
    format!("{}/me", kimi_code_base_url())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedUserInfoPhone {
    pub country_code: String,
    pub number: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedUserInfo {
    pub user_id: String,
    pub nickname: String,
    pub status: String,
    pub region: String,
    pub user_level: i64,
    pub user_level_name: String,
    pub domain: i64,
    pub domain_name: String,
    pub global_id: Option<String>,
    pub bio: Option<String>,
    pub avatar: Option<String>,
    pub username: Option<String>,
    pub email: Option<String>,
    pub phone: Option<ManagedUserInfoPhone>,
    /// Backend RFC3339 timestamp, passed through verbatim.
    pub created_time: Option<String>,
    /// Backend RFC3339 timestamp, passed through verbatim.
    pub last_login_time: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum FetchManagedUserInfoResult {
    Ok {
        user_info: Box<ManagedUserInfo>,
    },
    Error {
        status: Option<u16>,
        message: String,
    },
}

// Original: parseManagedUserInfoPayload()
//
// Lenient parse: anything that is not a record, or lacks a `user_id`
// string, is malformed; every other field degrades independently.
pub fn parse_managed_user_info_payload(payload: &Value) -> Option<ManagedUserInfo> {
    let record = payload.as_object()?;
    let user_id = string_field(record, "user_id")?;
    Some(ManagedUserInfo {
        user_id,
        nickname: string_field(record, "nickname").unwrap_or_default(),
        status: string_field(record, "status").unwrap_or_default(),
        region: string_field(record, "region").unwrap_or_default(),
        user_level: int_field(record, "user_level").unwrap_or(0),
        user_level_name: string_field(record, "user_level_name").unwrap_or_default(),
        domain: int_field(record, "domain").unwrap_or(0),
        domain_name: string_field(record, "domain_name").unwrap_or_default(),
        global_id: string_field(record, "global_id"),
        bio: string_field(record, "bio"),
        avatar: string_field(record, "avatar"),
        username: string_field(record, "username"),
        email: string_field(record, "email"),
        phone: parse_phone(record.get("phone")),
        created_time: string_field(record, "created_time"),
        last_login_time: string_field(record, "last_login_time"),
    })
}

fn parse_phone(raw: Option<&Value>) -> Option<ManagedUserInfoPhone> {
    let record = raw?.as_object()?;
    let country_code = string_field(record, "country_code").unwrap_or_default();
    let number = string_field(record, "number").unwrap_or_default();
    if country_code.is_empty() && number.is_empty() {
        return None;
    }
    Some(ManagedUserInfoPhone {
        country_code,
        number,
    })
}

fn string_field(record: &Map<String, Value>, key: &str) -> Option<String> {
    record
        .get(key)?
        .as_str()
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

fn int_field(record: &Map<String, Value>, key: &str) -> Option<i64> {
    let number = match record.get(key)? {
        Value::Number(number) => number.as_f64()?,
        Value::String(value) => value.trim().parse().ok()?,
        _ => return None,
    };
    if number.is_finite() {
        Some(number.trunc() as i64)
    } else {
        None
    }
}

// Original: fetchManagedUserInfo()
pub async fn fetch_managed_user_info(
    url: &str,
    access_token: &str,
    timeout: Option<std::time::Duration>,
) -> FetchManagedUserInfoResult {
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
            return FetchManagedUserInfoResult::Error {
                status: None,
                message: "Failed to fetch profile: request timed out.".to_owned(),
            };
        }
        Err(error) => {
            return FetchManagedUserInfoResult::Error {
                status: None,
                message: format!("Failed to fetch profile: {error}"),
            };
        }
    };

    if !response.status().is_success() {
        let status = response.status().as_u16();
        let hint = match status {
            401 => "Authorization failed. Please check your API key (try /login).".to_owned(),
            404 => "Profile endpoint not available. Try Kimi For Coding.".to_owned(),
            _ => format!("Failed to fetch profile: HTTP {status}"),
        };
        return FetchManagedUserInfoResult::Error {
            status: Some(status),
            message: read_api_error_message(response, &hint).await,
        };
    }

    match response.json::<Value>().await {
        Ok(payload) => match parse_managed_user_info_payload(&payload) {
            Some(user_info) => FetchManagedUserInfoResult::Ok {
                user_info: Box::new(user_info),
            },
            None => FetchManagedUserInfoResult::Error {
                status: None,
                message: "Failed to fetch profile: malformed response.".to_owned(),
            },
        },
        Err(error) => FetchManagedUserInfoResult::Error {
            status: None,
            message: format!("Failed to fetch profile: {error}"),
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
    fn rejects_non_records_and_missing_or_blank_user_ids() {
        for payload in [
            Value::Null,
            Value::String("nope".to_owned()),
            serde_json::json!([{ "user_id": "u_1" }]),
            serde_json::json!({ "nickname": "moonwalker" }),
            serde_json::json!({ "user_id": 42 }),
            serde_json::json!({ "user_id": "" }),
        ] {
            assert_eq!(parse_managed_user_info_payload(&payload), None);
        }
    }

    #[test]
    fn parses_the_full_profile_payload_into_the_domain_model() {
        let parsed = parse_managed_user_info_payload(&serde_json::json!({
            "user_id": "u_123",
            "global_id": "u_123",
            "nickname": "moonwalker",
            "avatar": "https://example.com/avatar.png",
            "bio": "to the moon",
            "username": "moonwalker2333",
            "email": "user@example.com",
            "user_level": 30,
            "user_level_name": "Vivace",
            "domain": 1,
            "domain_name": "DOMAIN_EXAMPLE",
            "phone": { "country_code": "86", "number": "176****0000" },
            "status": "USER_STATUS_NORMAL",
            "region": "REGION_CN",
            "created_time": "2026-06-11T13:26:47.561184Z",
            "last_login_time": "2026-07-16T03:12:03.033412Z"
        }))
        .expect("full payload");
        assert_eq!(
            parsed,
            ManagedUserInfo {
                user_id: "u_123".to_owned(),
                nickname: "moonwalker".to_owned(),
                status: "USER_STATUS_NORMAL".to_owned(),
                region: "REGION_CN".to_owned(),
                user_level: 30,
                user_level_name: "Vivace".to_owned(),
                domain: 1,
                domain_name: "DOMAIN_EXAMPLE".to_owned(),
                global_id: Some("u_123".to_owned()),
                bio: Some("to the moon".to_owned()),
                avatar: Some("https://example.com/avatar.png".to_owned()),
                username: Some("moonwalker2333".to_owned()),
                email: Some("user@example.com".to_owned()),
                phone: Some(ManagedUserInfoPhone {
                    country_code: "86".to_owned(),
                    number: "176****0000".to_owned(),
                }),
                created_time: Some("2026-06-11T13:26:47.561184Z".to_owned()),
                last_login_time: Some("2026-07-16T03:12:03.033412Z".to_owned()),
            }
        );
    }

    #[test]
    fn degrades_missing_fields_to_zero_values() {
        let parsed = parse_managed_user_info_payload(&serde_json::json!({ "user_id": "u_123" }))
            .expect("minimal payload");
        assert_eq!(
            parsed,
            ManagedUserInfo {
                user_id: "u_123".to_owned(),
                nickname: String::new(),
                status: String::new(),
                region: String::new(),
                user_level: 0,
                user_level_name: String::new(),
                domain: 0,
                domain_name: String::new(),
                global_id: None,
                bio: None,
                avatar: None,
                username: None,
                email: None,
                phone: None,
                created_time: None,
                last_login_time: None,
            }
        );
    }

    #[test]
    fn drops_empty_phone_records_and_keeps_partial_ones() {
        for phone in [
            serde_json::json!({ "country_code": 86 }),
            serde_json::json!("86"),
            serde_json::json!({ "country_code": "", "number": "" }),
        ] {
            let parsed = parse_managed_user_info_payload(
                &serde_json::json!({ "user_id": "u_1", "phone": phone }),
            )
            .expect("payload");
            assert_eq!(parsed.phone, None);
        }
        let parsed = parse_managed_user_info_payload(&serde_json::json!({
            "user_id": "u_1",
            "phone": { "number": "176****0000" }
        }))
        .expect("payload");
        assert_eq!(
            parsed.phone,
            Some(ManagedUserInfoPhone {
                country_code: String::new(),
                number: "176****0000".to_owned(),
            })
        );
    }

    #[test]
    fn coerces_numeric_strings_and_truncates_fractional_levels() {
        let parsed = parse_managed_user_info_payload(&serde_json::json!({
            "user_id": "u_1",
            "user_level": "30",
            "domain": 1.9
        }))
        .expect("payload");
        assert_eq!(parsed.user_level, 30);
        assert_eq!(parsed.domain, 1);

        let parsed = parse_managed_user_info_payload(&serde_json::json!({
            "user_id": "u_1",
            "user_level": "thirty",
            "domain": "NaN"
        }))
        .expect("payload");
        assert_eq!(parsed.user_level, 0);
        assert_eq!(parsed.domain, 0);
    }

    #[test]
    fn keeps_email_only_when_it_is_a_non_empty_string() {
        let parsed = parse_managed_user_info_payload(&serde_json::json!({
            "user_id": "u_1",
            "email": "user@example.com"
        }))
        .expect("payload");
        assert_eq!(parsed.email.as_deref(), Some("user@example.com"));
        for email in [serde_json::json!(""), serde_json::json!(42)] {
            let parsed = parse_managed_user_info_payload(
                &serde_json::json!({ "user_id": "u_1", "email": email }),
            )
            .expect("payload");
            assert_eq!(parsed.email, None);
        }
    }

    fn fake_http_server(
        status: u16,
        body: &str,
        delay: Duration,
    ) -> (String, Arc<Mutex<String>>, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake userinfo server");
        let address = listener.local_addr().expect("fake userinfo address");
        let request = Arc::new(Mutex::new(String::new()));
        let recorded = Arc::clone(&request);
        let body = body.to_owned();
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept userinfo request");
            let mut bytes = Vec::new();
            let mut buffer = [0_u8; 4_096];
            loop {
                let count = stream.read(&mut buffer).expect("read userinfo request");
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
        (format!("http://{address}/me"), request, handle)
    }

    #[tokio::test]
    async fn fetch_sends_auth_and_accept_then_parses_success() {
        let (url, request, handle) = fake_http_server(
            200,
            r#"{"user_id":"u_123","nickname":"moonwalker","user_level":30}"#,
            Duration::ZERO,
        );
        let result = fetch_managed_user_info(&url, "secret", None).await;
        handle.join().expect("userinfo server thread");
        let FetchManagedUserInfoResult::Ok { user_info } = result else {
            panic!("expected parsed user info")
        };
        assert_eq!(user_info.user_id, "u_123");
        assert_eq!(user_info.nickname, "moonwalker");
        assert_eq!(user_info.user_level, 30);
        let request = request.lock().expect("request lock").to_ascii_lowercase();
        assert!(request.starts_with("get /me http/1.1"));
        assert!(request.contains("authorization: bearer secret"));
        assert!(request.contains("accept: application/json"));
        assert!(!request.contains("x-msh-"));
    }

    #[tokio::test]
    async fn fetch_prefers_api_errors_and_preserves_status() {
        for (status, body, expected) in [
            (401, r#"{"message":"token expired"}"#, "token expired"),
            (
                403,
                r#"{"error":{"message":"account disabled"}}"#,
                "account disabled",
            ),
            (
                404,
                "",
                "Profile endpoint not available. Try Kimi For Coding.",
            ),
        ] {
            let (url, _, handle) = fake_http_server(status, body, Duration::ZERO);
            let result = fetch_managed_user_info(&url, "secret", None).await;
            handle.join().expect("userinfo server thread");
            assert_eq!(
                result,
                FetchManagedUserInfoResult::Error {
                    status: Some(status),
                    message: expected.to_owned()
                }
            );
        }
    }

    #[tokio::test]
    async fn fetch_treats_a_payload_without_user_id_as_malformed() {
        let (url, _, handle) =
            fake_http_server(200, r#"{"nickname":"moonwalker"}"#, Duration::ZERO);
        let result = fetch_managed_user_info(&url, "secret", None).await;
        handle.join().expect("userinfo server thread");
        assert_eq!(
            result,
            FetchManagedUserInfoResult::Error {
                status: None,
                message: "Failed to fetch profile: malformed response.".to_owned()
            }
        );
    }

    #[tokio::test]
    async fn fetch_classifies_timeout_without_an_http_status() {
        let (url, _, handle) =
            fake_http_server(200, r#"{"user_id":"u_1"}"#, Duration::from_millis(100));
        let result = fetch_managed_user_info(&url, "secret", Some(Duration::from_millis(10))).await;
        handle.join().expect("userinfo server thread");
        assert_eq!(
            result,
            FetchManagedUserInfoResult::Error {
                status: None,
                message: "Failed to fetch profile: request timed out.".to_owned()
            }
        );
    }
}
