use crate::app::build_engine;
use crate::engine::RunOptions;
use crate::integrations::feishu::client::FeishuClient;
use crate::integrations::feishu::config::FeishuConfig;
use crate::integrations::feishu::event::{FeishuCallback, parse_callback};
use crate::integrations::feishu::reporter::FeishuReporter;
use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::post;
use axum::{Json, Router};
use serde::Serialize;
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Clone)]
pub struct FeishuServerState {
    config: FeishuConfig,
    client: FeishuClient,
    work_dir: PathBuf,
}

impl FeishuServerState {
    pub fn new(config: FeishuConfig, client: FeishuClient, work_dir: PathBuf) -> Self {
        Self {
            config,
            client,
            work_dir,
        }
    }
}

pub fn router(state: FeishuServerState) -> Router {
    Router::new()
        .route("/feishu/events", post(handle_event))
        .with_state(Arc::new(state))
}

pub async fn serve(state: FeishuServerState) -> Result<(), Box<dyn std::error::Error>> {
    let addr = state.config.callback_addr();
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, router(state)).await?;
    Ok(())
}

async fn handle_event(
    State(state): State<Arc<FeishuServerState>>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, (StatusCode, Json<ErrorResponse>)> {
    match parse_callback(&body, state.config.verify_token.as_deref()) {
        Ok(FeishuCallback::Challenge { challenge }) => Ok(Json(json!({ "challenge": challenge }))),
        Ok(FeishuCallback::Ignored) => Ok(Json(json!({ "ok": true }))),
        Ok(FeishuCallback::Message(message)) => {
            let client = state.client.clone();
            let work_dir = state.work_dir.clone();
            tokio::task::spawn_blocking(move || {
                let run_result = (|| -> Result<(), Box<dyn std::error::Error>> {
                    let mut engine = build_engine(&work_dir)?;
                    let mut reporter = FeishuReporter::new(client, message.chat_id);
                    let options = RunOptions {
                        max_turns: 12,
                        enable_thinking: false,
                        stream: false,
                    };
                    engine.run_with_reporter(message.text, options, &mut reporter)?;
                    Ok(())
                })();

                if let Err(error) = run_result {
                    eprintln!("Feishu message run failed: {error}");
                }
            });

            Ok(Json(json!({ "ok": true })))
        }
        Err(error) => Err((
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: error.to_string(),
            }),
        )),
    }
}

#[derive(Debug, Serialize)]
struct ErrorResponse {
    error: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr};

    #[test]
    fn builds_callback_router() {
        let config = FeishuConfig {
            app_id: "app".to_string(),
            app_secret: "secret".to_string(),
            verify_token: Some("verify".to_string()),
            encrypt_key: None,
            callback_host: IpAddr::V4(Ipv4Addr::LOCALHOST),
            callback_port: 48080,
        };
        let token_cache = Arc::new(crate::integrations::feishu::token::TenantTokenCache::new(
            "app", "secret",
        ));
        let client = FeishuClient::new(token_cache);
        let state = FeishuServerState::new(config, client, PathBuf::from("."));

        let _router = router(state);
    }
}
