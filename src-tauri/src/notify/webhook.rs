//! Build provider-specific JSON payloads and POST them.
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::config_store::{ChannelKind, WebhookChannel};
use crate::error::{AppError, AppResult};
use crate::notify::{NotifyEvent, Notifier};

pub struct WebhookNotifier {
    pub client: reqwest::Client,
}

impl Default for WebhookNotifier {
    fn default() -> Self {
        Self {
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(15))
                .user_agent("git-companion/0.1")
                .build()
                .expect("reqwest client"),
        }
    }
}

#[async_trait]
impl Notifier for WebhookNotifier {
    async fn dispatch(
        &self,
        channel: &WebhookChannel,
        event: &NotifyEvent,
    ) -> AppResult<u16> {
        let payload = build_payload(channel, event);
        let resp = self
            .client
            .post(&channel.url)
            .json(&payload)
            .send()
            .await
            .map_err(|e| AppError::Webhook {
                status: 0,
                body: e.to_string(),
            })?;
        let status = resp.status().as_u16();
        if !resp.status().is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(AppError::Webhook {
                status,
                body: body.chars().take(4000).collect(),
            });
        }
        Ok(status)
    }
}

/// Pure function — exercised by `tests/notify_payload.rs` as a snapshot test.
pub fn build_payload(channel: &WebhookChannel, event: &NotifyEvent) -> Value {
    match channel.kind {
        ChannelKind::Slack => slack_payload(channel, event),
        ChannelKind::Discord => discord_payload(channel, event),
        ChannelKind::Teams => teams_payload(channel, event),
        ChannelKind::N8n => n8n_payload(channel, event),
        ChannelKind::Custom => custom_payload(channel, event),
    }
}

fn slack_payload(_ch: &WebhookChannel, event: &NotifyEvent) -> Value {
    let (title, body) = describe(event);
    json!({
        "text": format!("*{}* — {}", title, body.summary()),
        "blocks": [
            {
                "type": "section",
                "text": {
                    "type": "mrkdwn",
                    "text": format!("*Git Companion — {}*\n{}", title, body.summary())
                }
            }
        ]
    })
}

fn discord_payload(_ch: &WebhookChannel, event: &NotifyEvent) -> Value {
    let (title, body) = describe(event);
    json!({
        "content": format!("**{}** — {}", title, body.summary()),
        "embeds": [{
            "title": title,
            "description": body.summary(),
            "color": 0xff385c
        }]
    })
}

fn teams_payload(_ch: &WebhookChannel, event: &NotifyEvent) -> Value {
    let (title, body) = describe(event);
    json!({
        "@type": "MessageCard",
        "@context": "https://schema.org/extensions",
        "summary": title,
        "themeColor": "FF385C",
        "title": title,
        "text": body.summary(),
        "sections": [{
            "activityTitle": title,
            "text": body.summary()
        }]
    })
}

fn n8n_payload(_ch: &WebhookChannel, event: &NotifyEvent) -> Value {
    json!({
        "event": event_kind(event),
        "data": event
    })
}

fn custom_payload(_ch: &WebhookChannel, event: &NotifyEvent) -> Value {
    // v1 does not support custom templates — fall back to a minimal pass-through.
    json!({ "event": event_kind(event), "data": event })
}

struct EventBody<'a> {
    event: &'a NotifyEvent,
}

impl<'a> EventBody<'a> {
    fn summary(&self) -> String {
        match self.event {
            NotifyEvent::MainPush { author, message, sha, repo_name, branch, url } => format!(
                "{author} pushed to `{branch}` in **{repo_name}**\n{message}\nsha: `{sha}`\nrepo: {url}"
            ),
            NotifyEvent::BranchPush { author, message, sha, repo_name, branch, url } => format!(
                "{author} pushed `{branch}` in **{repo_name}**\n{message}\nsha: `{sha}`\nrepo: {url}"
            ),
            NotifyEvent::Release { author, repo_name, url, version } => format!(
                "{author} released **{repo_name} v{version}**\n{url}"
            ),
        }
    }
}

fn describe(event: &NotifyEvent) -> (String, EventBody<'_>) {
    let title = match event {
        NotifyEvent::MainPush { .. } => "Push to main".to_string(),
        NotifyEvent::BranchPush { .. } => "Branch push".to_string(),
        NotifyEvent::Release { .. } => "Release".to_string(),
    };
    (title, EventBody { event })
}

pub fn event_kind(event: &NotifyEvent) -> &'static str {
    match event {
        NotifyEvent::MainPush { .. } => "main_push",
        NotifyEvent::BranchPush { .. } => "branch_push",
        NotifyEvent::Release { .. } => "release",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config_store::ChannelKind;

    fn ch(kind: ChannelKind) -> WebhookChannel {
        WebhookChannel {
            id: uuid::Uuid::new_v4(),
            kind,
            name: "test".into(),
            url: "https://example.invalid/webhook".into(),
            default_for_main_push: true,
            default_for_branch_push: false,
            default_for_release: false,
            created_at: chrono::Utc::now(),
        }
    }

    #[test]
    fn slack_payload_shape() {
        let c = ch(ChannelKind::Slack);
        let e = NotifyEvent::BranchPush {
            author: "alice".into(),
            message: "feat: hello".into(),
            sha: "deadbeef".into(),
            repo_name: "demo".into(),
            url: "https://example.com/demo".into(),
            branch: "feat".into(),
        };
        let p = build_payload(&c, &e);
        assert!(p["text"].as_str().unwrap().contains("Branch push"));
        assert!(p["blocks"].is_array());
    }

    #[test]
    fn discord_payload_has_embed() {
        let c = ch(ChannelKind::Discord);
        let e = NotifyEvent::MainPush {
            author: "bob".into(),
            message: "fix: bug".into(),
            sha: "abcd1234".into(),
            repo_name: "demo".into(),
            url: "https://example.com/demo".into(),
            branch: "main".into(),
        };
        let p = build_payload(&c, &e);
        assert_eq!(p["embeds"][0]["color"], 0xff385c);
    }

    #[test]
    fn teams_payload_is_message_card() {
        let c = ch(ChannelKind::Teams);
        let e = NotifyEvent::Release {
            author: "carol".into(),
            repo_name: "demo".into(),
            url: "https://example.com/demo".into(),
            version: "1.2.3".into(),
        };
        let p = build_payload(&c, &e);
        assert_eq!(p["@type"], "MessageCard");
    }

    #[test]
    fn n8n_payload_passes_event_data() {
        let c = ch(ChannelKind::N8n);
        let e = NotifyEvent::BranchPush {
            author: "dave".into(),
            message: "x".into(),
            sha: "f".into(),
            repo_name: "demo".into(),
            url: "https://example.com/demo".into(),
            branch: "feat".into(),
        };
        let p = build_payload(&c, &e);
        assert_eq!(p["event"], "branch_push");
        assert_eq!(p["data"]["author"], "dave");
    }
}
