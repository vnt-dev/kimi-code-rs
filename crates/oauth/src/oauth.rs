use std::{
    collections::HashSet,
    error::Error,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use indexmap::IndexMap;
use serde_json::{Map, Value};

use super::{
    api_error::extract_api_error_message,
    errors::OAuthError,
    types::{DeviceAuthorization, OAuthFlowConfig, TokenInfo},
};

pub type DeviceHeaders = IndexMap<String, String>;

const DEFAULT_HTTP_TIMEOUT: Duration = Duration::from_secs(30);
const RETRYABLE_STATUSES: [u16; 5] = [429, 500, 502, 503, 504];

#[derive(Debug, Clone, PartialEq)]
pub enum DevicePollResult {
    Success(TokenInfo),
    Pending {
        error_code: String,
        description: String,
    },
    Expired,
    Denied {
        description: String,
    },
}

#[derive(Clone, Copy)]
pub struct RefreshOptions<'a> {
    pub device_headers: Option<&'a DeviceHeaders>,
    pub max_retries: usize,
    pub backoff: fn(usize) -> Duration,
}

impl Default for RefreshOptions<'_> {
    fn default() -> Self {
        Self {
            device_headers: None,
            max_retries: 3,
            backoff: exponential_backoff,
        }
    }
}

fn exponential_backoff(attempt: usize) -> Duration {
    let seconds = 1_u64.checked_shl(attempt as u32).unwrap_or(u64::MAX);
    Duration::from_secs(seconds)
}

#[derive(Debug)]
struct FormResponse {
    status: u16,
    data: Map<String, Value>,
}

// Original:
//   packages/oauth/src/oauth.ts
//   requestDeviceAuthorization()
pub async fn request_device_authorization(
    config: &OAuthFlowConfig,
    device_headers: Option<&DeviceHeaders>,
) -> Result<DeviceAuthorization, OAuthError> {
    let url = endpoint(config, "/api/oauth/device_authorization");
    let response = post_form(
        &url,
        &[("client_id", config.client_id.as_str())],
        device_headers,
    )
    .await?;

    if response.status != 200 {
        return Err(OAuthError::new(format!(
            "Device authorization failed (HTTP {}): {}",
            response.status,
            pick_error_detail(&response.data)
        )));
    }

    Ok(DeviceAuthorization {
        user_code: required_string(
            &response.data,
            "user_code",
            "Device authorization response missing user_code",
        )?,
        device_code: required_string(
            &response.data,
            "device_code",
            "Device authorization response missing device_code",
        )?,
        verification_uri: string_value(&response.data, "verification_uri").unwrap_or_default(),
        verification_uri_complete: required_string(
            &response.data,
            "verification_uri_complete",
            "Device authorization response missing verification_uri_complete",
        )?,
        expires_in: response.data.get("expires_in").and_then(js_number),
        interval: response
            .data
            .get("interval")
            .and_then(js_number)
            .unwrap_or(5),
    })
}

// Original: pollDeviceToken()
pub async fn poll_device_token(
    config: &OAuthFlowConfig,
    device_code: &str,
    device_headers: Option<&DeviceHeaders>,
) -> Result<DevicePollResult, OAuthError> {
    let url = endpoint(config, "/api/oauth/token");
    let response = post_form(
        &url,
        &[
            ("client_id", config.client_id.as_str()),
            ("device_code", device_code),
            ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
        ],
        device_headers,
    )
    .await?;

    if response.status == 200 && string_value(&response.data, "access_token").is_some() {
        return token_from_response(&response.data).map(DevicePollResult::Success);
    }
    if response.status >= 500 {
        return Err(OAuthError::new(format!(
            "Device token polling server error (HTTP {}): {}",
            response.status,
            pick_error_detail(&response.data)
        )));
    }

    let error_code =
        string_value(&response.data, "error").unwrap_or_else(|| "unknown_error".to_owned());
    let detail = extract_api_error_message(&Value::Object(response.data.clone()));
    let description = string_value(&response.data, "error_description")
        .or_else(|| detail.clone())
        .unwrap_or_default();
    match error_code.as_str() {
        "authorization_pending" | "slow_down" => Ok(DevicePollResult::Pending {
            error_code,
            description,
        }),
        "expired_token" => Ok(DevicePollResult::Expired),
        "access_denied" => Ok(DevicePollResult::Denied { description }),
        _ => Err(OAuthError::new(format!(
            "Device token polling failed (HTTP {}): {}",
            response.status,
            detail.unwrap_or_else(|| format!("{error_code} {description}"))
        ))),
    }
}

// Original: refreshAccessToken()
pub async fn refresh_access_token(
    config: &OAuthFlowConfig,
    refresh_token: &str,
    options: RefreshOptions<'_>,
) -> Result<TokenInfo, OAuthError> {
    let url = endpoint(config, "/api/oauth/token");
    let mut last_error = None;

    for attempt in 0..options.max_retries {
        let response = post_form(
            &url,
            &[
                ("client_id", config.client_id.as_str()),
                ("grant_type", "refresh_token"),
                ("refresh_token", refresh_token),
            ],
            options.device_headers,
        )
        .await;

        let response = match response {
            Ok(response) => response,
            Err(error) => {
                if attempt + 1 < options.max_retries {
                    tokio::time::sleep((options.backoff)(attempt)).await;
                    continue;
                }
                return Err(error);
            }
        };

        if response.status == 200 && string_value(&response.data, "access_token").is_some() {
            return token_from_response(&response.data);
        }

        let error_code = string_value(&response.data, "error").unwrap_or_default();
        let detail = extract_api_error_message(&Value::Object(response.data));
        if matches!(response.status, 401 | 403) || error_code == "invalid_grant" {
            return Err(OAuthError::unauthorized(
                detail.unwrap_or_else(|| "Token refresh unauthorized.".to_owned()),
            ));
        }

        let description =
            detail.unwrap_or_else(|| format!("Token refresh failed (HTTP {}).", response.status));
        if RETRYABLE_STATUSES.contains(&response.status) {
            last_error = Some(OAuthError::retryable_refresh(description));
            if attempt + 1 < options.max_retries {
                tokio::time::sleep((options.backoff)(attempt)).await;
                continue;
            }
        } else {
            return Err(OAuthError::new(description));
        }
    }

    Err(last_error.unwrap_or_else(|| OAuthError::new("Token refresh failed after retries.")))
}

async fn post_form(
    url: &str,
    parameters: &[(&str, &str)],
    device_headers: Option<&DeviceHeaders>,
) -> Result<FormResponse, OAuthError> {
    let body = url::form_urlencoded::Serializer::new(String::new())
        .extend_pairs(parameters.iter().copied())
        .finish();
    let client = reqwest::Client::new();
    let mut request = client
        .post(url)
        .timeout(DEFAULT_HTTP_TIMEOUT)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .header("Accept", "application/json")
        .body(body);
    if let Some(headers) = device_headers {
        for (name, value) in headers {
            request = request.header(name, value);
        }
    }

    let response = request.send().await.map_err(|error| {
        let description = describe_error_chain(&error);
        OAuthError::connection(
            format!("OAuth request to {url} failed: {description}"),
            error,
        )
    })?;
    let status = response.status().as_u16();
    let data = response
        .json::<Value>()
        .await
        .ok()
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default();
    Ok(FormResponse { status, data })
}

fn endpoint(config: &OAuthFlowConfig, path: &str) -> String {
    format!("{}{path}", config.oauth_host.trim_end_matches('/'))
}

fn token_from_response(payload: &Map<String, Value>) -> Result<TokenInfo, OAuthError> {
    let access_token = required_string(
        payload,
        "access_token",
        "OAuth response missing access_token",
    )?;
    let refresh_token = required_string(
        payload,
        "refresh_token",
        "OAuth response missing refresh_token",
    )?;
    let expires_in = payload.get("expires_in").and_then(js_number);
    let Some(expires_in) = expires_in.filter(|value| *value > 0) else {
        return Err(OAuthError::new(
            "OAuth response missing or invalid expires_in",
        ));
    };
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    Ok(TokenInfo {
        access_token,
        refresh_token,
        expires_at: i64::try_from(now.saturating_add(expires_in)).unwrap_or(i64::MAX),
        scope: string_value(payload, "scope").unwrap_or_default(),
        token_type: string_value(payload, "token_type").unwrap_or_else(|| "Bearer".to_owned()),
        expires_in,
    })
}

fn required_string(
    payload: &Map<String, Value>,
    key: &str,
    message: &str,
) -> Result<String, OAuthError> {
    string_value(payload, key)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| OAuthError::new(message))
}

fn string_value(payload: &Map<String, Value>, key: &str) -> Option<String> {
    payload.get(key)?.as_str().map(str::to_owned)
}

fn pick_error_detail(data: &Map<String, Value>) -> String {
    extract_api_error_message(&Value::Object(data.clone())).unwrap_or_else(|| "unknown".to_owned())
}

fn js_number(value: &Value) -> Option<u64> {
    match value {
        Value::Number(number) => number.as_u64().or_else(|| {
            number
                .as_f64()
                .filter(|number| number.is_finite() && *number >= 0.0)
                .map(|number| number.trunc() as u64)
        }),
        Value::String(text) => text
            .trim()
            .parse::<f64>()
            .ok()
            .filter(|number| number.is_finite() && *number >= 0.0)
            .map(|number| number.trunc() as u64),
        Value::Bool(value) => Some(u8::from(*value) as u64),
        Value::Null => Some(0),
        Value::Array(values) if values.is_empty() => Some(0),
        Value::Array(values) if values.len() == 1 => js_number(&values[0]),
        Value::Array(_) | Value::Object(_) => None,
    }
}

fn describe_error_chain(error: &(dyn Error + 'static)) -> String {
    let mut messages = HashSet::new();
    let mut ordered = Vec::new();
    let mut current = Some(error);
    while let Some(item) = current {
        let message = item.to_string();
        if messages.insert(message.clone()) {
            ordered.push(message);
        }
        current = item.source();
    }
    ordered.join(": ")
}

#[cfg(test)]
mod tests {
    use std::{
        io::{Read, Write},
        net::TcpListener,
        sync::{Arc, Mutex},
        thread,
    };

    use super::*;
    use crate::{errors::OAuthErrorKind, identity::KIMI_CODE_PLATFORM};

    #[derive(Clone)]
    struct FakeResponse {
        status: u16,
        body: String,
        drop_connection: bool,
    }

    fn json_response(status: u16, body: Value) -> FakeResponse {
        FakeResponse {
            status,
            body: body.to_string(),
            drop_connection: false,
        }
    }

    fn fake_server(
        responses: Vec<FakeResponse>,
    ) -> (String, Arc<Mutex<Vec<String>>>, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake OAuth server");
        let address = listener.local_addr().expect("fake server address");
        let recorded = Arc::new(Mutex::new(Vec::new()));
        let recorded_for_thread = Arc::clone(&recorded);
        let handle = thread::spawn(move || {
            for response in responses {
                let (mut stream, _) = listener.accept().expect("accept fake request");
                let mut request = Vec::new();
                let mut buffer = [0_u8; 4096];
                loop {
                    let count = stream.read(&mut buffer).expect("read fake request");
                    if count == 0 {
                        break;
                    }
                    request.extend_from_slice(&buffer[..count]);
                    let text = String::from_utf8_lossy(&request);
                    if let Some(header_end) = text.find("\r\n\r\n") {
                        let content_length = text[..header_end]
                            .lines()
                            .find_map(|line| {
                                line.to_ascii_lowercase()
                                    .strip_prefix("content-length:")
                                    .and_then(|value| value.trim().parse::<usize>().ok())
                            })
                            .unwrap_or(0);
                        if request.len() >= header_end + 4 + content_length {
                            break;
                        }
                    }
                }
                recorded_for_thread
                    .lock()
                    .expect("recorded lock")
                    .push(String::from_utf8_lossy(&request).into_owned());
                if response.drop_connection {
                    continue;
                }
                let reply = format!(
                    "HTTP/1.1 {} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    response.status,
                    response.body.len(),
                    response.body
                );
                stream
                    .write_all(reply.as_bytes())
                    .expect("write fake reply");
            }
        });
        (format!("http://{address}"), recorded, handle)
    }

    fn config(host: String) -> OAuthFlowConfig {
        OAuthFlowConfig {
            name: "kimi-code".to_owned(),
            oauth_host: host,
            client_id: "test-client-id".to_owned(),
        }
    }

    fn headers() -> DeviceHeaders {
        IndexMap::from([
            ("X-Msh-Platform".to_owned(), KIMI_CODE_PLATFORM.to_owned()),
            ("X-Msh-Device-Id".to_owned(), "test-device-id".to_owned()),
        ])
    }

    fn no_backoff(_: usize) -> Duration {
        Duration::ZERO
    }

    #[tokio::test]
    async fn requests_and_validates_device_authorization() {
        let (host, recorded, handle) = fake_server(vec![json_response(
            200,
            serde_json::json!({
                "user_code": "WDJB-MJHT",
                "device_code": "devcode123",
                "verification_uri_complete": "https://auth/verify?code=x",
                "expires_in": 600,
            }),
        )]);
        let auth = request_device_authorization(&config(host), Some(&headers()))
            .await
            .expect("device authorization");
        handle.join().expect("fake server thread");
        assert_eq!(auth.user_code, "WDJB-MJHT");
        assert_eq!(auth.interval, 5);
        let request = recorded.lock().expect("recorded lock")[0].to_ascii_lowercase();
        assert!(request.starts_with("post /api/oauth/device_authorization "));
        assert!(request.contains("content-type: application/x-www-form-urlencoded"));
        assert!(request.contains("x-msh-device-id: test-device-id"));
        assert!(request.ends_with("client_id=test-client-id"));
    }

    #[tokio::test]
    async fn rejects_missing_required_device_authorization_fields() {
        let (host, _, handle) = fake_server(vec![json_response(
            200,
            serde_json::json!({ "user_code": "U", "verification_uri_complete": "https://x" }),
        )]);
        let error = request_device_authorization(&config(host), None)
            .await
            .expect_err("missing device code must fail");
        handle.join().expect("fake server thread");
        assert_eq!(
            error.to_string(),
            "Device authorization response missing device_code"
        );
    }

    #[tokio::test]
    async fn classifies_device_poll_results_and_server_failures() {
        let responses = vec![
            json_response(400, serde_json::json!({ "error": "authorization_pending" })),
            json_response(
                400,
                serde_json::json!({ "error": "slow_down", "detail": "wait" }),
            ),
            json_response(400, serde_json::json!({ "error": "expired_token" })),
            json_response(
                400,
                serde_json::json!({ "error": "access_denied", "message": "no" }),
            ),
            json_response(500, serde_json::json!({ "message": "overloaded" })),
        ];
        let (host, _, handle) = fake_server(responses);
        let config = config(host);
        assert!(matches!(
            poll_device_token(&config, "d", None).await.expect("pending"),
            DevicePollResult::Pending { ref error_code, .. } if error_code == "authorization_pending"
        ));
        assert!(matches!(
            poll_device_token(&config, "d", None).await.expect("slow down"),
            DevicePollResult::Pending { ref description, .. } if description == "wait"
        ));
        assert_eq!(
            poll_device_token(&config, "d", None)
                .await
                .expect("expired"),
            DevicePollResult::Expired
        );
        assert!(matches!(
            poll_device_token(&config, "d", None).await.expect("denied"),
            DevicePollResult::Denied { ref description } if description == "no"
        ));
        let error = poll_device_token(&config, "d", None)
            .await
            .expect_err("server error");
        handle.join().expect("fake server thread");
        assert_eq!(
            error.to_string(),
            "Device token polling server error (HTTP 500): overloaded"
        );
    }

    #[tokio::test]
    async fn parses_successful_tokens_and_preserves_form_encoding() {
        let (host, recorded, handle) = fake_server(vec![json_response(
            200,
            serde_json::json!({
                "access_token": "a",
                "refresh_token": "r",
                "expires_in": "60",
            }),
        )]);
        let result = poll_device_token(&config(host), "device code", None)
            .await
            .expect("token result");
        handle.join().expect("fake server thread");
        let DevicePollResult::Success(token) = result else {
            panic!("expected successful token")
        };
        assert_eq!(token.expires_in, 60);
        assert_eq!(token.token_type, "Bearer");
        let request = &recorded.lock().expect("recorded lock")[0];
        assert!(request.contains("device_code=device+code"));
        assert!(
            request.contains("grant_type=urn%3Aietf%3Aparams%3Aoauth%3Agrant-type%3Adevice_code")
        );
    }

    #[tokio::test]
    async fn refresh_retries_transient_status_then_succeeds() {
        let (host, recorded, handle) = fake_server(vec![
            json_response(503, serde_json::json!({ "error_description": "busy" })),
            json_response(
                200,
                serde_json::json!({
                    "access_token": "new-a", "refresh_token": "new-r", "expires_in": 60
                }),
            ),
        ]);
        let config = config(host);
        let token = refresh_access_token(
            &config,
            "old-r",
            RefreshOptions {
                max_retries: 2,
                backoff: no_backoff,
                ..RefreshOptions::default()
            },
        )
        .await
        .expect("retry succeeds");
        handle.join().expect("fake server thread");
        assert_eq!(token.access_token, "new-a");
        assert_eq!(recorded.lock().expect("recorded lock").len(), 2);
    }

    #[tokio::test]
    async fn refresh_fails_fast_for_unauthorized_and_non_retryable_responses() {
        for (status, body, expected_kind) in [
            (
                401,
                serde_json::json!({ "message": "revoked" }),
                OAuthErrorKind::Unauthorized,
            ),
            (
                400,
                serde_json::json!({ "error": "invalid_request" }),
                OAuthErrorKind::General,
            ),
        ] {
            let (host, recorded, handle) = fake_server(vec![json_response(status, body)]);
            let error = refresh_access_token(
                &config(host),
                "old-r",
                RefreshOptions {
                    max_retries: 3,
                    backoff: no_backoff,
                    ..RefreshOptions::default()
                },
            )
            .await
            .expect_err("refresh must fail");
            handle.join().expect("fake server thread");
            assert_eq!(error.kind(), expected_kind);
            assert_eq!(recorded.lock().expect("recorded lock").len(), 1);
        }
    }

    #[tokio::test]
    async fn refresh_exhaustion_preserves_retryable_error_kind() {
        let (host, _, handle) = fake_server(vec![
            json_response(503, serde_json::json!({})),
            json_response(503, serde_json::json!({})),
        ]);
        let error = refresh_access_token(
            &config(host),
            "old-r",
            RefreshOptions {
                max_retries: 2,
                backoff: no_backoff,
                ..RefreshOptions::default()
            },
        )
        .await
        .expect_err("retry exhaustion");
        handle.join().expect("fake server thread");
        assert_eq!(error.kind(), OAuthErrorKind::RetryableRefresh);
    }

    #[tokio::test]
    async fn refresh_retries_dropped_connections() {
        let dropped = FakeResponse {
            status: 0,
            body: String::new(),
            drop_connection: true,
        };
        let success = json_response(
            200,
            serde_json::json!({
                "access_token": "recovered", "refresh_token": "r", "expires_in": 60
            }),
        );
        let (host, recorded, handle) = fake_server(vec![dropped, success]);
        let token = refresh_access_token(
            &config(host),
            "old-r",
            RefreshOptions {
                max_retries: 2,
                backoff: no_backoff,
                ..RefreshOptions::default()
            },
        )
        .await
        .expect("transport retry succeeds");
        handle.join().expect("fake server thread");
        assert_eq!(token.access_token, "recovered");
        assert_eq!(recorded.lock().expect("recorded lock").len(), 2);
    }

    #[tokio::test]
    async fn malformed_success_token_is_rejected() {
        let (host, _, handle) = fake_server(vec![json_response(
            200,
            serde_json::json!({ "access_token": "a", "refresh_token": "r", "expires_in": 0 }),
        )]);
        let error = poll_device_token(&config(host), "d", None)
            .await
            .expect_err("zero expiry must fail");
        handle.join().expect("fake server thread");
        assert_eq!(
            error.to_string(),
            "OAuth response missing or invalid expires_in"
        );
    }
}
