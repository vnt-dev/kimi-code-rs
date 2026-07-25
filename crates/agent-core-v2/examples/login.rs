use async_trait::async_trait;
use kimi_code_agent_core_v2::app::{
    auth::{OAuthToolkitContract, OAuthToolkitService},
    bootstrap::{BootstrapInput, ensure_kimi_home, resolve_bootstrap_options},
};
use kimi_code_oauth::{
    DeviceAuthorization, DeviceCodeObserver, KimiOAuthLoginOptions, OAuthManagerError,
};

const PROVIDER: &str = "kimi-code";

struct PrintDeviceCode;

#[async_trait]
impl DeviceCodeObserver for PrintDeviceCode {
    async fn on_device_code(
        &self,
        authorization: &DeviceAuthorization,
    ) -> Result<(), OAuthManagerError> {
        println!("请打开以下地址完成登录：");
        println!("{}", authorization.verification_uri_complete);
        Ok(())
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let bootstrap = resolve_bootstrap_options(BootstrapInput::default())?;
    ensure_kimi_home(&bootstrap.home_dir)?;

    let oauth = OAuthToolkitService::new(&bootstrap.home_dir)?;
    let result = oauth
        .login(
            Some(PROVIDER),
            KimiOAuthLoginOptions {
                on_device_code: Some(&PrintDeviceCode),
                ..KimiOAuthLoginOptions::default()
            },
        )
        .await?;

    let has_credential = oauth
        .get_cached_access_token(Some(PROVIDER), None)
        .await?
        .is_some();

    println!("provider: {}", result.provider_name);
    println!("登录成功: {}", result.ok);
    println!("本地凭据可用: {has_credential}");

    Ok(())
}
