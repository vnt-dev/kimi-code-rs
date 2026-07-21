use std::{collections::HashMap, error::Error, fmt};

use indexmap::IndexMap;
use serde_json::Value;

use super::{
    api_error::read_api_error_message, identity::parse_kimi_code_custom_headers,
    managed_usage::kimi_code_base_url,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManagedKimiCodeProtocol {
    Anthropic,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SupportsThinkingType {
    Only,
    No,
    Both,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedKimiCodeModelInfo {
    pub id: String,
    pub context_length: u64,
    pub supports_reasoning: bool,
    pub supports_image_in: bool,
    pub supports_video_in: bool,
    pub supports_tool_use: bool,
    pub supports_thinking_type: Option<SupportsThinkingType>,
    pub support_efforts: Option<Vec<String>>,
    pub default_effort: Option<String>,
    pub display_name: Option<String>,
    pub protocol: Option<ManagedKimiCodeProtocol>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThinkEfforts {
    pub support_efforts: Option<Vec<String>>,
    pub default_effort: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelParseError {
    message: String,
}

impl fmt::Display for ModelParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for ModelParseError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredentialKind {
    OAuth,
    ApiKey,
}

#[derive(Debug)]
pub enum ManagedModelsError {
    Unauthorized {
        status: u16,
        base_url: String,
        message: String,
        credential_kind: CredentialKind,
    },
    Api(String),
    Request(reqwest::Error),
    Json(reqwest::Error),
    InvalidHeader(String),
    Model(ModelParseError),
    UnexpectedResponse(String),
}

impl ManagedModelsError {
    pub fn status(&self) -> Option<u16> {
        match self {
            Self::Unauthorized { status, .. } => Some(*status),
            _ => None,
        }
    }

    pub fn base_url(&self) -> Option<&str> {
        match self {
            Self::Unauthorized { base_url, .. } => Some(base_url),
            _ => None,
        }
    }

    pub fn is_unauthorized(&self) -> bool {
        matches!(self, Self::Unauthorized { .. })
    }
}

impl fmt::Display for ManagedModelsError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unauthorized {
                base_url,
                message,
                credential_kind,
                ..
            } => write!(
                formatter,
                "Kimi Code models endpoint {base_url} rejected {}: {message}",
                match credential_kind {
                    CredentialKind::ApiKey => "the API key",
                    CredentialKind::OAuth => "OAuth credentials",
                }
            ),
            Self::Api(message)
            | Self::InvalidHeader(message)
            | Self::UnexpectedResponse(message) => formatter.write_str(message),
            Self::Request(error) | Self::Json(error) => error.fmt(formatter),
            Self::Model(error) => error.fmt(formatter),
        }
    }
}

impl Error for ManagedModelsError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Request(error) | Self::Json(error) => Some(error),
            Self::Model(error) => Some(error),
            _ => None,
        }
    }
}

// Original:
//   packages/oauth/src/managed-kimi-code.ts
//   parseModelProtocol()
pub fn parse_model_protocol(value: Option<&Value>) -> Option<ManagedKimiCodeProtocol> {
    (value?.as_str()? == "anthropic").then_some(ManagedKimiCodeProtocol::Anthropic)
}

// Original: parseStringArray()
pub fn parse_string_array(value: Option<&Value>) -> Option<Vec<String>> {
    let values = value?.as_array()?;
    let parsed = values
        .iter()
        .filter_map(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .collect::<Vec<_>>();
    (!parsed.is_empty()).then_some(parsed)
}

// Original: parseSupportsThinkingType()
pub fn parse_supports_thinking_type(value: Option<&Value>) -> Option<SupportsThinkingType> {
    match value?.as_str()? {
        "only" => Some(SupportsThinkingType::Only),
        "no" => Some(SupportsThinkingType::No),
        "both" => Some(SupportsThinkingType::Both),
        _ => None,
    }
}

// Original: parseThinkEfforts()
pub fn parse_think_efforts(value: Option<&Value>) -> ThinkEfforts {
    let Some(record) = value.and_then(Value::as_object) else {
        return empty_think_efforts();
    };
    if record.get("support").and_then(Value::as_bool) != Some(true) {
        return empty_think_efforts();
    }
    ThinkEfforts {
        support_efforts: parse_string_array(record.get("valid_efforts")),
        default_effort: record
            .get("default_effort")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_owned),
    }
}

fn empty_think_efforts() -> ThinkEfforts {
    ThinkEfforts {
        support_efforts: None,
        default_effort: None,
    }
}

// Original:
//   packages/oauth/src/managed-kimi-code.ts toModelInfo()
//   packages/oauth/src/open-platform.ts toModelInfo()
pub fn parse_model_info(
    item: &Value,
    model_label: &str,
) -> Result<Option<ManagedKimiCodeModelInfo>, ModelParseError> {
    let Some(record) = item.as_object() else {
        return Ok(None);
    };
    let Some(id) = record
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
    else {
        return Ok(None);
    };
    let context_length =
        js_positive_integer(record.get("context_length")).ok_or_else(|| ModelParseError {
            message: format!("{model_label} \"{id}\" must include a positive context_length."),
        })?;
    let think_efforts = parse_think_efforts(record.get("think_efforts"));
    Ok(Some(ManagedKimiCodeModelInfo {
        id: id.to_owned(),
        context_length,
        supports_reasoning: js_boolean(record.get("supports_reasoning")),
        supports_image_in: js_boolean(record.get("supports_image_in")),
        supports_video_in: js_boolean(record.get("supports_video_in")),
        supports_tool_use: record
            .get("supports_tool_use")
            .is_none_or(|value| js_boolean(Some(value))),
        supports_thinking_type: parse_supports_thinking_type(record.get("supports_thinking_type")),
        support_efforts: think_efforts.support_efforts,
        default_effort: think_efforts.default_effort,
        display_name: record
            .get("display_name")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_owned),
        protocol: parse_model_protocol(record.get("protocol")),
    }))
}

// Original: fetchManagedKimiCodeModels()
pub async fn fetch_managed_kimi_code_models(
    access_token: &str,
    base_url: Option<&str>,
    headers: Option<&IndexMap<String, String>>,
    credential_kind: CredentialKind,
) -> Result<Vec<ManagedKimiCodeModelInfo>, ManagedModelsError> {
    let base_url = base_url
        .map_or_else(kimi_code_base_url, str::to_owned)
        .trim_end_matches('/')
        .to_owned();
    let environment = std::env::vars().collect::<HashMap<_, _>>();
    let custom_headers = parse_kimi_code_custom_headers(&environment);
    let request_headers = build_model_headers(
        access_token,
        &custom_headers,
        headers.unwrap_or(&IndexMap::new()),
    )?;
    let response = reqwest::Client::new()
        .get(format!("{base_url}/models"))
        .headers(request_headers)
        .send()
        .await
        .map_err(ManagedModelsError::Request)?;
    if !response.status().is_success() {
        let status = response.status().as_u16();
        let fallback = format!("Failed to list Kimi Code models (HTTP {status}).");
        let message = read_api_error_message(response, &fallback).await;
        if matches!(status, 401..=403) {
            return Err(ManagedModelsError::Unauthorized {
                status,
                base_url,
                message,
                credential_kind,
            });
        }
        return Err(ManagedModelsError::Api(message));
    }
    let payload = response
        .json::<Value>()
        .await
        .map_err(ManagedModelsError::Json)?;
    let Some(data) = payload.get("data").and_then(Value::as_array) else {
        return Err(ManagedModelsError::UnexpectedResponse(format!(
            "Unexpected models response for {base_url}."
        )));
    };
    data.iter()
        .filter_map(|item| match parse_model_info(item, "Kimi Code model") {
            Ok(Some(model)) => Some(Ok(model)),
            Ok(None) => None,
            Err(error) => Some(Err(ManagedModelsError::Model(error))),
        })
        .collect()
}

fn build_model_headers(
    access_token: &str,
    custom_headers: &IndexMap<String, String>,
    supplied_headers: &IndexMap<String, String>,
) -> Result<reqwest::header::HeaderMap, ManagedModelsError> {
    let mut headers = reqwest::header::HeaderMap::new();
    for source in [custom_headers, supplied_headers] {
        for (name, value) in source {
            let name = reqwest::header::HeaderName::from_bytes(name.as_bytes())
                .map_err(|error| ManagedModelsError::InvalidHeader(error.to_string()))?;
            let value = reqwest::header::HeaderValue::from_str(value)
                .map_err(|error| ManagedModelsError::InvalidHeader(error.to_string()))?;
            headers.insert(name, value);
        }
    }
    headers.insert(
        reqwest::header::AUTHORIZATION,
        reqwest::header::HeaderValue::from_str(&format!("Bearer {access_token}"))
            .map_err(|error| ManagedModelsError::InvalidHeader(error.to_string()))?,
    );
    headers.insert(
        reqwest::header::ACCEPT,
        reqwest::header::HeaderValue::from_static("application/json"),
    );
    Ok(headers)
}

fn js_positive_integer(value: Option<&Value>) -> Option<u64> {
    let number = match value? {
        Value::Number(number) => number.as_f64()?,
        Value::String(value) if value.trim().is_empty() => 0.0,
        Value::String(value) => value.trim().parse().ok()?,
        Value::Bool(value) => u8::from(*value) as f64,
        Value::Null => 0.0,
        Value::Array(values) if values.is_empty() => 0.0,
        Value::Array(values) if values.len() == 1 => return js_positive_integer(values.first()),
        Value::Array(_) | Value::Object(_) => return None,
    };
    (number.is_finite() && number > 0.0 && number.fract() == 0.0 && number <= u64::MAX as f64)
        .then_some(number as u64)
}

fn js_boolean(value: Option<&Value>) -> bool {
    match value {
        None | Some(Value::Null) => false,
        Some(Value::Bool(value)) => *value,
        Some(Value::Number(number)) => number.as_f64().is_some_and(|value| value != 0.0),
        Some(Value::String(value)) => !value.is_empty(),
        Some(Value::Array(_) | Value::Object(_)) => true,
    }
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

    #[test]
    fn parses_protocol_thinking_type_and_nonempty_string_arrays() {
        assert_eq!(
            parse_model_protocol(Some(&Value::String("anthropic".to_owned()))),
            Some(ManagedKimiCodeProtocol::Anthropic)
        );
        assert_eq!(
            parse_model_protocol(Some(&Value::String("kimi".to_owned()))),
            None
        );
        for (value, expected) in [
            ("only", Some(SupportsThinkingType::Only)),
            ("no", Some(SupportsThinkingType::No)),
            ("both", Some(SupportsThinkingType::Both)),
            ("unknown", None),
        ] {
            assert_eq!(
                parse_supports_thinking_type(Some(&Value::String(value.to_owned()))),
                expected
            );
        }
        assert_eq!(
            parse_string_array(Some(&serde_json::json!(["low", "", 3, "high"]))),
            Some(vec!["low".to_owned(), "high".to_owned()])
        );
    }

    #[test]
    fn think_efforts_are_gated_by_literal_true_support() {
        let enabled = parse_think_efforts(Some(&serde_json::json!({
            "support": true,
            "valid_efforts": ["low", "high", "max"],
            "default_effort": "high"
        })));
        assert_eq!(
            enabled,
            ThinkEfforts {
                support_efforts: Some(vec!["low".to_owned(), "high".to_owned(), "max".to_owned()]),
                default_effort: Some("high".to_owned())
            }
        );
        for value in [
            Value::Null,
            serde_json::json!({ "support": false, "valid_efforts": ["low"] }),
            serde_json::json!({ "support": 1, "default_effort": "high" }),
        ] {
            assert_eq!(parse_think_efforts(Some(&value)), empty_think_efforts());
        }
    }

    #[test]
    fn parses_model_fields_with_javascript_number_and_boolean_rules() {
        let parsed = parse_model_info(
            &serde_json::json!({
                "id": "kimi-k2",
                "context_length": "256000",
                "supports_reasoning": 1,
                "supports_image_in": "false",
                "supports_video_in": 0,
                "supports_tool_use": false,
                "supports_thinking_type": "only",
                "display_name": "Kimi K2",
                "protocol": "anthropic",
                "think_efforts": {
                    "support": true,
                    "valid_efforts": ["low", "high"],
                    "default_effort": "high"
                }
            }),
            "Kimi Code model",
        )
        .expect("valid model")
        .expect("model exists");
        assert_eq!(parsed.context_length, 256_000);
        assert!(parsed.supports_reasoning);
        assert!(
            parsed.supports_image_in,
            "nonempty strings are JavaScript truthy"
        );
        assert!(!parsed.supports_video_in);
        assert!(!parsed.supports_tool_use);
        assert_eq!(
            parsed.supports_thinking_type,
            Some(SupportsThinkingType::Only)
        );
        assert_eq!(parsed.protocol, Some(ManagedKimiCodeProtocol::Anthropic));
    }

    #[test]
    fn skips_missing_ids_and_rejects_non_positive_or_fractional_context() {
        assert_eq!(
            parse_model_info(&serde_json::json!({ "context_length": 1 }), "")
                .expect("missing id is skipped"),
            None
        );
        for context in [
            serde_json::json!(0),
            serde_json::json!(-1),
            serde_json::json!(1.5),
        ] {
            let error = parse_model_info(
                &serde_json::json!({ "id": "bad", "context_length": context }),
                "Kimi Code model",
            )
            .expect_err("invalid context");
            assert_eq!(
                error.to_string(),
                "Kimi Code model \"bad\" must include a positive context_length."
            );
        }
    }

    fn fake_models_server(
        status: u16,
        body: &str,
    ) -> (String, Arc<Mutex<String>>, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind models server");
        let address = listener.local_addr().expect("models server address");
        let request = Arc::new(Mutex::new(String::new()));
        let recorded = Arc::clone(&request);
        let body = body.to_owned();
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept models request");
            let mut bytes = Vec::new();
            let mut buffer = [0_u8; 4_096];
            loop {
                let count = stream.read(&mut buffer).expect("read models request");
                if count == 0 {
                    break;
                }
                bytes.extend_from_slice(&buffer[..count]);
                if bytes.windows(4).any(|window| window == b"\r\n\r\n") {
                    break;
                }
            }
            *recorded.lock().expect("request lock") = String::from_utf8_lossy(&bytes).into_owned();
            let response = format!(
                "HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream
                .write_all(response.as_bytes())
                .expect("write models response");
        });
        (format!("http://{address}/coding/v1"), request, handle)
    }

    #[test]
    fn model_headers_follow_custom_supplied_and_auth_override_order() {
        let custom = IndexMap::from([
            ("X-Test".to_owned(), "custom".to_owned()),
            ("Authorization".to_owned(), "wrong".to_owned()),
        ]);
        let supplied = IndexMap::from([
            ("x-test".to_owned(), "supplied".to_owned()),
            ("Accept".to_owned(), "text/plain".to_owned()),
        ]);
        let headers = build_model_headers("right", &custom, &supplied).expect("headers");
        assert_eq!(headers["x-test"], "supplied");
        assert_eq!(headers[reqwest::header::AUTHORIZATION], "Bearer right");
        assert_eq!(headers[reqwest::header::ACCEPT], "application/json");
    }

    #[tokio::test]
    async fn fetches_and_parses_managed_models() {
        let (base_url, request, handle) = fake_models_server(
            200,
            r#"{"data":[{"id":"kimi-for-coding","context_length":262144,"supports_reasoning":true,"supports_image_in":true,"think_efforts":{"support":true,"valid_efforts":["low","high"],"default_effort":"high"}},{"bad":true}]}"#,
        );
        let models = fetch_managed_kimi_code_models(
            "oauth-token",
            Some(&base_url),
            None,
            CredentialKind::OAuth,
        )
        .await
        .expect("models");
        handle.join().expect("models server thread");
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].context_length, 262_144);
        assert_eq!(
            models[0].support_efforts,
            Some(vec!["low".to_owned(), "high".to_owned()])
        );
        let request = request.lock().expect("request lock").to_ascii_lowercase();
        assert!(request.starts_with("get /coding/v1/models http/1.1"));
        assert!(request.contains("authorization: bearer oauth-token"));
    }

    #[tokio::test]
    async fn classifies_auth_statuses_with_credential_specific_messages() {
        for (status, kind, credential_text) in [
            (401, CredentialKind::OAuth, "OAuth credentials"),
            (402, CredentialKind::ApiKey, "the API key"),
            (403, CredentialKind::OAuth, "OAuth credentials"),
        ] {
            let (base_url, _, handle) =
                fake_models_server(status, r#"{"error":{"message":"membership rejected"}}"#);
            let error = fetch_managed_kimi_code_models("token", Some(&base_url), None, kind)
                .await
                .expect_err("auth error");
            handle.join().expect("models server thread");
            assert!(error.is_unauthorized());
            assert_eq!(error.status(), Some(status));
            assert_eq!(error.base_url(), Some(base_url.as_str()));
            assert_eq!(
                error.to_string(),
                format!(
                    "Kimi Code models endpoint {base_url} rejected {credential_text}: membership rejected"
                )
            );
        }
    }

    #[tokio::test]
    async fn surfaces_non_auth_api_errors_and_shape_failures() {
        let (base_url, _, handle) =
            fake_models_server(429, r#"{"error":{"message":"quota exceeded"}}"#);
        let error =
            fetch_managed_kimi_code_models("token", Some(&base_url), None, CredentialKind::OAuth)
                .await
                .expect_err("quota error");
        handle.join().expect("models server thread");
        assert!(!error.is_unauthorized());
        assert_eq!(error.to_string(), "quota exceeded");

        let (base_url, _, handle) = fake_models_server(200, r#"{}"#);
        let error =
            fetch_managed_kimi_code_models("token", Some(&base_url), None, CredentialKind::OAuth)
                .await
                .expect_err("shape error");
        handle.join().expect("models server thread");
        assert!(error.to_string().contains("Unexpected models response"));
    }
}
