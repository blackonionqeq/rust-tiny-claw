use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tracing::{debug, warn};

const TENANT_TOKEN_URL: &str =
    "https://open.feishu.cn/open-apis/auth/v3/tenant_access_token/internal";
const EXPIRY_SAFETY_MARGIN: Duration = Duration::from_secs(60);

#[derive(Debug)]
pub struct TenantTokenCache {
    client: Client,
    app_id: String,
    app_secret: String,
    cached: Mutex<Option<CachedToken>>,
}

impl TenantTokenCache {
    pub fn new(app_id: impl Into<String>, app_secret: impl Into<String>) -> Self {
        Self {
            client: Client::new(),
            app_id: app_id.into(),
            app_secret: app_secret.into(),
            cached: Mutex::new(None),
        }
    }

    pub fn tenant_access_token(&self) -> Result<String, TokenError> {
        {
            let cached = self
                .cached
                .lock()
                .map_err(|_| TokenError::new("token cache poisoned"))?;
            if let Some(cached) = cached.as_ref()
                && cached.expires_at > Instant::now()
            {
                debug!("Feishu tenant access token cache hit");
                return Ok(cached.token.clone());
            }
        }

        debug!("Feishu tenant access token cache miss");
        let token = self.fetch_tenant_access_token()?;
        let mut cached = self
            .cached
            .lock()
            .map_err(|_| TokenError::new("token cache poisoned"))?;
        *cached = Some(token.clone());
        Ok(token.token)
    }

    fn fetch_tenant_access_token(&self) -> Result<CachedToken, TokenError> {
        let response = self
            .client
            .post(TENANT_TOKEN_URL)
            .json(&TenantTokenRequest {
                app_id: &self.app_id,
                app_secret: &self.app_secret,
            })
            .send()
            .map_err(|error| TokenError::new(format!("Feishu token request failed: {error}")))?;

        let status = response.status();
        let body = response.text().map_err(|error| {
            TokenError::new(format!("failed to read Feishu token response: {error}"))
        })?;

        if !status.is_success() {
            warn!(%status, "Feishu token endpoint returned HTTP error");
            return Err(TokenError::new(format!(
                "Feishu token endpoint returned HTTP {status}: {body}"
            )));
        }

        let response: TenantTokenResponse = serde_json::from_str(&body).map_err(|error| {
            TokenError::new(format!(
                "invalid Feishu token response: {error}; raw response: {body}"
            ))
        })?;

        if response.code != 0 {
            warn!(
                code = response.code,
                msg = %response.msg,
                "Feishu token endpoint returned API error"
            );
            return Err(TokenError::new(format!(
                "Feishu token endpoint returned code {}: {}",
                response.code, response.msg
            )));
        }

        let ttl = Duration::from_secs(response.expire.max(1));
        let expires_at = Instant::now() + ttl.saturating_sub(EXPIRY_SAFETY_MARGIN);

        debug!(
            ttl_secs = response.expire,
            "Feishu tenant access token refreshed"
        );
        Ok(CachedToken {
            token: response.tenant_access_token,
            expires_at,
        })
    }
}

#[derive(Debug, Clone)]
struct CachedToken {
    token: String,
    expires_at: Instant,
}

#[derive(Debug, Serialize)]
struct TenantTokenRequest<'a> {
    app_id: &'a str,
    app_secret: &'a str,
}

#[derive(Debug, Deserialize)]
struct TenantTokenResponse {
    code: i64,
    msg: String,
    tenant_access_token: String,
    expire: u64,
}

#[derive(Debug)]
pub struct TokenError {
    message: String,
}

impl TokenError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl std::fmt::Display for TokenError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}", self.message)
    }
}

impl std::error::Error for TokenError {}
