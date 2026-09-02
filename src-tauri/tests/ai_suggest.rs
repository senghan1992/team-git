//! Wiremock-driven tests for the AI suggester's transport path.
//!
//! `ai::suggest` reads from the global config store, so the test exercises
//! `ai::suggest_with` directly with an injected client + `AiConfig`.
use std::time::Duration;

use git_companion::ai::{suggest_with, ConflictContext};
use git_companion::config_store::AiConfig;
use serde_json::json;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn enabled_cfg(base_url: &str) -> AiConfig {
    AiConfig {
        enabled: true,
        base_url: base_url.into(),
        api_key: "sk-test-xxx".into(),
        model: "gpt-4o-mini".into(),
    }
}

fn ctx() -> ConflictContext {
    ConflictContext {
        file_path: "src/foo.ts".into(),
        base: Some("shared base\n".into()),
        ours: "ours change\n".into(),
        theirs: "theirs change\n".into(),
    }
}

#[tokio::test]
async fn suggest_with_posts_bearer_and_returns_message_content() {
    let server = MockServer::start().await;
    let cfg = enabled_cfg(&server.uri());
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();

    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .and(header("authorization", "Bearer sk-test-xxx"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "choices": [
                {"message": {"role": "assistant", "content": "merged output"}}
            ]
        })))
        .expect(1)
        .mount(&server)
        .await;

    let out = suggest_with(&client, &cfg, &ctx()).await.unwrap();
    assert_eq!(out, "merged output");
}

#[tokio::test]
async fn suggest_with_surfaces_api_error_as_internal() {
    let server = MockServer::start().await;
    let cfg = enabled_cfg(&server.uri());
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();

    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(401).set_body_string("unauthorized"))
        .expect(1)
        .mount(&server)
        .await;

    let err = suggest_with(&client, &cfg, &ctx()).await.unwrap_err();
    let msg = format!("{err:?}");
    assert!(msg.contains("401"), "expected 401 in error, got: {msg}");
}

#[tokio::test]
async fn suggest_with_rejects_empty_config() {
    let client = reqwest::Client::new();
    let cfg = AiConfig {
        enabled: true,
        base_url: "".into(),
        api_key: "k".into(),
        model: "m".into(),
    };
    let err = suggest_with(&client, &cfg, &ctx()).await.unwrap_err();
    let msg = format!("{err:?}");
    assert!(msg.contains("Base URL") || msg.contains("비어"));
}
