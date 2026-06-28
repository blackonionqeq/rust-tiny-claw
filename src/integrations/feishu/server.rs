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
use std::panic::AssertUnwindSafe;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;
use tower_http::trace::{DefaultMakeSpan, DefaultOnRequest, DefaultOnResponse, TraceLayer};
use tracing::{Level, error, info, warn};

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
        .layer(
            TraceLayer::new_for_http()
                .make_span_with(DefaultMakeSpan::new().level(Level::INFO))
                .on_request(DefaultOnRequest::new().level(Level::INFO))
                .on_response(DefaultOnResponse::new().level(Level::INFO)),
        )
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
        Ok(FeishuCallback::Challenge { challenge }) => {
            info!("Feishu URL verification challenge accepted");
            Ok(Json(json!({ "challenge": challenge })))
        }
        Ok(FeishuCallback::Ignored) => {
            let event_type = body
                .pointer("/header/event_type")
                .and_then(Value::as_str)
                .unwrap_or("<missing>");
            info!(%event_type, "Feishu event ignored");
            Ok(Json(json!({ "ok": true })))
        }
        Ok(FeishuCallback::Message(message)) => {
            let message_id = message.message_id.clone();
            let chat_id = message.chat_id.clone();
            let sender_id = message.sender_id.clone();
            let text_len = message.text.chars().count();
            info!(
                %message_id,
                %chat_id,
                %sender_id,
                text_len,
                "Feishu message received"
            );

            let client = state.client.clone();
            let work_dir = state.work_dir.clone();
            tokio::task::spawn_blocking(move || {
                let started = Instant::now();
                info!(%message_id, %chat_id, "Feishu agent run started");

                let run_result = std::panic::catch_unwind(AssertUnwindSafe(
                    || -> Result<(), Box<dyn std::error::Error>> {
                        let mut engine = build_engine(&work_dir)?;
                        let mut reporter = FeishuReporter::new(client, message.chat_id);
                        let options = RunOptions {
                            max_turns: 12,
                            enable_thinking: false,
                            stream: false,
                        };
                        engine.run_with_reporter(message.text, options, &mut reporter)?;
                        Ok(())
                    },
                ));

                let elapsed_ms = started.elapsed().as_millis();
                match run_result {
                    Ok(Ok(())) => {
                        info!(%message_id, %chat_id, elapsed_ms, "Feishu agent run succeeded");
                    }
                    Ok(Err(error)) => {
                        error!(
                            %message_id,
                            %chat_id,
                            elapsed_ms,
                            error = %error,
                            "Feishu agent run failed"
                        );
                    }
                    Err(_) => {
                        error!(
                            %message_id,
                            %chat_id,
                            elapsed_ms,
                            "Feishu agent run panicked"
                        );
                    }
                }
            });

            Ok(Json(json!({ "ok": true })))
        }
        Err(error) => {
            warn!(error = %error, "Feishu callback rejected");
            Err((
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse {
                    error: error.to_string(),
                }),
            ))
        }
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
