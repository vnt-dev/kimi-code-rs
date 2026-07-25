use async_trait::async_trait;
use indexmap::IndexMap;
use reqwest::header::{AUTHORIZATION, HeaderMap, HeaderName, HeaderValue};
use reqwest::multipart::{Form, Part};
use serde_json::Value;
use std::path::Path;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

use crate::kosong::contract::errors::ChatProviderError;
use crate::kosong::contract::message::{ContentPart, MediaUrl};
use crate::kosong::contract::provider::{
    GenerateOptions, ProviderError, ProviderRequestAuth, VideoUploadInput, VideoUploadSource,
};
use crate::kosong::provider::bases::openai::openai_common::{
    convert_openai_error, convert_openai_status_error,
};
use crate::kosong::provider::bases::request_auth::{
    merge_request_headers, require_provider_api_key,
};

pub type KimiFilesClientFactory =
    Arc<dyn Fn(ProviderRequestAuth) -> Arc<dyn KimiFilesClient> + Send + Sync>;

#[async_trait]
pub trait KimiFilesClient: Send + Sync {
    async fn upload(
        &self,
        file: KimiUploadFile,
        signal: Option<&CancellationToken>,
    ) -> Result<String, ProviderError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KimiUploadFile {
    pub data: Vec<u8>,
    pub mime_type: String,
    pub filename: String,
}

pub struct KimiFilesOptions {
    pub api_key: Option<String>,
    pub base_url: String,
    pub default_headers: Option<IndexMap<String, String>>,
    pub client_factory: Option<KimiFilesClientFactory>,
}

pub struct KimiFiles {
    api_key: Option<String>,
    base_url: String,
    default_headers: Option<IndexMap<String, String>>,
    cached_client: Option<Arc<dyn KimiFilesClient>>,
    client_factory: Option<KimiFilesClientFactory>,
}

impl KimiFiles {
    pub fn new(options: KimiFilesOptions) -> Self {
        let cached_client = options
            .api_key
            .as_deref()
            .filter(|api_key| !api_key.is_empty())
            .map(|api_key| {
                Arc::new(ReqwestKimiFilesClient {
                    api_key: api_key.to_owned(),
                    base_url: options.base_url.clone(),
                    headers: options.default_headers.clone(),
                    client: reqwest::Client::new(),
                }) as Arc<dyn KimiFilesClient>
            });
        Self {
            api_key: options.api_key,
            base_url: options.base_url,
            default_headers: options.default_headers,
            cached_client,
            client_factory: options.client_factory,
        }
    }

    // Original: kimi-files.ts, KimiFiles.uploadVideo().
    pub async fn upload_video(
        &self,
        input: &VideoUploadSource,
        options: Option<&GenerateOptions>,
    ) -> Result<ContentPart, ProviderError> {
        let file = match input {
            VideoUploadSource::Location(location) => file_from_location(location).await?,
            VideoUploadSource::Data(input) => file_from_data(input)?,
        };
        let auth = options.and_then(|options| options.auth.as_ref());
        let client = self.create_client(auth)?;
        let id = client
            .upload(file, options.and_then(|options| options.signal.as_ref()))
            .await?;
        Ok(ContentPart::VideoUrl {
            video_url: MediaUrl {
                url: format!("ms://{id}"),
                id: Some(id),
            },
        })
    }

    fn create_client(
        &self,
        auth: Option<&ProviderRequestAuth>,
    ) -> Result<Arc<dyn KimiFilesClient>, ProviderError> {
        if let Some(factory) = self.client_factory.as_ref() {
            return Ok(factory(auth.cloned().unwrap_or_default()));
        }
        if auth.is_none()
            && let Some(client) = self.cached_client.as_ref()
        {
            return Ok(Arc::clone(client));
        }
        let api_key =
            require_provider_api_key("KimiFiles.uploadVideo", auth, self.api_key.as_deref())
                .map_err(boxed)?;
        Ok(Arc::new(ReqwestKimiFilesClient {
            api_key,
            base_url: self.base_url.clone(),
            headers: merge_request_headers(
                self.default_headers.as_ref(),
                auth.and_then(|auth| auth.headers.as_ref()),
            ),
            client: reqwest::Client::new(),
        }))
    }
}

async fn file_from_location(location: &str) -> Result<KimiUploadFile, ProviderError> {
    let path = Path::new(location);
    if !path.exists() {
        return Err(boxed(ChatProviderError::ChatProvider {
            message: format!("Video file not found: {location}"),
        }));
    }
    let filename = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let Some(mime_type) = guess_mime_type_from_ext(&filename) else {
        return Err(boxed(ChatProviderError::ChatProvider {
            message: format!(
                "KimiFiles.uploadVideo: file extension does not indicate a video type: {filename}"
            ),
        }));
    };
    let data = tokio::fs::read(path).await.map_err(|error| {
        boxed(ChatProviderError::ChatProvider {
            message: error.to_string(),
        })
    })?;
    Ok(KimiUploadFile {
        data,
        mime_type: mime_type.to_owned(),
        filename,
    })
}

fn file_from_data(input: &VideoUploadInput) -> Result<KimiUploadFile, ProviderError> {
    if !input.mime_type.starts_with("video/") {
        return Err(boxed(ChatProviderError::ChatProvider {
            message: format!("Expected a video mime type, got {}", input.mime_type),
        }));
    }
    Ok(KimiUploadFile {
        data: input.data.clone(),
        mime_type: input.mime_type.clone(),
        filename: input
            .filename
            .clone()
            .unwrap_or_else(|| guess_filename(&input.mime_type)),
    })
}

fn guess_filename(mime_type: &str) -> String {
    format!(
        "upload.{}",
        mime_to_ext(&mime_type.to_ascii_lowercase()).unwrap_or("bin")
    )
}

fn mime_to_ext(mime_type: &str) -> Option<&'static str> {
    match mime_type {
        "video/mp4" => Some("mp4"),
        "video/mpeg" => Some("mpeg"),
        "video/quicktime" => Some("mov"),
        "video/webm" => Some("webm"),
        "video/x-matroska" => Some("mkv"),
        "video/x-msvideo" => Some("avi"),
        "video/x-flv" => Some("flv"),
        "video/3gpp" => Some("3gp"),
        _ => None,
    }
}

fn guess_mime_type_from_ext(filename: &str) -> Option<&'static str> {
    match filename.rsplit_once('.')?.1.to_ascii_lowercase().as_str() {
        "mp4" => Some("video/mp4"),
        "mpeg" => Some("video/mpeg"),
        "mov" => Some("video/quicktime"),
        "webm" => Some("video/webm"),
        "mkv" => Some("video/x-matroska"),
        "avi" => Some("video/x-msvideo"),
        "flv" => Some("video/x-flv"),
        "3gp" => Some("video/3gpp"),
        _ => None,
    }
}

struct ReqwestKimiFilesClient {
    api_key: String,
    base_url: String,
    headers: Option<IndexMap<String, String>>,
    client: reqwest::Client,
}

#[async_trait]
impl KimiFilesClient for ReqwestKimiFilesClient {
    async fn upload(
        &self,
        file: KimiUploadFile,
        signal: Option<&CancellationToken>,
    ) -> Result<String, ProviderError> {
        let part = Part::bytes(file.data)
            .file_name(file.filename)
            .mime_str(&file.mime_type)
            .map_err(|error| {
                boxed(ChatProviderError::ChatProvider {
                    message: error.to_string(),
                })
            })?;
        let form = Form::new()
            .part("file", part)
            .text("purpose", "video".to_owned());
        let request = self
            .client
            .post(format!("{}/files", self.base_url.trim_end_matches('/')))
            .headers(build_headers(&self.api_key, self.headers.as_ref())?)
            .multipart(form)
            .send();
        let response = if let Some(signal) = signal {
            tokio::select! {
                biased;
                _ = signal.cancelled() => return Err(boxed(ChatProviderError::Abort)),
                response = request => response,
            }
        } else {
            request.await
        }
        .map_err(convert_openai_error)
        .map_err(boxed)?;
        let status = response.status();
        let headers = response.headers().clone();
        let body = response
            .text()
            .await
            .map_err(convert_openai_error)
            .map_err(boxed)?;
        let value = serde_json::from_str::<Value>(&body).unwrap_or(Value::Null);
        if !status.is_success() {
            let message = value
                .get("error")
                .and_then(|error| error.get("message"))
                .and_then(Value::as_str)
                .or_else(|| value.get("message").and_then(Value::as_str))
                .unwrap_or(&body);
            return Err(boxed(convert_openai_status_error(
                status.as_u16(),
                message,
                &headers,
            )));
        }
        value
            .get("id")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| {
                boxed(ChatProviderError::ChatProvider {
                    message: "KimiFiles.uploadVideo: response is missing file id".to_owned(),
                })
            })
    }
}

fn build_headers(
    api_key: &str,
    headers: Option<&IndexMap<String, String>>,
) -> Result<HeaderMap, ProviderError> {
    let mut result = HeaderMap::new();
    let authorization = HeaderValue::from_str(&format!("Bearer {api_key}")).map_err(|error| {
        boxed(ChatProviderError::ChatProvider {
            message: format!("KimiFiles.uploadVideo: invalid apiKey header: {error}"),
        })
    })?;
    result.insert(AUTHORIZATION, authorization);
    if let Some(headers) = headers {
        for (name, value) in headers {
            let name = HeaderName::from_bytes(name.as_bytes()).map_err(|error| {
                boxed(ChatProviderError::ChatProvider {
                    message: format!("KimiFiles.uploadVideo: invalid header name: {error}"),
                })
            })?;
            let value = HeaderValue::from_str(value).map_err(|error| {
                boxed(ChatProviderError::ChatProvider {
                    message: format!("KimiFiles.uploadVideo: invalid header value: {error}"),
                })
            })?;
            result.insert(name, value);
        }
    }
    Ok(result)
}

fn boxed(error: ChatProviderError) -> ProviderError {
    Box::new(error)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    struct CapturingClient {
        files: Arc<Mutex<Vec<KimiUploadFile>>>,
    }

    #[async_trait]
    impl KimiFilesClient for CapturingClient {
        async fn upload(
            &self,
            file: KimiUploadFile,
            _signal: Option<&CancellationToken>,
        ) -> Result<String, ProviderError> {
            self.files.lock().unwrap().push(file);
            Ok("file_abc123".to_owned())
        }
    }

    #[tokio::test]
    async fn uploads_bytes_with_request_auth_and_default_filename() {
        let auths = Arc::new(Mutex::new(Vec::new()));
        let captured = Arc::new(Mutex::new(Vec::new()));
        let files = KimiFiles::new(KimiFilesOptions {
            api_key: None,
            base_url: "https://api.example/v1".to_owned(),
            default_headers: None,
            client_factory: Some({
                let auths = Arc::clone(&auths);
                let captured = Arc::clone(&captured);
                Arc::new(move |auth| {
                    auths.lock().unwrap().push(auth);
                    Arc::new(CapturingClient {
                        files: Arc::clone(&captured),
                    })
                })
            }),
        });
        let part = files
            .upload_video(
                &VideoUploadSource::Data(VideoUploadInput {
                    data: vec![1, 2, 3],
                    mime_type: "video/mp4".to_owned(),
                    filename: None,
                }),
                Some(&GenerateOptions {
                    auth: Some(ProviderRequestAuth {
                        api_key: Some("request-token".to_owned()),
                        headers: None,
                    }),
                    ..GenerateOptions::default()
                }),
            )
            .await
            .unwrap();
        assert_eq!(
            auths.lock().unwrap()[0].api_key.as_deref(),
            Some("request-token")
        );
        assert_eq!(captured.lock().unwrap()[0].filename, "upload.mp4");
        assert_eq!(
            part,
            ContentPart::VideoUrl {
                video_url: MediaUrl {
                    url: "ms://file_abc123".to_owned(),
                    id: Some("file_abc123".to_owned())
                }
            }
        );
    }

    #[tokio::test]
    async fn rejects_non_video_inputs_before_creating_a_client() {
        let files = KimiFiles::new(KimiFilesOptions {
            api_key: Some("key".to_owned()),
            base_url: "https://api.example/v1".to_owned(),
            default_headers: None,
            client_factory: None,
        });
        let error = files
            .upload_video(
                &VideoUploadSource::Data(VideoUploadInput {
                    data: vec![],
                    mime_type: "image/png".to_owned(),
                    filename: None,
                }),
                None,
            )
            .await
            .unwrap_err();
        assert_eq!(
            error.to_string(),
            "Expected a video mime type, got image/png"
        );
    }
}
