use crate::app::build_feishu_engine;
use crate::context_engine::ContextBudget;
use crate::engine::RunOptions;
use crate::integrations::feishu::approval::{ApprovalManager, callback_response};
use crate::integrations::feishu::client::FeishuClient;
use crate::integrations::feishu::config::FeishuConfig;
use crate::integrations::feishu::event::{FeishuCallback, parse_callback};
use crate::integrations::feishu::reporter::FeishuReporter;
use crate::memory::SessionManager;
use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::post;
use axum::{Json, Router};
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::fs;
use std::io;
use std::panic::AssertUnwindSafe;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tower_http::trace::{DefaultMakeSpan, DefaultOnRequest, DefaultOnResponse, TraceLayer};
use tracing::{Level, error, info, warn};

const MESSAGE_DEDUP_TTL: Duration = Duration::from_secs(10 * 60);
const UNSUPPORTED_MESSAGE_REPLY: &str =
    "当前只支持文本消息。请直接发送文字指令，我会按文字内容执行。";

#[derive(Clone)]
pub struct FeishuServerState {
    config: FeishuConfig,
    client: FeishuClient,
    work_dir: PathBuf,
    deduper: Arc<MessageDeduper>,
    sessions: Arc<SessionManager>,
    approvals: Arc<ApprovalManager>,
}

impl FeishuServerState {
    pub fn new(config: FeishuConfig, client: FeishuClient, work_dir: PathBuf) -> Self {
        Self {
            config,
            client,
            work_dir,
            deduper: Arc::new(MessageDeduper::new(MESSAGE_DEDUP_TTL)),
            sessions: Arc::new(SessionManager::new()),
            approvals: Arc::new(ApprovalManager::new()),
        }
    }

    fn already_seen_message(&self, message_id: &str) -> bool {
        self.deduper.already_seen_or_insert(message_id)
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
        Ok(FeishuCallback::CardAction(action)) => {
            let approval_id = action.approval_id.clone();
            let decision = action.decision;
            let operator_id = action
                .operator_id
                .clone()
                .unwrap_or_else(|| "<unknown>".to_string());
            info!(
                %approval_id,
                %operator_id,
                ?decision,
                "Feishu approval card action received"
            );

            let outcome = state
                .approvals
                .resolve(&approval_id, decision, action.reject_reason)
                .map_err(|error| {
                    warn!(%approval_id, error = %error, "Feishu approval resolution failed");
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(ErrorResponse {
                            error: error.to_string(),
                        }),
                    )
                })?;

            Ok(Json(callback_response(outcome)))
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

            if state.already_seen_message(&message_id) {
                info!(%message_id, %chat_id, "duplicate Feishu message ignored");
                return Ok(Json(json!({ "ok": true, "duplicate": true })));
            }

            let client = state.client.clone();
            let workspace_root = state.work_dir.clone();
            let sessions = Arc::clone(&state.sessions);
            let approvals = Arc::clone(&state.approvals);
            tokio::task::spawn_blocking(move || {
                let started = Instant::now();
                info!(%message_id, %chat_id, "Feishu agent run started");

                let run_result = std::panic::catch_unwind(AssertUnwindSafe(
                    || -> Result<(), Box<dyn std::error::Error>> {
                        let work_dir = chat_work_dir(&workspace_root, &chat_id)?;
                        info!(
                            %message_id,
                            %chat_id,
                            work_dir = %work_dir.display(),
                            "Feishu chat workspace resolved"
                        );
                        let mut engine = build_feishu_engine(
                            &work_dir,
                            client.clone(),
                            chat_id.clone(),
                            approvals,
                        )?;
                        let mut reporter = FeishuReporter::new(client, message.chat_id);
                        let session =
                            sessions.get_or_create(format!("feishu:{chat_id}"), work_dir.clone());
                        let options = RunOptions {
                            max_turns: 12,
                            enable_thinking: false,
                            plan_mode: false,
                            stream: false,
                            context_budget: ContextBudget::default(),
                        };
                        engine.run_session_with_reporter(
                            &session,
                            message.text,
                            options,
                            &mut reporter,
                        )?;
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
        Ok(FeishuCallback::UnsupportedMessage(message)) => {
            let message_id = message.message_id.clone();
            let chat_id = message.chat_id.clone();
            let sender_id = message.sender_id.clone();
            let message_type = message.message_type.clone();
            info!(
                %message_id,
                %chat_id,
                %sender_id,
                %message_type,
                "unsupported Feishu message received"
            );

            if state.already_seen_message(&message_id) {
                info!(%message_id, %chat_id, "duplicate unsupported Feishu message ignored");
                return Ok(Json(json!({ "ok": true, "duplicate": true })));
            }

            let client = state.client.clone();
            tokio::task::spawn_blocking(move || {
                if let Err(error) = client.send_text_to_chat(&chat_id, UNSUPPORTED_MESSAGE_REPLY) {
                    error!(
                        %message_id,
                        %chat_id,
                        error = %error,
                        "failed to send unsupported Feishu message reply"
                    );
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

fn chat_work_dir(workspace_root: &Path, chat_id: &str) -> io::Result<PathBuf> {
    let work_dir = workspace_root
        .join("feishu")
        .join(chat_id_path_segment(chat_id));
    fs::create_dir_all(&work_dir)?;
    work_dir.canonicalize()
}

fn chat_id_path_segment(chat_id: &str) -> String {
    if chat_id.is_empty() {
        return "chat-empty".to_string();
    }

    let mut segment = String::from("chat-");
    for byte in chat_id.bytes() {
        match byte {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_' => {
                segment.push(byte as char);
            }
            _ => {
                segment.push('_');
                segment.push_str(&format!("{byte:02x}"));
            }
        }
    }
    segment
}

#[derive(Debug, Serialize)]
struct ErrorResponse {
    error: String,
}

#[derive(Debug)]
struct MessageDeduper {
    ttl: Duration,
    seen_at: Mutex<HashMap<String, Instant>>,
}

impl MessageDeduper {
    fn new(ttl: Duration) -> Self {
        Self {
            ttl,
            seen_at: Mutex::new(HashMap::new()),
        }
    }

    fn already_seen_or_insert(&self, message_id: &str) -> bool {
        let now = Instant::now();
        let mut seen_at = self
            .seen_at
            .lock()
            .expect("Feishu message dedup cache poisoned");

        seen_at.retain(|_, first_seen_at| now.duration_since(*first_seen_at) < self.ttl);

        if seen_at.contains_key(message_id) {
            true
        } else {
            seen_at.insert(message_id.to_string(), now);
            false
        }
    }
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

    #[test]
    fn deduper_detects_duplicate_message_ids() {
        let deduper = MessageDeduper::new(Duration::from_secs(60));

        assert!(!deduper.already_seen_or_insert("om_1"));
        assert!(deduper.already_seen_or_insert("om_1"));
        assert!(!deduper.already_seen_or_insert("om_2"));
    }

    #[test]
    fn chat_work_dir_is_nested_under_workspace_root() {
        let workspace_root = tempfile::tempdir().unwrap();

        let work_dir = chat_work_dir(workspace_root.path(), "oc_abc123").unwrap();

        assert_eq!(
            work_dir,
            workspace_root
                .path()
                .join("feishu")
                .join("chat-oc_abc123")
                .canonicalize()
                .unwrap()
        );
        assert!(work_dir.is_dir());
    }

    #[test]
    fn chat_id_path_segment_escapes_path_separators() {
        assert_eq!(chat_id_path_segment("../oc/abc"), "chat-_2e_2e_2foc_2fabc");
    }
}
