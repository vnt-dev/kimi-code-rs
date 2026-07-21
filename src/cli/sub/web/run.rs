use std::{error::Error, fmt};

use async_trait::async_trait;
use url::Url;

use super::{
    access_urls::{access_url_lines, build_openable_url, is_loopback_host, split_token_fragment},
    networks::NetworkAddress,
    shared::{
        DEFAULT_FOREGROUND_LOG_LEVEL, ParsedServerOptions, ServerCliOptions, ServerOptionError,
        parse_server_options,
    },
};

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WebCliOptions {
    pub server: ServerCliOptions,
    pub open: bool,
}

#[derive(Debug)]
pub struct WebRuntimeError(Box<dyn Error + Send + Sync>);

impl WebRuntimeError {
    pub fn new(error: impl Error + Send + Sync + 'static) -> Self {
        Self(Box::new(error))
    }
}

impl fmt::Display for WebRuntimeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl Error for WebRuntimeError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(self.0.as_ref())
    }
}

#[derive(Debug)]
pub enum WebCommandError {
    Options(ServerOptionError),
    Runtime(WebRuntimeError),
    InvalidOrigin(url::ParseError),
}

impl fmt::Display for WebCommandError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Options(error) => error.fmt(formatter),
            Self::Runtime(error) => error.fmt(formatter),
            Self::InvalidOrigin(error) => error.fmt(formatter),
        }
    }
}

impl Error for WebCommandError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Options(error) => Some(error),
            Self::Runtime(error) => Some(error),
            Self::InvalidOrigin(error) => Some(error),
        }
    }
}

impl From<ServerOptionError> for WebCommandError {
    fn from(error: ServerOptionError) -> Self {
        Self::Options(error)
    }
}

impl From<WebRuntimeError> for WebCommandError {
    fn from(error: WebRuntimeError) -> Self {
        Self::Runtime(error)
    }
}

#[async_trait]
pub trait WebCommandRuntime: Send + Sync {
    async fn start_server_foreground(
        &self,
        options: ParsedServerOptions,
        on_ready: &mut (dyn FnMut(String) -> Result<(), WebCommandError> + Send),
    ) -> Result<(), WebRuntimeError>;

    fn resolve_token(&self) -> Option<String>;

    fn network_addresses(&self) -> Option<Vec<NetworkAddress>>;

    fn open_url(&self, url: &str);

    fn version(&self) -> &str;

    fn color_enabled(&self) -> bool;

    fn write_stdout(&self, text: &str);
}

// Original:
//   apps/kimi-code/src/cli/sub/web/run.ts
//   handleWebCommand()
pub async fn handle_web_command(
    runtime: &dyn WebCommandRuntime,
    options: &WebCliOptions,
) -> Result<(), WebCommandError> {
    let parsed = parse_server_options(&options.server)?;
    let mut on_ready = |origin: String| {
        // Resolve only after the server reports ready because first startup
        // creates the persistent token file.
        let token = if parsed.dangerous_bypass_auth {
            None
        } else {
            runtime.resolve_token()
        };
        let output = if parsed.log_level == DEFAULT_FOREGROUND_LOG_LEVEL {
            format_ready_banner(
                &origin,
                &parsed.host,
                FormatReadyBannerOptions {
                    token: token.as_deref(),
                    network_addresses: runtime.network_addresses().as_deref(),
                    dangerous_bypass_auth: parsed.dangerous_bypass_auth,
                    version: runtime.version(),
                    color: runtime.color_enabled(),
                },
            )?
        } else {
            format_ready_line(
                &origin,
                token.as_deref(),
                parsed.dangerous_bypass_auth,
                runtime.color_enabled(),
            )
        };
        runtime.write_stdout(&output);
        if options.open {
            let target = token
                .as_deref()
                .map_or_else(|| origin.clone(), |token| build_web_url(&origin, token));
            runtime.open_url(&target);
        }
        Ok(())
    };
    runtime
        .start_server_foreground(parsed.clone(), &mut on_ready)
        .await?;
    Ok(())
}

pub fn build_web_url(origin: &str, token: &str) -> String {
    build_openable_url(origin, Some(token))
}

pub fn format_ready_line(
    origin: &str,
    token: Option<&str>,
    dangerous_bypass_auth: bool,
    color: bool,
) -> String {
    let notice = if dangerous_bypass_auth {
        format!("{}\n", format_danger_notice_lines(color).join("\n"))
    } else {
        String::new()
    };
    format!(
        "{notice}Kimi server: {}\n",
        build_openable_url(origin, token)
    )
}

#[derive(Debug, Clone, Copy)]
pub struct FormatReadyBannerOptions<'a> {
    pub token: Option<&'a str>,
    pub network_addresses: Option<&'a [NetworkAddress]>,
    pub dangerous_bypass_auth: bool,
    pub version: &'a str,
    pub color: bool,
}

// Original:
//   apps/kimi-code/src/cli/sub/web/run.ts
//   formatReadyBanner()
pub fn format_ready_banner(
    origin: &str,
    host: &str,
    options: FormatReadyBannerOptions<'_>,
) -> Result<String, WebCommandError> {
    let url = Url::parse(origin).map_err(WebCommandError::InvalidOrigin)?;
    let port = url.port().unwrap_or(0);
    let style = BannerStyle {
        color: options.color,
    };
    let mut lines = vec![
        String::new(),
        format!(
            "  {}  {}  {}",
            style.primary("▐█▛█▛█▌"),
            style.primary_bold("Kimi server ready"),
            style.dim(options.version)
        ),
        format!(
            "  {}  {}",
            style.primary("▐█████▌"),
            style.dim("Local web UI is available from this machine.")
        ),
        String::new(),
    ];
    if options.dangerous_bypass_auth {
        lines.extend(format_danger_notice_lines(options.color));
        lines.push(String::new());
    }

    for line in access_url_lines(host, port, options.token, options.network_addresses) {
        let (base, fragment) = split_token_fragment(&line.url);
        let rendered_url = if fragment.is_empty() {
            style.accent(base)
        } else {
            format!("{}{}", style.accent(base), style.dim(fragment))
        };
        lines.push(format!("  {}{rendered_url}", style.label(line.label)));
    }
    if is_loopback_host(host) {
        lines.push(format!(
            "  {}{}{}",
            style.label("Network:  "),
            style.muted("off"),
            style.dim("  use --host to enable")
        ));
    }
    if let Some(token) = options.token {
        lines.push(String::new());
        lines.push(format!("  {}{token}", style.label("Token:    ")));
        lines.push(String::new());
    }
    lines.push(format!(
        "  {}{}{}",
        style.label("Logs:     "),
        style.muted("off"),
        style.dim("  use --log-level info to enable")
    ));
    lines.push(format!(
        "  {}{}",
        style.label("Stop:     "),
        style.muted("Ctrl+C")
    ));
    lines.push(String::new());
    Ok(lines.join("\n"))
}

fn format_danger_notice_lines(color: bool) -> Vec<String> {
    let style = BannerStyle { color };
    vec![
        format!(
            "  {}",
            style.error_bold("⚠ DANGER: authentication is DISABLED (--dangerous-bypass-auth).")
        ),
        format!(
            "  {}",
            style.error(
                "Anyone who can reach this port gets full access. Only continue if you understand the risk."
            )
        ),
        format!(
            "  {}{}{}",
            style.error("If you are unsure, stop this process now with "),
            style.error_bold("Ctrl+C"),
            style.error(".")
        ),
    ]
}

struct BannerStyle {
    color: bool,
}

impl BannerStyle {
    fn primary(&self, text: &str) -> String {
        self.paint(text, (79, 168, 255), false)
    }

    fn primary_bold(&self, text: &str) -> String {
        self.paint(text, (79, 168, 255), true)
    }

    fn accent(&self, text: &str) -> String {
        self.paint(text, (91, 192, 190), false)
    }

    fn dim(&self, text: &str) -> String {
        self.paint(text, (136, 136, 136), false)
    }

    fn muted(&self, text: &str) -> String {
        self.paint(text, (107, 107, 107), false)
    }

    fn label(&self, text: &str) -> String {
        self.paint(text, (136, 136, 136), true)
    }

    fn error(&self, text: &str) -> String {
        self.paint(text, (232, 84, 84), false)
    }

    fn error_bold(&self, text: &str) -> String {
        self.paint(text, (232, 84, 84), true)
    }

    fn paint(&self, text: &str, (red, green, blue): (u8, u8, u8), bold: bool) -> String {
        if !self.color {
            return text.to_owned();
        }
        let weight = if bold { "1;" } else { "" };
        format!("\u{1b}[{weight}38;2;{red};{green};{blue}m{text}\u{1b}[0m")
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::super::{
        networks::AddressFamily,
        shared::{HostInput, ServerLogLevel},
    };
    use super::*;

    #[derive(Debug, Clone, Copy)]
    struct TestError;

    impl fmt::Display for TestError {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("server failed")
        }
    }

    impl Error for TestError {}

    struct RuntimeMock {
        origin: String,
        token: Option<String>,
        addresses: Option<Vec<NetworkAddress>>,
        color: bool,
        fail: bool,
        starts: Mutex<Vec<ParsedServerOptions>>,
        opened: Mutex<Vec<String>>,
        stdout: Mutex<String>,
    }

    impl RuntimeMock {
        fn new(origin: &str) -> Self {
            Self {
                origin: origin.to_owned(),
                token: None,
                addresses: None,
                color: false,
                fail: false,
                starts: Mutex::new(Vec::new()),
                opened: Mutex::new(Vec::new()),
                stdout: Mutex::new(String::new()),
            }
        }
    }

    #[async_trait]
    impl WebCommandRuntime for RuntimeMock {
        async fn start_server_foreground(
            &self,
            options: ParsedServerOptions,
            on_ready: &mut (dyn FnMut(String) -> Result<(), WebCommandError> + Send),
        ) -> Result<(), WebRuntimeError> {
            self.starts.lock().expect("starts").push(options);
            if self.fail {
                return Err(WebRuntimeError::new(TestError));
            }
            on_ready(self.origin.clone())
                .map_err(|error| WebRuntimeError::new(TestMessage(error.to_string())))?;
            Ok(())
        }

        fn resolve_token(&self) -> Option<String> {
            self.token.clone()
        }

        fn network_addresses(&self) -> Option<Vec<NetworkAddress>> {
            self.addresses.clone()
        }

        fn open_url(&self, url: &str) {
            self.opened.lock().expect("opened").push(url.to_owned());
        }

        fn version(&self) -> &str {
            "1.2.3-test"
        }

        fn color_enabled(&self) -> bool {
            self.color
        }

        fn write_stdout(&self, text: &str) {
            self.stdout.lock().expect("stdout").push_str(text);
        }
    }

    #[derive(Debug)]
    struct TestMessage(String);

    impl fmt::Display for TestMessage {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str(&self.0)
        }
    }

    impl Error for TestMessage {}

    #[tokio::test]
    async fn prints_full_ready_banner_using_actual_bound_port_and_token() {
        let mut runtime = RuntimeMock::new("http://127.0.0.1:58628");
        runtime.token = Some("tok".to_owned());
        let options = WebCliOptions {
            server: ServerCliOptions {
                port: Some("58627".to_owned()),
                ..ServerCliOptions::default()
            },
            open: false,
        };

        handle_web_command(&runtime, &options).await.expect("web");

        let output = runtime.stdout.lock().expect("stdout");
        assert!(output.contains("Kimi server ready"));
        assert!(output.contains("http://127.0.0.1:58628/#token=tok"));
        assert!(output.contains("Network:  off  use --host to enable"));
        assert!(output.contains("Token:    tok"));
        assert!(output.contains("▐█▛█▛█▌"));
    }

    #[tokio::test]
    async fn wildcard_banner_lists_injected_network_addresses() {
        let mut runtime = RuntimeMock::new("http://0.0.0.0:58627");
        runtime.token = Some("tok-xyz".to_owned());
        runtime.addresses = Some(vec![NetworkAddress {
            address: "192.168.98.66".to_owned(),
            family: AddressFamily::Ipv4,
        }]);
        let options = WebCliOptions {
            server: ServerCliOptions {
                host: HostInput::Value("0.0.0.0".to_owned()),
                ..ServerCliOptions::default()
            },
            open: false,
        };

        handle_web_command(&runtime, &options).await.expect("web");

        let output = runtime.stdout.lock().expect("stdout");
        assert!(output.contains("http://localhost:58627/#token=tok-xyz"));
        assert!(output.contains("http://192.168.98.66:58627/#token=tok-xyz"));
    }

    #[tokio::test]
    async fn opens_token_url_only_after_ready() {
        let mut runtime = RuntimeMock::new("http://127.0.0.1:58627");
        runtime.token = Some("tok-xyz".to_owned());

        handle_web_command(
            &runtime,
            &WebCliOptions {
                server: ServerCliOptions::default(),
                open: true,
            },
        )
        .await
        .expect("web");

        assert_eq!(
            runtime.opened.lock().expect("opened").as_slice(),
            ["http://127.0.0.1:58627/#token=tok-xyz"]
        );
    }

    #[tokio::test]
    async fn auth_bypass_prints_danger_suppresses_token_and_opens_plain_origin() {
        let mut runtime = RuntimeMock::new("http://127.0.0.1:58627");
        runtime.token = Some("must-not-leak".to_owned());
        let options = WebCliOptions {
            server: ServerCliOptions {
                dangerous_bypass_auth: true,
                ..ServerCliOptions::default()
            },
            open: true,
        };

        handle_web_command(&runtime, &options).await.expect("web");

        let output = runtime.stdout.lock().expect("stdout");
        assert!(output.contains("⚠ DANGER: authentication is DISABLED"));
        assert!(!output.contains("must-not-leak"));
        assert!(!output.contains("#token="));
        assert_eq!(
            runtime.opened.lock().expect("opened").as_slice(),
            ["http://127.0.0.1:58627"]
        );
    }

    #[tokio::test]
    async fn non_default_log_level_uses_compact_ready_line() {
        let mut runtime = RuntimeMock::new("http://127.0.0.1:58627");
        runtime.token = Some("tok".to_owned());
        let options = WebCliOptions {
            server: ServerCliOptions {
                log_level: Some("info".to_owned()),
                ..ServerCliOptions::default()
            },
            open: false,
        };

        handle_web_command(&runtime, &options).await.expect("web");

        assert_eq!(
            runtime.stdout.lock().expect("stdout").as_str(),
            "Kimi server: http://127.0.0.1:58627/#token=tok\n"
        );
    }

    #[tokio::test]
    async fn invalid_options_fail_before_starting_the_server() {
        let runtime = RuntimeMock::new("http://127.0.0.1:58627");
        let result = handle_web_command(
            &runtime,
            &WebCliOptions {
                server: ServerCliOptions {
                    log_level: Some("shout".to_owned()),
                    ..ServerCliOptions::default()
                },
                open: false,
            },
        )
        .await;

        assert!(
            result
                .expect_err("invalid log level")
                .to_string()
                .contains("invalid --log-level")
        );
        assert!(runtime.starts.lock().expect("starts").is_empty());
    }

    #[test]
    fn color_mode_uses_the_dark_palette_truecolor_sequences() {
        let output = format_ready_banner(
            "http://127.0.0.1:58627",
            "127.0.0.1",
            FormatReadyBannerOptions {
                token: None,
                network_addresses: None,
                dangerous_bypass_auth: true,
                version: "1.2.3",
                color: true,
            },
        )
        .expect("banner");
        assert!(output.contains("\u{1b}[38;2;79;168;255m▐█▛█▛█▌\u{1b}[0m"));
        assert!(output.contains("\u{1b}[1;38;2;79;168;255mKimi server ready\u{1b}[0m"));
        assert!(output.contains("\u{1b}[1;38;2;232;84;84m⚠ DANGER"));
    }

    #[test]
    fn ready_line_always_warns_when_auth_is_bypassed() {
        let output = format_ready_line("http://localhost:1", None, true, false);
        assert!(output.starts_with("  ⚠ DANGER"));
        assert!(output.ends_with("Kimi server: http://localhost:1/\n"));
    }

    #[tokio::test]
    async fn parsed_options_are_threaded_to_the_runner() {
        let mut runtime = RuntimeMock::new("http://0.0.0.0:59000");
        runtime.fail = true;
        let options = WebCliOptions {
            server: ServerCliOptions {
                host: HostInput::Bare,
                port: Some("59000".to_owned()),
                log_level: Some(ServerLogLevel::Debug.as_str().to_owned()),
                debug_endpoints: true,
                insecure_no_tls: true,
                allow_remote_shutdown: true,
                allow_remote_terminals: true,
                dangerous_bypass_auth: true,
                allowed_hosts: vec![".example.com".to_owned()],
            },
            open: false,
        };

        let error = handle_web_command(&runtime, &options)
            .await
            .expect_err("server error");
        assert_eq!(error.to_string(), "server failed");
        assert_eq!(
            runtime.starts.lock().expect("starts").as_slice(),
            [ParsedServerOptions {
                host: "0.0.0.0".to_owned(),
                port: 59_000,
                log_level: ServerLogLevel::Debug,
                debug_endpoints: true,
                insecure_no_tls: true,
                allow_remote_shutdown: true,
                allow_remote_terminals: true,
                dangerous_bypass_auth: true,
                allowed_hosts: vec![".example.com".to_owned()],
            }]
        );
    }
}
