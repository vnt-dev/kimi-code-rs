use std::{
    error::Error,
    fmt,
    path::{Path, PathBuf},
};

use async_trait::async_trait;
use indexmap::IndexMap;

use super::{
    managed_auth::KIMI_CODE_PROVIDER_NAME,
    managed_config::{ManagedKimiCodeApplyOptions, ManagedKimiCodeApplyResult},
    managed_models::{
        CredentialKind, ManagedKimiCodeModelInfo, ManagedModelsError,
        fetch_managed_kimi_code_models,
    },
};

#[async_trait]
pub trait ManagedKimiConfigAdapter: Send + Sync {
    type Config: Send;
    type Error: Error + Send + Sync + 'static;

    async fn read(&self) -> Result<Self::Config, Self::Error>;

    async fn write(&self, config: Self::Config) -> Result<(), Self::Error>;

    fn apply(
        &self,
        config: &mut Self::Config,
        options: ManagedKimiCodeApplyOptions<'_>,
    ) -> Result<ManagedKimiCodeApplyResult, Self::Error>;

    fn config_path(&self) -> Option<&Path> {
        None
    }

    fn supports_remove(&self) -> bool {
        false
    }

    fn remove(&self, _config: &mut Self::Config) -> Result<(), Self::Error> {
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct ProvisionManagedKimiCodeConfigOptions<'a, A> {
    pub adapter: &'a A,
    pub access_token: &'a str,
    pub base_url: Option<&'a str>,
    pub oauth_key: Option<&'a str>,
    pub oauth_host: Option<&'a str>,
    pub preserve_default_model: bool,
    pub headers: Option<&'a IndexMap<String, String>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedKimiCodeProvisionResult {
    pub provider_name: &'static str,
    pub default_model: String,
    pub default_thinking: bool,
    pub models: Vec<ManagedKimiCodeModelInfo>,
    pub config_path: Option<PathBuf>,
}

#[derive(Debug)]
pub enum ProvisionManagedKimiCodeError<E> {
    Models(ManagedModelsError),
    Adapter(E),
}

impl<E: fmt::Display> fmt::Display for ProvisionManagedKimiCodeError<E> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Models(error) => error.fmt(formatter),
            Self::Adapter(error) => error.fmt(formatter),
        }
    }
}

impl<E: Error + 'static> Error for ProvisionManagedKimiCodeError<E> {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Models(error) => Some(error),
            Self::Adapter(error) => Some(error),
        }
    }
}

// Original:
//   packages/oauth/src/managed-kimi-code.ts
//   provisionManagedKimiCodeConfigAfterLogin()
pub async fn provision_managed_kimi_code_config_after_login<A>(
    options: ProvisionManagedKimiCodeConfigOptions<'_, A>,
) -> Result<ManagedKimiCodeProvisionResult, ProvisionManagedKimiCodeError<A::Error>>
where
    A: ManagedKimiConfigAdapter,
{
    provision_managed_kimi_code_config(options).await
}

// Original:
//   packages/oauth/src/managed-kimi-code.ts
//   provisionManagedKimiCodeConfig()
pub async fn provision_managed_kimi_code_config<A>(
    options: ProvisionManagedKimiCodeConfigOptions<'_, A>,
) -> Result<ManagedKimiCodeProvisionResult, ProvisionManagedKimiCodeError<A::Error>>
where
    A: ManagedKimiConfigAdapter,
{
    let models = fetch_managed_kimi_code_models(
        options.access_token,
        options.base_url,
        options.headers,
        CredentialKind::OAuth,
    )
    .await
    .map_err(ProvisionManagedKimiCodeError::Models)?;
    let mut config = options
        .adapter
        .read()
        .await
        .map_err(ProvisionManagedKimiCodeError::Adapter)?;
    let applied = options
        .adapter
        .apply(
            &mut config,
            ManagedKimiCodeApplyOptions {
                models: &models,
                base_url: options.base_url,
                oauth_key: options.oauth_key,
                oauth_host: options.oauth_host,
                preserve_default_model: options.preserve_default_model,
            },
        )
        .map_err(ProvisionManagedKimiCodeError::Adapter)?;
    options
        .adapter
        .write(config)
        .await
        .map_err(ProvisionManagedKimiCodeError::Adapter)?;

    Ok(ManagedKimiCodeProvisionResult {
        provider_name: KIMI_CODE_PROVIDER_NAME,
        default_model: applied.default_model,
        default_thinking: applied.default_thinking,
        models,
        config_path: options.adapter.config_path().map(Path::to_path_buf),
    })
}

#[cfg(test)]
mod tests {
    use std::{
        io::{Read, Write},
        net::TcpListener,
        sync::Mutex,
        thread,
    };

    use serde_json::{Map, Value};

    use super::*;
    use crate::managed_config::{ManagedConfigError, apply_managed_kimi_code_config};

    #[derive(Debug, Clone, PartialEq, Eq)]
    struct TestAdapterError(String);

    impl fmt::Display for TestAdapterError {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str(&self.0)
        }
    }

    impl Error for TestAdapterError {}

    struct TestAdapter {
        initial: Value,
        events: Mutex<Vec<&'static str>>,
        written: Mutex<Option<Value>>,
        apply_error: bool,
        write_error: bool,
        path: PathBuf,
    }

    impl TestAdapter {
        fn new() -> Self {
            Self {
                initial: serde_json::json!({ "providers": {} }),
                events: Mutex::new(Vec::new()),
                written: Mutex::new(None),
                apply_error: false,
                write_error: false,
                path: PathBuf::from("C:/tmp/config.toml"),
            }
        }

        fn events(&self) -> Vec<&'static str> {
            self.events.lock().expect("events lock").clone()
        }
    }

    #[async_trait]
    impl ManagedKimiConfigAdapter for TestAdapter {
        type Config = Map<String, Value>;
        type Error = TestAdapterError;

        async fn read(&self) -> Result<Self::Config, Self::Error> {
            self.events.lock().expect("events lock").push("read");
            self.initial
                .as_object()
                .cloned()
                .ok_or_else(|| TestAdapterError("config is not an object".to_owned()))
        }

        async fn write(&self, config: Self::Config) -> Result<(), Self::Error> {
            self.events.lock().expect("events lock").push("write");
            if self.write_error {
                return Err(TestAdapterError("write failed".to_owned()));
            }
            *self.written.lock().expect("written lock") = Some(Value::Object(config));
            Ok(())
        }

        fn apply(
            &self,
            config: &mut Self::Config,
            options: ManagedKimiCodeApplyOptions<'_>,
        ) -> Result<ManagedKimiCodeApplyResult, Self::Error> {
            self.events.lock().expect("events lock").push("apply");
            if self.apply_error {
                return Err(TestAdapterError("apply failed".to_owned()));
            }
            apply_managed_kimi_code_config(config, options).map_err(map_config_error)
        }

        fn config_path(&self) -> Option<&Path> {
            Some(&self.path)
        }
    }

    fn map_config_error(error: ManagedConfigError) -> TestAdapterError {
        TestAdapterError(error.to_string())
    }

    fn models_server(status: u16, body: &str) -> (String, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind models server");
        let address = listener.local_addr().expect("models server address");
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
            let response = format!(
                "HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream
                .write_all(response.as_bytes())
                .expect("write models response");
        });
        (format!("http://{address}/coding/v1"), handle)
    }

    fn options<'a>(
        adapter: &'a TestAdapter,
        base_url: &'a str,
    ) -> ProvisionManagedKimiCodeConfigOptions<'a, TestAdapter> {
        ProvisionManagedKimiCodeConfigOptions {
            adapter,
            access_token: "oauth-token",
            base_url: Some(base_url),
            oauth_key: Some("oauth/test"),
            oauth_host: Some("https://auth.test"),
            preserve_default_model: false,
            headers: None,
        }
    }

    #[tokio::test]
    async fn provisions_in_fetch_read_apply_write_order_and_returns_metadata() {
        let (base_url, server) = models_server(
            200,
            r#"{"data":[{"id":"kimi-for-coding","context_length":262144,"supports_reasoning":true}]}"#,
        );
        let adapter = TestAdapter::new();

        let result = provision_managed_kimi_code_config_after_login(options(&adapter, &base_url))
            .await
            .expect("provision config");
        server.join().expect("models server thread");

        assert_eq!(adapter.events(), vec!["read", "apply", "write"]);
        assert_eq!(result.provider_name, "managed:kimi-code");
        assert_eq!(result.default_model, "kimi-code/kimi-for-coding");
        assert!(result.default_thinking);
        assert_eq!(result.models.len(), 1);
        assert_eq!(
            result.config_path,
            Some(PathBuf::from("C:/tmp/config.toml"))
        );
        let written = adapter.written.lock().expect("written lock");
        let written = written.as_ref().expect("written config");
        assert_eq!(written["defaultModel"], "kimi-code/kimi-for-coding");
        assert_eq!(
            written["providers"]["managed:kimi-code"]["oauth"]["key"],
            "oauth/test"
        );
    }

    #[tokio::test]
    async fn fetch_failure_does_not_read_or_write_config() {
        let (base_url, server) = models_server(401, r#"{"error":{"message":"rejected"}}"#);
        let adapter = TestAdapter::new();

        let error = provision_managed_kimi_code_config(options(&adapter, &base_url))
            .await
            .expect_err("fetch failure");
        server.join().expect("models server thread");

        assert!(matches!(error, ProvisionManagedKimiCodeError::Models(_)));
        assert!(adapter.events().is_empty());
        assert!(adapter.written.lock().expect("written lock").is_none());
    }

    #[tokio::test]
    async fn apply_failure_skips_write() {
        let (base_url, server) = models_server(
            200,
            r#"{"data":[{"id":"kimi-for-coding","context_length":262144}]}"#,
        );
        let mut adapter = TestAdapter::new();
        adapter.apply_error = true;

        let error = provision_managed_kimi_code_config(options(&adapter, &base_url))
            .await
            .expect_err("apply failure");
        server.join().expect("models server thread");

        assert!(matches!(error, ProvisionManagedKimiCodeError::Adapter(_)));
        assert_eq!(adapter.events(), vec!["read", "apply"]);
        assert!(adapter.written.lock().expect("written lock").is_none());
    }

    #[tokio::test]
    async fn write_failure_is_propagated_after_apply() {
        let (base_url, server) = models_server(
            200,
            r#"{"data":[{"id":"kimi-for-coding","context_length":262144}]}"#,
        );
        let mut adapter = TestAdapter::new();
        adapter.write_error = true;

        let error = provision_managed_kimi_code_config(options(&adapter, &base_url))
            .await
            .expect_err("write failure");
        server.join().expect("models server thread");

        assert_eq!(error.to_string(), "write failed");
        assert_eq!(adapter.events(), vec!["read", "apply", "write"]);
    }
}
