use std::{error::Error, fmt};

use serde_json::Value;

use super::{
    api_error::read_api_error_message,
    identity::parse_kimi_code_custom_headers,
    managed_models::{
        ManagedKimiCodeModelInfo, ModelParseError, SupportsThinkingType, parse_model_info,
    },
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenPlatformDefinition {
    pub id: &'static str,
    pub name: &'static str,
    pub base_url: &'static str,
    pub console_url: Option<&'static str>,
    pub allowed_prefixes: &'static [&'static str],
}

pub const OPEN_PLATFORMS: [OpenPlatformDefinition; 2] = [
    OpenPlatformDefinition {
        id: "moonshot-cn",
        name: "Kimi Platform (API key · platform.kimi.com)",
        base_url: "https://api.moonshot.cn/v1",
        console_url: Some("https://platform.kimi.com"),
        allowed_prefixes: &["kimi-k"],
    },
    OpenPlatformDefinition {
        id: "moonshot-ai",
        name: "Kimi Platform (API key · platform.kimi.ai)",
        base_url: "https://api.moonshot.ai/v1",
        console_url: Some("https://platform.kimi.ai"),
        allowed_prefixes: &["kimi-k"],
    },
];

#[derive(Debug)]
pub enum OpenPlatformError {
    Api { status: u16, message: String },
    Request(reqwest::Error),
    Json(reqwest::Error),
    Model(ModelParseError),
    InvalidHeader(String),
    UnexpectedResponse(String),
}

impl OpenPlatformError {
    pub fn status(&self) -> Option<u16> {
        match self {
            Self::Api { status, .. } => Some(*status),
            _ => None,
        }
    }
}

impl fmt::Display for OpenPlatformError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Api { message, .. }
            | Self::InvalidHeader(message)
            | Self::UnexpectedResponse(message) => formatter.write_str(message),
            Self::Request(error) | Self::Json(error) => error.fmt(formatter),
            Self::Model(error) => error.fmt(formatter),
        }
    }
}

impl Error for OpenPlatformError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Request(error) | Self::Json(error) => Some(error),
            Self::Model(error) => Some(error),
            Self::Api { .. } | Self::InvalidHeader(_) | Self::UnexpectedResponse(_) => None,
        }
    }
}

// Original:
//   packages/oauth/src/open-platform.ts
//   getOpenPlatformById()
pub fn get_open_platform_by_id(id: &str) -> Option<&'static OpenPlatformDefinition> {
    OPEN_PLATFORMS.iter().find(|platform| platform.id == id)
}

// Original: isOpenPlatformId()
pub fn is_open_platform_id(id: &str) -> bool {
    get_open_platform_by_id(id).is_some()
}

// Original: capabilitiesForModel()
pub fn capabilities_for_model(model: &ManagedKimiCodeModelInfo) -> Option<Vec<String>> {
    let mut capabilities = Vec::new();
    match model.supports_thinking_type {
        Some(SupportsThinkingType::Only) => {
            capabilities.push("thinking".to_owned());
            capabilities.push("always_thinking".to_owned());
        }
        Some(SupportsThinkingType::Both) => capabilities.push("thinking".to_owned()),
        Some(SupportsThinkingType::No) => {}
        None if model.supports_reasoning => capabilities.push("thinking".to_owned()),
        None => {}
    }
    if model.supports_image_in {
        capabilities.push("image_in".to_owned());
    }
    if model.supports_video_in {
        capabilities.push("video_in".to_owned());
    }
    if model.supports_tool_use {
        capabilities.push("tool_use".to_owned());
    }
    (!capabilities.is_empty()).then_some(capabilities)
}

// Original: filterModelsByPrefix()
pub fn filter_models_by_prefix(
    models: &[ManagedKimiCodeModelInfo],
    platform: &OpenPlatformDefinition,
) -> Vec<ManagedKimiCodeModelInfo> {
    if platform.allowed_prefixes.is_empty() {
        return models.to_vec();
    }
    models
        .iter()
        .filter(|model| {
            platform
                .allowed_prefixes
                .iter()
                .any(|prefix| model.id.starts_with(prefix))
        })
        .cloned()
        .collect()
}

// Original: fetchOpenPlatformModels()
//
// Rust adaptation:
//   Dropping this async future cancels the reqwest request, corresponding to
//   the original optional AbortSignal without introducing a second signal API.
pub async fn fetch_open_platform_models(
    platform: &OpenPlatformDefinition,
    api_key: &str,
) -> Result<Vec<ManagedKimiCodeModelInfo>, OpenPlatformError> {
    let url = format!("{}/models", platform.base_url.trim_end_matches('/'));
    let environment = std::env::vars().collect();
    let headers = build_headers(api_key, &parse_kimi_code_custom_headers(&environment))?;
    let response = reqwest::Client::new()
        .get(url)
        .headers(headers)
        .send()
        .await
        .map_err(OpenPlatformError::Request)?;
    if !response.status().is_success() {
        let status = response.status().as_u16();
        let fallback = format!("Failed to list models (HTTP {status}).");
        return Err(OpenPlatformError::Api {
            status,
            message: read_api_error_message(response, &fallback).await,
        });
    }
    let payload = response
        .json::<Value>()
        .await
        .map_err(OpenPlatformError::Json)?;
    let Some(models) = payload.get("data").and_then(Value::as_array) else {
        return Err(OpenPlatformError::UnexpectedResponse(format!(
            "Unexpected models response for {}.",
            platform.base_url
        )));
    };
    models
        .iter()
        .filter_map(|item| match parse_model_info(item, "Model") {
            Ok(Some(model)) => Some(Ok(model)),
            Ok(None) => None,
            Err(error) => Some(Err(OpenPlatformError::Model(error))),
        })
        .collect()
}

fn build_headers(
    api_key: &str,
    custom_headers: &indexmap::IndexMap<String, String>,
) -> Result<reqwest::header::HeaderMap, OpenPlatformError> {
    let mut headers = reqwest::header::HeaderMap::new();
    for (name, value) in custom_headers {
        let name = reqwest::header::HeaderName::from_bytes(name.as_bytes())
            .map_err(|error| OpenPlatformError::InvalidHeader(error.to_string()))?;
        let value = reqwest::header::HeaderValue::from_str(value)
            .map_err(|error| OpenPlatformError::InvalidHeader(error.to_string()))?;
        headers.insert(name, value);
    }
    let authorization = reqwest::header::HeaderValue::from_str(&format!("Bearer {api_key}"))
        .map_err(|error| OpenPlatformError::InvalidHeader(error.to_string()))?;
    headers.insert(reqwest::header::AUTHORIZATION, authorization);
    headers.insert(
        reqwest::header::ACCEPT,
        reqwest::header::HeaderValue::from_static("application/json"),
    );
    Ok(headers)
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
    use crate::oauth::managed_models::ManagedKimiCodeProtocol;

    fn model(id: &str) -> ManagedKimiCodeModelInfo {
        ManagedKimiCodeModelInfo {
            id: id.to_owned(),
            context_length: 1_000,
            supports_reasoning: false,
            supports_image_in: false,
            supports_video_in: false,
            supports_tool_use: false,
            supports_thinking_type: None,
            support_efforts: None,
            default_effort: None,
            display_name: None,
            protocol: None,
        }
    }

    fn fake_platform_server(
        status: u16,
        body: &str,
    ) -> (
        OpenPlatformDefinition,
        Arc<Mutex<String>>,
        thread::JoinHandle<()>,
    ) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind platform server");
        let address = listener.local_addr().expect("platform server address");
        let request = Arc::new(Mutex::new(String::new()));
        let recorded = Arc::clone(&request);
        let body = body.to_owned();
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept platform request");
            let mut bytes = Vec::new();
            let mut buffer = [0_u8; 4_096];
            loop {
                let count = stream.read(&mut buffer).expect("read platform request");
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
                .expect("write platform response");
        });
        let base_url = Box::leak(format!("http://{address}/v1").into_boxed_str());
        (
            OpenPlatformDefinition {
                id: "test",
                name: "Test",
                base_url,
                console_url: None,
                allowed_prefixes: &[],
            },
            request,
            handle,
        )
    }

    #[test]
    fn exposes_the_two_original_platforms_and_exact_ids() {
        assert_eq!(
            get_open_platform_by_id("moonshot-cn").map(|p| p.base_url),
            Some("https://api.moonshot.cn/v1")
        );
        assert_eq!(
            get_open_platform_by_id("moonshot-ai").map(|p| p.console_url),
            Some(Some("https://platform.kimi.ai"))
        );
        assert!(is_open_platform_id("moonshot-cn"));
        assert!(!is_open_platform_id("kimi-code"));
    }

    #[test]
    fn thinking_declaration_overrides_legacy_reasoning_and_caps_keep_order() {
        let mut full = model("full");
        full.supports_reasoning = true;
        full.supports_image_in = true;
        full.supports_video_in = true;
        full.supports_tool_use = true;
        assert_eq!(
            capabilities_for_model(&full),
            Some(vec![
                "thinking".to_owned(),
                "image_in".to_owned(),
                "video_in".to_owned(),
                "tool_use".to_owned()
            ])
        );

        full.supports_thinking_type = Some(SupportsThinkingType::No);
        full.supports_image_in = false;
        full.supports_video_in = false;
        full.supports_tool_use = false;
        assert_eq!(capabilities_for_model(&full), None);

        full.supports_thinking_type = Some(SupportsThinkingType::Only);
        assert_eq!(
            capabilities_for_model(&full),
            Some(vec!["thinking".to_owned(), "always_thinking".to_owned()])
        );
    }

    #[test]
    fn prefix_filter_returns_owned_matches_or_every_model() {
        let models = vec![model("kimi-k2"), model("gpt-4")];
        assert_eq!(
            filter_models_by_prefix(&models, &OPEN_PLATFORMS[0]),
            vec![model("kimi-k2")]
        );
        let unrestricted = OpenPlatformDefinition {
            id: "x",
            name: "X",
            base_url: "https://x",
            console_url: None,
            allowed_prefixes: &[],
        };
        assert_eq!(filter_models_by_prefix(&models, &unrestricted), models);
    }

    #[test]
    fn auth_and_accept_override_case_insensitive_custom_header_names() {
        let custom = indexmap::IndexMap::from([
            ("authorization".to_owned(), "Bearer wrong".to_owned()),
            ("ACCEPT".to_owned(), "text/plain".to_owned()),
            ("X-Custom".to_owned(), "kept".to_owned()),
        ]);
        let headers = build_headers("right", &custom).expect("valid headers");
        assert_eq!(headers[reqwest::header::AUTHORIZATION], "Bearer right");
        assert_eq!(headers[reqwest::header::ACCEPT], "application/json");
        assert_eq!(headers["x-custom"], "kept");
    }

    #[tokio::test]
    async fn fetches_parses_and_sends_bearer_headers() {
        let (platform, request, handle) = fake_platform_server(
            200,
            r#"{"data":[{"id":"kimi-k2","context_length":256000,"supports_reasoning":true,"supports_image_in":true,"supports_video_in":true,"display_name":"Kimi K2","protocol":"anthropic"},{"invalid":true}]}"#,
        );
        let models = fetch_open_platform_models(&platform, "sk-test")
            .await
            .expect("models");
        handle.join().expect("platform server thread");
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].id, "kimi-k2");
        assert_eq!(models[0].protocol, Some(ManagedKimiCodeProtocol::Anthropic));
        assert!(models[0].supports_tool_use, "missing field defaults true");
        let request = request.lock().expect("request lock").to_ascii_lowercase();
        assert!(request.starts_with("get /v1/models http/1.1"));
        assert!(request.contains("authorization: bearer sk-test"));
        assert!(request.contains("accept: application/json"));
    }

    #[tokio::test]
    async fn preserves_api_status_and_rejects_unexpected_shapes() {
        let (platform, _, handle) =
            fake_platform_server(401, r#"{"error":{"message":"invalid API key"}}"#);
        let error = fetch_open_platform_models(&platform, "bad")
            .await
            .expect_err("api error");
        handle.join().expect("platform server thread");
        assert_eq!(error.status(), Some(401));
        assert_eq!(error.to_string(), "invalid API key");

        let (platform, _, handle) = fake_platform_server(200, r#"{}"#);
        let error = fetch_open_platform_models(&platform, "key")
            .await
            .expect_err("shape error");
        handle.join().expect("platform server thread");
        assert!(error.to_string().contains("Unexpected models response"));
    }
}
