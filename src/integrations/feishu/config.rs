use std::env;
use std::net::{IpAddr, SocketAddr};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeishuConfig {
    pub app_id: String,
    pub app_secret: String,
    pub verify_token: Option<String>,
    pub encrypt_key: Option<String>,
    pub callback_host: IpAddr,
    pub callback_port: u16,
}

impl FeishuConfig {
    pub fn from_env() -> Result<Self, ConfigError> {
        Ok(Self {
            app_id: required_env("FEISHU_APP_ID")?,
            app_secret: required_env("FEISHU_APP_SECRET")?,
            verify_token: optional_env("FEISHU_VERIFY_TOKEN"),
            encrypt_key: optional_env("FEISHU_ENCRYPT_KEY"),
            callback_host: env::var("FEISHU_CALLBACK_HOST")
                .unwrap_or_else(|_| "0.0.0.0".to_string())
                .parse()
                .map_err(|error| {
                    ConfigError::new(format!("invalid FEISHU_CALLBACK_HOST: {error}"))
                })?,
            callback_port: match env::var("FEISHU_CALLBACK_PORT") {
                Ok(value) => value.parse().map_err(|error| {
                    ConfigError::new(format!("invalid FEISHU_CALLBACK_PORT: {error}"))
                })?,
                Err(_) => 48080,
            },
        })
    }

    pub fn callback_addr(&self) -> SocketAddr {
        SocketAddr::new(self.callback_host, self.callback_port)
    }
}

#[derive(Debug)]
pub struct ConfigError {
    message: String,
}

impl ConfigError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}", self.message)
    }
}

impl std::error::Error for ConfigError {}

fn required_env(name: &str) -> Result<String, ConfigError> {
    match env::var(name) {
        Ok(value) if !value.trim().is_empty() => Ok(value),
        _ => Err(ConfigError::new(format!(
            "missing required environment variable: {name}"
        ))),
    }
}

fn optional_env(name: &str) -> Option<String> {
    env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}
