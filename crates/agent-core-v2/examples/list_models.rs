use kimi_code_agent_core_v2::app::{
    auth::{OAuthToolkitContract, OAuthToolkitService},
    bootstrap::{BootstrapInput, resolve_bootstrap_options},
};
use kimi_code_oauth::{CredentialKind, fetch_managed_kimi_code_models};

const PROVIDER: &str = "kimi-code";

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let bootstrap = resolve_bootstrap_options(BootstrapInput::default())?;
    let oauth = OAuthToolkitService::new(&bootstrap.home_dir)?;

    let access_token = oauth
        .get_cached_access_token(Some(PROVIDER), None)
        .await?
        .ok_or("尚未登录，请先运行 login 示例")?;
    println!("access_token={access_token}");
    let models =
        fetch_managed_kimi_code_models(&access_token, None, None, CredentialKind::OAuth).await?;

    for model in models {
        println!(
            "{} - {}",
            model.id,
            model.display_name.as_deref().unwrap_or("未命名模型")
        );
    }

    Ok(())
}
