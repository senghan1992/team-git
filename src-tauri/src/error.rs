use serde::Serialize;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum AppError {
    #[error("git error: {0}")]
    Git(String),
    #[error("io error: {0}")]
    Io(String),
    #[error("config error: {0}")]
    Config(String),
    #[error("repo not found: {0}")]
    RepoNotFound(String),
    #[error("channel not found: {0}")]
    ChannelNotFound(String),
    #[error("webhook error ({status}): {body}")]
    Webhook { status: u16, body: String },
    #[error("webhook url invalid: {0}")]
    WebhookUrlInvalid(String),
    #[error("ssh auth error: {0}")]
    SshAuth(String),
    #[error("db error: {0}")]
    Db(String),
    #[error("hook error: {0}")]
    Hook(String),
    #[error("internal: {0}")]
    Internal(String),
}

impl From<std::io::Error> for AppError {
    fn from(value: std::io::Error) -> Self {
        AppError::Io(value.to_string())
    }
}

impl From<rusqlite::Error> for AppError {
    fn from(value: rusqlite::Error) -> Self {
        AppError::Db(value.to_string())
    }
}

impl From<serde_json::Error> for AppError {
    fn from(value: serde_json::Error) -> Self {
        AppError::Config(value.to_string())
    }
}

impl Serialize for AppError {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        #[derive(Serialize)]
        struct Body<'a> {
            kind: &'a str,
            message: String,
        }
        let kind = match self {
            AppError::Git(_) => "git",
            AppError::Io(_) => "io",
            AppError::Config(_) => "config",
            AppError::RepoNotFound(_) => "repo_not_found",
            AppError::ChannelNotFound(_) => "channel_not_found",
            AppError::Webhook { .. } => "webhook",
            AppError::WebhookUrlInvalid(_) => "webhook_url_invalid",
            AppError::SshAuth(_) => "ssh_auth",
            AppError::Db(_) => "db",
            AppError::Hook(_) => "hook",
            AppError::Internal(_) => "internal",
        };
        Body {
            kind,
            message: self.to_string(),
        }
        .serialize(serializer)
    }
}

pub type AppResult<T> = Result<T, AppError>;
