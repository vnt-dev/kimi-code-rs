use std::time::Duration;

use serde::Serialize;
use serde_json::{Map, Value};

use super::{api_error::read_api_error_message, managed_usage::kimi_code_base_url};

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SubmitFeedbackBody {
    pub session_id: String,
    pub content: String,
    pub version: String,
    pub os: String,
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub contact: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub info: Option<Map<String, Value>>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum FetchSubmitFeedbackResult {
    Ok {
        feedback_id: f64,
    },
    Error {
        status: Option<u16>,
        message: String,
    },
}

// Original:
//   packages/oauth/src/managed-feedback.ts
//   kimiCodeFeedbackUrl()
pub fn kimi_code_feedback_url(base_url: Option<&str>) -> String {
    let base_url = base_url.map_or_else(kimi_code_base_url, str::to_owned);
    format!("{}/feedback", base_url.trim_end_matches('/'))
}

// Original: fetchSubmitFeedback()
pub async fn fetch_submit_feedback(
    url: &str,
    access_token: &str,
    body: &SubmitFeedbackBody,
    timeout: Option<Duration>,
) -> FetchSubmitFeedbackResult {
    let response = reqwest::Client::new()
        .post(url)
        .timeout(timeout.unwrap_or(Duration::from_secs(8)))
        .header("Authorization", format!("Bearer {access_token}"))
        .header("Accept", "application/json")
        .json(body)
        .send()
        .await;
    let response = match response {
        Ok(response) => response,
        Err(error) if error.is_timeout() => {
            return FetchSubmitFeedbackResult::Error {
                status: None,
                message: "Failed to submit feedback: request timed out.".to_owned(),
            };
        }
        Err(error) => {
            return FetchSubmitFeedbackResult::Error {
                status: None,
                message: format!("Failed to submit feedback: {error}"),
            };
        }
    };

    if !response.status().is_success() {
        let status = response.status().as_u16();
        let fallback = format!("Failed to submit feedback: HTTP {status}");
        return FetchSubmitFeedbackResult::Error {
            status: Some(status),
            message: read_api_error_message(response, &fallback).await,
        };
    }

    match response.json::<Value>().await {
        Ok(payload) => match parse_feedback_id(&payload) {
            Some(feedback_id) => FetchSubmitFeedbackResult::Ok { feedback_id },
            None => FetchSubmitFeedbackResult::Error {
                status: None,
                message: "Failed to submit feedback: missing feedback_id.".to_owned(),
            },
        },
        Err(error) => FetchSubmitFeedbackResult::Error {
            status: None,
            message: format!("Failed to submit feedback: {error}"),
        },
    }
}

fn parse_feedback_id(payload: &Value) -> Option<f64> {
    read_feedback_id(payload).or_else(|| payload.get("data").and_then(read_feedback_id))
}

fn read_feedback_id(payload: &Value) -> Option<f64> {
    let record = payload.as_object()?;
    let value = record.get("feedback_id").or_else(|| record.get("id"))?;
    let number = value.as_f64()?;
    (number.is_finite() && number.fract() == 0.0).then_some(number)
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

    fn sample_body() -> SubmitFeedbackBody {
        SubmitFeedbackBody {
            session_id: "sess-123".to_owned(),
            content: "great tool".to_owned(),
            version: "kimi-code-0.1.1".to_owned(),
            os: "Darwin 25.3.0".to_owned(),
            model: Some("kimi-code/kimi-for-coding".to_owned()),
            contact: Some("test@example.com".to_owned()),
            info: Some(Map::from_iter([
                ("tool".to_owned(), Value::String("kimi-code-cli".to_owned())),
                ("env".to_owned(), Value::String("test".to_owned())),
            ])),
        }
    }

    fn fake_server(
        status: u16,
        body: &str,
        delay: Duration,
    ) -> (String, Arc<Mutex<String>>, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind feedback server");
        let address = listener.local_addr().expect("feedback server address");
        let request = Arc::new(Mutex::new(String::new()));
        let recorded = Arc::clone(&request);
        let body = body.to_owned();
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept feedback request");
            let mut bytes = Vec::new();
            let mut buffer = [0_u8; 4_096];
            loop {
                let count = stream.read(&mut buffer).expect("read feedback request");
                if count == 0 {
                    break;
                }
                bytes.extend_from_slice(&buffer[..count]);
                let text = String::from_utf8_lossy(&bytes);
                if let Some(header_end) = text.find("\r\n\r\n") {
                    let content_length = text[..header_end]
                        .lines()
                        .find_map(|line| {
                            line.to_ascii_lowercase()
                                .strip_prefix("content-length:")
                                .and_then(|value| value.trim().parse::<usize>().ok())
                        })
                        .unwrap_or(0);
                    if bytes.len() >= header_end + 4 + content_length {
                        break;
                    }
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
        (format!("http://{address}/feedback"), request, handle)
    }

    #[test]
    fn feedback_url_trims_every_trailing_slash() {
        assert_eq!(
            kimi_code_feedback_url(Some("https://example.test/v9///")),
            "https://example.test/v9/feedback"
        );
    }

    #[tokio::test]
    async fn submits_exact_json_with_auth_and_parses_direct_or_nested_ids() {
        for response in [r#"{"feedback_id":3}"#, r#"{"data":{"id":4}}"#] {
            let (url, request, handle) = fake_server(200, response, Duration::ZERO);
            let result = fetch_submit_feedback(&url, "access-token", &sample_body(), None).await;
            handle.join().expect("feedback server thread");
            assert!(matches!(result, FetchSubmitFeedbackResult::Ok { .. }));
            let request = request.lock().expect("request lock");
            let (headers, body) = request.split_once("\r\n\r\n").expect("request body");
            let headers = headers.to_ascii_lowercase();
            assert!(headers.starts_with("post /feedback http/1.1"));
            assert!(headers.contains("authorization: bearer access-token"));
            assert!(headers.contains("content-type: application/json"));
            assert!(headers.contains("accept: application/json"));
            let sent: Value = serde_json::from_str(body).expect("feedback JSON");
            assert_eq!(sent["version"], "kimi-code-0.1.1");
            assert_eq!(sent["model"], "kimi-code/kimi-for-coding");
        }
    }

    #[tokio::test]
    async fn reports_missing_id_api_errors_and_timeout() {
        for (status, body, expected) in [
            (
                200,
                r#"{"ok":true}"#,
                "Failed to submit feedback: missing feedback_id.",
            ),
            (
                400,
                r#"{"error":{"message":"feedback rejected"}}"#,
                "feedback rejected",
            ),
            (500, "", "Failed to submit feedback: HTTP 500"),
        ] {
            let (url, _, handle) = fake_server(status, body, Duration::ZERO);
            let result = fetch_submit_feedback(&url, "token", &sample_body(), None).await;
            handle.join().expect("feedback server thread");
            let FetchSubmitFeedbackResult::Error {
                status: actual,
                message,
            } = result
            else {
                panic!("expected feedback error")
            };
            assert_eq!(actual, (status != 200).then_some(status));
            assert_eq!(message, expected);
        }

        let (url, _, handle) = fake_server(200, r#"{"feedback_id":1}"#, Duration::from_millis(100));
        let result = fetch_submit_feedback(
            &url,
            "token",
            &sample_body(),
            Some(Duration::from_millis(10)),
        )
        .await;
        handle.join().expect("feedback server thread");
        assert_eq!(
            result,
            FetchSubmitFeedbackResult::Error {
                status: None,
                message: "Failed to submit feedback: request timed out.".to_owned()
            }
        );
    }

    #[test]
    fn accepts_only_integer_numeric_feedback_ids() {
        for (payload, expected) in [
            (serde_json::json!({ "feedback_id": -1 }), Some(-1.0)),
            (serde_json::json!({ "id": 2 }), Some(2.0)),
            (serde_json::json!({ "feedback_id": 1.5 }), None),
            (serde_json::json!({ "feedback_id": "3" }), None),
        ] {
            assert_eq!(parse_feedback_id(&payload), expected);
        }
    }
}
