use rust_tiny_claw::integrations::feishu::client::FeishuClient;
use rust_tiny_claw::integrations::feishu::config::FeishuConfig;
use rust_tiny_claw::integrations::feishu::server::{FeishuServerState, serve};
use rust_tiny_claw::integrations::feishu::token::TenantTokenCache;
use std::env;
use std::sync::Arc;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let _ = dotenvy::dotenv();
    let _ = dotenvy::from_filename(".env.feishu");

    let config = FeishuConfig::from_env()?;
    if config.encrypt_key.is_some() {
        eprintln!(
            "FEISHU_ENCRYPT_KEY is configured, but encrypted callbacks are not implemented yet"
        );
    }

    let work_dir = env::current_dir()?;
    let token_cache = Arc::new(TenantTokenCache::new(
        config.app_id.clone(),
        config.app_secret.clone(),
    ));
    let client = FeishuClient::new(token_cache);
    let addr = config.callback_addr();
    let state = FeishuServerState::new(config, client, work_dir);

    println!("rust-tiny-claw Feishu callback server listening on http://{addr}/feishu/events");
    serve(state).await
}
