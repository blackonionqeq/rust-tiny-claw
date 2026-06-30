#![cfg(feature = "feishu")]

use rust_tiny_claw::integrations::feishu::approval::{
    ApprovalDecision, ApprovalManager, ResolveOutcome, callback_response,
};
use rust_tiny_claw::integrations::feishu::client::FeishuClient;
use rust_tiny_claw::integrations::feishu::config::FeishuConfig;
use rust_tiny_claw::integrations::feishu::event::{FeishuCallback, parse_callback};
use rust_tiny_claw::integrations::feishu::server::{FeishuServerState, router};
use rust_tiny_claw::integrations::feishu::token::TenantTokenCache;
use serde_json::json;
use std::net::{IpAddr, Ipv4Addr};
use std::path::PathBuf;
use std::sync::Arc;

#[test]
fn feishu_fixture_wires_router_event_parsing_and_approval_response()
-> Result<(), Box<dyn std::error::Error>> {
    let config = FeishuConfig {
        app_id: "app".to_string(),
        app_secret: "secret".to_string(),
        verify_token: Some("verify".to_string()),
        encrypt_key: None,
        callback_host: IpAddr::V4(Ipv4Addr::LOCALHOST),
        callback_port: 48080,
    };
    let token_cache = Arc::new(TenantTokenCache::new("app", "secret"));
    let client = FeishuClient::new(token_cache);
    let state = FeishuServerState::new(config, client, PathBuf::from("."));
    let _router = router(state);

    let callback = json!({
        "header": {
            "event_type": "card.action.trigger",
            "token": "verify"
        },
        "event": {
            "operator": {
                "operator_id": { "open_id": "ou_1" }
            },
            "action": {
                "value": {
                    "action": "reject_tool_call",
                    "approval_id": "approval_1"
                },
                "form_value": {
                    "reject_reason": "Use a dry-run first."
                }
            }
        }
    });

    let action = match parse_callback(&callback, Some("verify"))? {
        FeishuCallback::CardAction(action) => action,
        other => panic!("expected card action callback, got {other:?}"),
    };

    assert_eq!(action.approval_id, "approval_1");
    assert_eq!(action.decision, ApprovalDecision::Reject);
    assert_eq!(action.operator_id.as_deref(), Some("ou_1"));
    assert_eq!(
        action.reject_reason.as_deref(),
        Some("Use a dry-run first.")
    );

    let manager = ApprovalManager::new();
    let outcome = manager.resolve(&action.approval_id, action.decision, action.reject_reason)?;
    assert_eq!(outcome, ResolveOutcome::Unknown);

    let response = callback_response(outcome);
    assert_eq!(response["toast"]["type"], "warning");
    assert_eq!(
        response["toast"]["content"],
        "Approval request was not found or has expired."
    );

    Ok(())
}
