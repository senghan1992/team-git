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
        ..AiConfig::default()
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
        ..AiConfig::default()
    };
    let err = suggest_with(&client, &cfg, &ctx()).await.unwrap_err();
    let msg = format!("{err:?}");
    assert!(msg.contains("Base URL") || msg.contains("비어"));
}

// ── Pre-configured resolver prompt (scenario 5) ─────────────────────────────
//
// The merge manager writes the prompt once in Settings; every conflicted file
// must then be resolved with *that* text, not the built-in default.

#[test]
fn effective_prompt_falls_back_to_default_when_unset() {
    let cfg = AiConfig::default();
    assert_eq!(
        git_companion::ai::effective_system_prompt(&cfg),
        git_companion::ai::DEFAULT_SYSTEM_PROMPT
    );
    // Whitespace-only is treated as unset, so a cleared textarea still works.
    let blank = AiConfig {
        system_prompt: "   \n  ".into(),
        ..AiConfig::default()
    };
    assert_eq!(
        git_companion::ai::effective_system_prompt(&blank),
        git_companion::ai::DEFAULT_SYSTEM_PROMPT
    );
}

#[tokio::test]
async fn suggest_with_sends_the_preconfigured_prompt() {
    let server = MockServer::start().await;
    let cfg = AiConfig {
        system_prompt: "  우리 팀 규칙: 항상 theirs 쪽 API 시그니처를 유지한다.  ".into(),
        ..enabled_cfg(&server.uri())
    };
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();

    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .and(wiremock::matchers::body_string_contains(
            "우리 팀 규칙: 항상 theirs 쪽 API 시그니처를 유지한다.",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "choices": [{"message": {"role": "assistant", "content": "ok"}}]
        })))
        .expect(1)
        .mount(&server)
        .await;

    let out = suggest_with(&client, &cfg, &ctx()).await.unwrap();
    assert_eq!(out, "ok");
}
