//! Shared HTTP client defaults for built-in model providers.

use std::time::Duration;

pub const PROVIDER_REQUEST_TIMEOUT: Duration = Duration::from_secs(5 * 60);

pub fn default_provider_http_client() -> reqwest::Client {
    provider_http_client(PROVIDER_REQUEST_TIMEOUT)
}

fn provider_http_client(timeout: Duration) -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(timeout)
        .read_timeout(timeout)
        .build()
        .expect("the built-in provider HTTP client configuration must be valid")
}

#[cfg(test)]
mod tests {
    use tokio::{io::AsyncReadExt, net::TcpListener};

    use super::*;

    #[tokio::test]
    async fn provider_http_client_enforces_its_total_request_timeout() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0_u8; 1024];
            let _ = socket.read(&mut request).await;
            tokio::time::sleep(Duration::from_secs(1)).await;
        });

        let error = provider_http_client(Duration::from_millis(20))
            .get(format!("http://{address}"))
            .send()
            .await
            .unwrap_err();

        assert!(error.is_timeout());
        server.abort();
    }
}
