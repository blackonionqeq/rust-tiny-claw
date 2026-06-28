use rust_tiny_claw::integrations::feishu::client::FeishuClient;
use rust_tiny_claw::integrations::feishu::config::FeishuConfig;
use rust_tiny_claw::integrations::feishu::server::{FeishuServerState, serve};
use rust_tiny_claw::integrations::feishu::token::TenantTokenCache;
use std::env;
use std::sync::Arc;
use tracing::{info, warn};
use tracing_subscriber::{EnvFilter, layer::SubscriberExt, util::SubscriberInitExt};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let _ = dotenvy::dotenv();
    let _ = dotenvy::from_filename(".env.feishu");
    init_logging();

    let config = FeishuConfig::from_env()?;
    if config.encrypt_key.is_some() {
        warn!("FEISHU_ENCRYPT_KEY is configured, but encrypted callbacks are not implemented yet");
    }

    let work_dir = env::current_dir()?;
    let provider = env::var("TINY_CLAW_PROVIDER").unwrap_or_else(|_| "mock".to_string());
    info!(
        %provider,
        work_dir = %work_dir.display(),
        verify_token_configured = config.verify_token.is_some(),
        encrypt_key_configured = config.encrypt_key.is_some(),
        "Feishu gateway configuration loaded"
    );

    let token_cache = Arc::new(TenantTokenCache::new(
        config.app_id.clone(),
        config.app_secret.clone(),
    ));
    let client = FeishuClient::new(token_cache);
    let addr = config.callback_addr();
    let state = FeishuServerState::new(config, client, work_dir);

    info!(
        %addr,
        "rust-tiny-claw Feishu callback server listening on /feishu/events"
    );
    serve(state).await
}

fn init_logging() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        EnvFilter::new("tiny_claw_feishu=info,rust_tiny_claw=info,tower_http=info")
    });

    tracing_subscriber::registry()
        .with(filter)
        .with(tracing_subscriber::fmt::layer())
        .init();
}
