use rust_tiny_claw::integrations::feishu::client::FeishuClient;
use rust_tiny_claw::integrations::feishu::config::FeishuConfig;
use rust_tiny_claw::integrations::feishu::server::{FeishuServerState, serve};
use rust_tiny_claw::integrations::feishu::token::TenantTokenCache;
use std::env;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tracing::{info, warn};
use tracing_subscriber::{EnvFilter, layer::SubscriberExt, util::SubscriberInitExt};

const FEISHU_WORKSPACE_ENV: &str = "TINY_CLAW_WORKSPACE";
const DEFAULT_FEISHU_WORKSPACE: &str = ".feishu-workspace";

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let _ = dotenvy::dotenv();
    let _ = dotenvy::from_filename(".env.feishu");
    init_logging();

    let config = FeishuConfig::from_env()?;
    if config.encrypt_key.is_some() {
        warn!("FEISHU_ENCRYPT_KEY is configured, but encrypted callbacks are not implemented yet");
    }

    let work_dir = resolve_feishu_work_dir()?;
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

fn resolve_feishu_work_dir() -> Result<PathBuf, Box<dyn std::error::Error>> {
    let current_dir = env::current_dir()?;
    Ok(resolve_feishu_work_dir_from(
        env::var_os(FEISHU_WORKSPACE_ENV),
        &current_dir,
    )?)
}

fn resolve_feishu_work_dir_from(
    configured: Option<OsString>,
    current_dir: &Path,
) -> std::io::Result<PathBuf> {
    let configured = configured
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty());
    let work_dir = configured.unwrap_or_else(|| current_dir.join(DEFAULT_FEISHU_WORKSPACE));
    let work_dir = if work_dir.is_absolute() {
        work_dir
    } else {
        current_dir.join(work_dir)
    };

    fs::create_dir_all(&work_dir)?;
    work_dir.canonicalize()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_feishu_workspace_is_hidden_directory_under_current_dir() {
        let current_dir = tempfile::tempdir().unwrap();

        let work_dir = resolve_feishu_work_dir_from(None, current_dir.path()).unwrap();

        assert_eq!(
            work_dir,
            current_dir
                .path()
                .join(DEFAULT_FEISHU_WORKSPACE)
                .canonicalize()
                .unwrap()
        );
        assert!(work_dir.is_dir());
    }

    #[test]
    fn env_workspace_overrides_default_and_can_be_relative() {
        let current_dir = tempfile::tempdir().unwrap();

        let work_dir = resolve_feishu_work_dir_from(
            Some(OsString::from("project-workspace")),
            current_dir.path(),
        )
        .unwrap();

        assert_eq!(
            work_dir,
            current_dir
                .path()
                .join("project-workspace")
                .canonicalize()
                .unwrap()
        );
        assert!(work_dir.is_dir());
    }

    #[test]
    fn empty_env_workspace_falls_back_to_default() {
        let current_dir = tempfile::tempdir().unwrap();

        let work_dir =
            resolve_feishu_work_dir_from(Some(OsString::from("")), current_dir.path()).unwrap();

        assert_eq!(
            work_dir,
            current_dir
                .path()
                .join(DEFAULT_FEISHU_WORKSPACE)
                .canonicalize()
                .unwrap()
        );
        assert!(work_dir.is_dir());
    }
}
