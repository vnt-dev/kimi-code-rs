use std::time::Duration;

use serde::Serialize;
use serde_json::Value;

use super::{api_error::read_api_error_message, managed_usage::kimi_code_base_url};

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CreateFeedbackUploadUrlBody {
    pub file_hash: String,
    pub file_name: String,
    pub file_size: i64,
    pub feedback_id: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FeedbackUploadPart {
    pub part_number: i64,
    pub url: String,
    pub method: String,
    pub size: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CompleteFeedbackUploadPart {
    pub part_number: i64,
    pub etag: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CompleteFeedbackUploadBody {
    pub upload_id: i64,
    pub parts: Vec<CompleteFeedbackUploadPart>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum FetchCreateFeedbackUploadUrlResult {
    Ok {
        upload_id: i64,
        parts: Vec<FeedbackUploadPart>,
    },
    Error {
        status: Option<u16>,
        message: String,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum FetchCompleteFeedbackUploadResult {
    Ok,
    Error {
        status: Option<u16>,
        message: String,
    },
}

enum PostJsonResult {
    Ok(Value),
    Error {
        status: Option<u16>,
        message: String,
    },
}

// Original:
//   packages/oauth/src/managed-feedback-upload.ts
//   kimiCodeFeedbackUploadUrl()
pub fn kimi_code_feedback_upload_url(base_url: Option<&str>) -> String {
    format!("{}/feedback/upload_url", feedback_base_url(base_url))
}

// Original: kimiCodeFeedbackUploadCompleteUrl()
pub fn kimi_code_feedback_upload_complete_url(base_url: Option<&str>) -> String {
    format!("{}/feedback/upload_complete", feedback_base_url(base_url))
}

// Original: fetchCreateFeedbackUploadUrl()
pub async fn fetch_create_feedback_upload_url(
    access_token: &str,
    body: &CreateFeedbackUploadUrlBody,
    timeout: Option<Duration>,
    base_url: Option<&str>,
) -> FetchCreateFeedbackUploadUrlResult {
    match post_json(
        &kimi_code_feedback_upload_url(base_url),
        access_token,
        body,
        timeout,
    )
    .await
    {
        PostJsonResult::Error { status, message } => {
            FetchCreateFeedbackUploadUrlResult::Error { status, message }
        }
        PostJsonResult::Ok(payload) => match read_upload(&payload) {
            Some((upload_id, parts)) => FetchCreateFeedbackUploadUrlResult::Ok { upload_id, parts },
            None => FetchCreateFeedbackUploadUrlResult::Error {
                status: None,
                message: "Feedback upload request failed: missing upload id or parts.".to_owned(),
            },
        },
    }
}

// Original: fetchCompleteFeedbackUpload()
pub async fn fetch_complete_feedback_upload(
    access_token: &str,
    body: &CompleteFeedbackUploadBody,
    timeout: Option<Duration>,
    base_url: Option<&str>,
) -> FetchCompleteFeedbackUploadResult {
    match post_json(
        &kimi_code_feedback_upload_complete_url(base_url),
        access_token,
        body,
        timeout,
    )
    .await
    {
        PostJsonResult::Ok(_) => FetchCompleteFeedbackUploadResult::Ok,
        PostJsonResult::Error { status, message } => {
            FetchCompleteFeedbackUploadResult::Error { status, message }
        }
    }
}

async fn post_json<T: Serialize + ?Sized>(
    url: &str,
    access_token: &str,
    body: &T,
    timeout: Option<Duration>,
) -> PostJsonResult {
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
            return PostJsonResult::Error {
                status: None,
                message: "Feedback upload request timed out.".to_owned(),
            };
        }
        Err(error) => {
            return PostJsonResult::Error {
                status: None,
                message: format!("Feedback upload request failed: {error}"),
            };
        }
    };
    if !response.status().is_success() {
        let status = response.status().as_u16();
        let fallback = format!("Feedback upload request failed: HTTP {status}");
        return PostJsonResult::Error {
            status: Some(status),
            message: read_api_error_message(response, &fallback).await,
        };
    }
    match response.text().await {
        Ok(text) if text.is_empty() => PostJsonResult::Ok(serde_json::json!({})),
        Ok(text) => match serde_json::from_str(&text) {
            Ok(payload) => PostJsonResult::Ok(payload),
            Err(error) => PostJsonResult::Error {
                status: None,
                message: format!("Feedback upload request failed: {error}"),
            },
        },
        Err(error) => PostJsonResult::Error {
            status: None,
            message: format!("Feedback upload request failed: {error}"),
        },
    }
}

fn feedback_base_url(base_url: Option<&str>) -> String {
    base_url
        .map_or_else(kimi_code_base_url, str::to_owned)
        .trim_end_matches('/')
        .to_owned()
}

fn read_upload(payload: &Value) -> Option<(i64, Vec<FeedbackUploadPart>)> {
    let upload = payload.get("upload")?.as_object()?;
    let upload_id = integer_field(upload.get("id"))?;
    let raw_parts = upload.get("parts")?.as_array()?;
    if raw_parts.is_empty() {
        return None;
    }
    let parts = raw_parts
        .iter()
        .map(read_part)
        .collect::<Option<Vec<_>>>()?;
    Some((upload_id, parts))
}

fn read_part(item: &Value) -> Option<FeedbackUploadPart> {
    let record = item.as_object()?;
    Some(FeedbackUploadPart {
        part_number: integer_field(record.get("part_number"))?,
        url: nonempty_string(record.get("url"))?,
        method: nonempty_string(record.get("method")).unwrap_or_else(|| "PUT".to_owned()),
        size: integer_field(record.get("size"))?,
    })
}

fn integer_field(value: Option<&Value>) -> Option<i64> {
    value?.as_i64()
}

fn nonempty_string(value: Option<&Value>) -> Option<String> {
    value?
        .as_str()
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
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

    fn fake_server(
        path: &str,
        status: u16,
        response_body: &str,
    ) -> (String, Arc<Mutex<String>>, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind upload server");
        let address = listener.local_addr().expect("upload server address");
        let request = Arc::new(Mutex::new(String::new()));
        let recorded = Arc::clone(&request);
        let response_body = response_body.to_owned();
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept upload request");
            let mut bytes = Vec::new();
            let mut buffer = [0_u8; 4_096];
            loop {
                let count = stream.read(&mut buffer).expect("read upload request");
                if count == 0 {
                    break;
                }
                bytes.extend_from_slice(&buffer[..count]);
                let text = String::from_utf8_lossy(&bytes);
                if let Some(header_end) = text.find("\r\n\r\n") {
                    let length = text[..header_end]
                        .lines()
                        .find_map(|line| {
                            line.to_ascii_lowercase()
                                .strip_prefix("content-length:")
                                .and_then(|value| value.trim().parse::<usize>().ok())
                        })
                        .unwrap_or(0);
                    if bytes.len() >= header_end + 4 + length {
                        break;
                    }
                }
            }
            *recorded.lock().expect("request lock") = String::from_utf8_lossy(&bytes).into_owned();
            let response = format!(
                "HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response_body}",
                response_body.len()
            );
            stream
                .write_all(response.as_bytes())
                .expect("write upload response");
        });
        (format!("http://{address}{path}"), request, handle)
    }

    #[test]
    fn builds_both_upload_paths_after_trimming_slashes() {
        assert_eq!(
            kimi_code_feedback_upload_url(Some("https://example/v1///")),
            "https://example/v1/feedback/upload_url"
        );
        assert_eq!(
            kimi_code_feedback_upload_complete_url(Some("https://example/v1///")),
            "https://example/v1/feedback/upload_complete"
        );
    }

    #[test]
    fn upload_parser_requires_every_part_and_defaults_the_method() {
        assert_eq!(
            read_upload(&serde_json::json!({
                "upload": { "id": 9, "parts": [
                    { "part_number": 1, "url": "https://put/1", "size": 10 },
                    { "part_number": 2, "url": "https://put/2", "method": "PATCH", "size": 20 }
                ] }
            })),
            Some((
                9,
                vec![
                    FeedbackUploadPart {
                        part_number: 1,
                        url: "https://put/1".to_owned(),
                        method: "PUT".to_owned(),
                        size: 10
                    },
                    FeedbackUploadPart {
                        part_number: 2,
                        url: "https://put/2".to_owned(),
                        method: "PATCH".to_owned(),
                        size: 20
                    }
                ]
            ))
        );
        for payload in [
            serde_json::json!({ "upload": { "id": 1, "parts": [] } }),
            serde_json::json!({ "upload": { "id": 1, "parts": [{ "part_number": 1, "size": 2 }] } }),
            serde_json::json!({ "upload": { "parts": [{ "part_number": 1, "url": "x", "size": 2 }] } }),
        ] {
            assert_eq!(read_upload(&payload), None);
        }
    }

    #[tokio::test]
    async fn create_upload_posts_json_and_parses_parts() {
        let response =
            r#"{"upload":{"id":7,"parts":[{"part_number":1,"url":"https://put/1","size":12}]}}"#;
        let (url, request, handle) = fake_server("/feedback/upload_url", 200, response);
        let base = url.trim_end_matches("/feedback/upload_url");
        let result = fetch_create_feedback_upload_url(
            "token",
            &CreateFeedbackUploadUrlBody {
                file_hash: "abc".to_owned(),
                file_name: "feedback.zip".to_owned(),
                file_size: 12,
                feedback_id: 3,
            },
            None,
            Some(base),
        )
        .await;
        handle.join().expect("upload server thread");
        assert!(matches!(
            result,
            FetchCreateFeedbackUploadUrlResult::Ok { upload_id: 7, .. }
        ));
        let request = request.lock().expect("request lock");
        assert!(
            request
                .to_ascii_lowercase()
                .contains("authorization: bearer token")
        );
        assert!(request.contains(r#""file_hash":"abc""#));
    }

    #[tokio::test]
    async fn complete_upload_posts_parts_and_accepts_an_empty_success_body() {
        let (url, request, handle) = fake_server("/feedback/upload_complete", 200, "");
        let base = url.trim_end_matches("/feedback/upload_complete");
        let result = fetch_complete_feedback_upload(
            "token",
            &CompleteFeedbackUploadBody {
                upload_id: 7,
                parts: vec![CompleteFeedbackUploadPart {
                    part_number: 1,
                    etag: "etag-1".to_owned(),
                }],
            },
            None,
            Some(base),
        )
        .await;
        handle.join().expect("upload server thread");
        assert_eq!(result, FetchCompleteFeedbackUploadResult::Ok);
        assert!(
            request
                .lock()
                .expect("request lock")
                .contains(r#""etag":"etag-1""#)
        );
    }

    #[tokio::test]
    async fn create_upload_surfaces_api_status_and_invalid_payloads() {
        for (status, body, expected_status, expected_message) in [
            (
                401,
                r#"{"message":"login required"}"#,
                Some(401),
                "login required",
            ),
            (
                200,
                r#"{"upload":{"id":1}}"#,
                None,
                "Feedback upload request failed: missing upload id or parts.",
            ),
        ] {
            let (url, _, handle) = fake_server("/feedback/upload_url", status, body);
            let base = url.trim_end_matches("/feedback/upload_url");
            let result = fetch_create_feedback_upload_url(
                "token",
                &CreateFeedbackUploadUrlBody {
                    file_hash: "h".to_owned(),
                    file_name: "f".to_owned(),
                    file_size: 1,
                    feedback_id: 2,
                },
                None,
                Some(base),
            )
            .await;
            handle.join().expect("upload server thread");
            assert_eq!(
                result,
                FetchCreateFeedbackUploadUrlResult::Error {
                    status: expected_status,
                    message: expected_message.to_owned()
                }
            );
        }
    }
}
