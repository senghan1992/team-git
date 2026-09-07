//! Optional AI conflict resolver — OpenAI-compatible `/chat/completions`.
//!
//! Disabled by default. When enabled, the merge-center conflict panel can call
//! `commands::ai::ai_suggest_resolution` to ask for a suggested merged file.
//! Credentials live in `AiConfig` and never leave the device.

use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::config_store::AiConfig;
use crate::error::{AppError, AppResult};

#[derive(Debug, Clone)]
pub struct ConflictContext {
    pub file_path: String,
    pub base: Option<String>,
    pub ours: String,
    pub theirs: String,
}

/// git 병합 충돌 해결용 **기본** 시스템 프롬프트: 양쪽 브랜치에서 수정한 기능이
/// 서로 영향받지 않도록 모두 반영하는 최종 코드를 요청한다.
///
/// 병합 관리자는 설정에서 이 문구를 프로젝트에 맞게 미리 바꿔 둘 수 있다
/// (`AiConfig::system_prompt`). 비어 있으면 이 기본값을 쓴다.
pub const DEFAULT_SYSTEM_PROMPT: &str = "git 병합에 실패한 상태입니다. ours(현재 브랜치)와 theirs(병합 대상 브랜치) 양쪽에서 수정한 기능들이 서로 영향받지 않도록 모두 반영하는 최종 코드를 제안하세요. 기능이 깨지지 않게 import/선언 누락, 중복 정의, 끊긴 호출부가 없어야 합니다. 판단 근거 주석 없이 코드만 반환하세요. 직접적인 결합이 불가능하면 양쪽 의도를 모두 만족하는 대안 코드를 제시하세요.";

#[derive(Debug, Serialize)]
struct ChatRequest<'a> {
    model: &'a str,
    messages: Vec<ChatMessage<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
}

#[derive(Debug, Serialize)]
struct ChatMessage<'a> {
    role: &'a str,
    content: String,
}

#[derive(Debug, Deserialize)]
struct ChatResponse {
    choices: Vec<Choice>,
}

#[derive(Debug, Deserialize)]
struct Choice {
    message: ChoiceMessage,
}

#[derive(Debug, Deserialize)]
struct ChoiceMessage {
    content: String,
}

/// Load `AiConfig` from the global store, refuse if disabled.
pub async fn suggest(ctx: &ConflictContext) -> AppResult<String> {
    let cfg = crate::config_store::load()?.ai;
    if !cfg.enabled {
        return Err(AppError::Config(
            "AI 충돌 해결이 비활성화되어 있습니다. 설정에서 활성화하세요.".into(),
        ));
    }
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(60))
        .build()
        .map_err(|e| AppError::Internal(format!("reqwest: {e}")))?;
    suggest_with(&client, &cfg, ctx).await
}

/// Pure transport function — exercised directly by the wiremock test.
pub async fn suggest_with(
    client: &reqwest::Client,
    cfg: &AiConfig,
    ctx: &ConflictContext,
) -> AppResult<String> {
    if cfg.base_url.trim().is_empty() || cfg.model.trim().is_empty() {
        return Err(AppError::Config(
            "AI 설정이 비어 있습니다. Base URL과 모델명을 입력하세요.".into(),
        ));
    }

    let user_prompt = build_user_prompt(ctx);

    let body = ChatRequest {
        model: &cfg.model,
        messages: vec![
            ChatMessage {
                role: "system",
                content: effective_system_prompt(cfg),
            },
            ChatMessage {
                role: "user",
                content: user_prompt,
            },
        ],
        max_tokens: None,
    };

    let url = format!("{}/chat/completions", cfg.base_url.trim_end_matches('/'));
    let resp = client
        .post(&url)
        .bearer_auth(&cfg.api_key)
        .json(&body)
        .send()
        .await
        .map_err(|e| AppError::Internal(format!("AI 요청 실패: {e}")))?;

    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(AppError::Internal(format!(
            "AI 응답 오류 ({status}): {}",
            body.chars().take(2000).collect::<String>()
        )));
    }

    let parsed: ChatResponse = resp
        .json()
        .await
        .map_err(|e| AppError::Internal(format!("AI 응답 파싱 실패: {e}")))?;

    parsed
        .choices
        .into_iter()
        .next()
        .map(|c| c.message.content)
        .ok_or_else(|| AppError::Internal("AI 응답에 선택지가 없습니다.".into()))
}

/// The prompt actually sent: the manager's pre-configured text when set,
/// otherwise the built-in default.
pub fn effective_system_prompt(cfg: &AiConfig) -> String {
    let custom = cfg.system_prompt.trim();
    if custom.is_empty() {
        DEFAULT_SYSTEM_PROMPT.to_string()
    } else {
        custom.to_string()
    }
}

/// 연결 테스트 결과 — 설정 화면의 "연결 테스트" 버튼이 보여 준다.
#[derive(Debug, Serialize)]
pub struct AiProbeResult {
    pub ok: bool,
    /// 왕복 시간(ms). 실패하면 0.
    pub latency_ms: u64,
    /// 성공이면 응답 전문(짧게), 실패면 사유.
    pub detail: String,
}

/// 저장하지 않은 설정으로 즉시 연결을 시험한다. 병합 해결과 같은
/// `/chat/completions` 경로를 그대로 쓰므로, 성공하면 실제 해결도 같은
/// 서버·모델로 동작한다는 보장이 된다 (pi 같은 커스텀 엔드포인트 포함).
pub async fn probe_with(
    client: &reqwest::Client,
    cfg: &AiConfig,
) -> AiProbeResult {
    if cfg.base_url.trim().is_empty() || cfg.model.trim().is_empty() {
        return AiProbeResult {
            ok: false,
            latency_ms: 0,
            detail: "Base URL과 모델명을 입력하세요.".into(),
        };
    }
    let body = ChatRequest {
        model: cfg.model.trim(),
        messages: vec![ChatMessage {
            role: "user",
            content: "ping".into(),
        }],
        max_tokens: Some(1),
    };
    let url = format!("{}/chat/completions", cfg.base_url.trim_end_matches('/'));
    let start = std::time::Instant::now();
    let resp = match client
        .post(&url)
        .bearer_auth(cfg.api_key.trim())
        .json(&body)
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => {
            return AiProbeResult {
                ok: false,
                latency_ms: 0,
                detail: format!("연결 실패: {e}"),
            }
        }
    };
    let latency_ms = start.elapsed().as_millis() as u64;
    if !resp.status().is_success() {
        let status = resp.status().as_u16();
        let body = resp.text().await.unwrap_or_default();
        return AiProbeResult {
            ok: false,
            latency_ms,
            detail: format!(
                "HTTP {status}: {}",
                body.chars().take(300).collect::<String>()
            ),
        };
    }
    let parsed: Result<ChatResponse, _> = resp.json().await;
    match parsed {
        Ok(p) => match p.choices.into_iter().next() {
            Some(c) => AiProbeResult {
                ok: true,
                latency_ms,
                detail: c.message.content.chars().take(120).collect::<String>(),
            },
            None => AiProbeResult {
                ok: false,
                latency_ms,
                detail: "응답에 선택지가 없습니다. 모델명을 확인하세요.".into(),
            },
        },
        Err(_) => AiProbeResult {
            ok: false,
            latency_ms,
            detail: "응답 형식이 OpenAI 호환이 아닙니다. /chat/completions 를 지원하는 엔드포인트인지 확인하세요.".into(),
        },
    }
}

fn build_user_prompt(ctx: &ConflictContext) -> String {
    use std::fmt::Write;
    let mut s = format!(
        "브렌치 병합이 오류가 나고 있어. 수정한 기능들 영향 없게 수정해줘.\n\n파일: {}\n\n",
        ctx.file_path
    );
    if let Some(base) = &ctx.base {
        let _ = writeln!(s, "```base\n{base}\n```");
    }
    let _ = writeln!(s, "```ours\n{}\n```", ctx.ours);
    let _ = writeln!(s, "```theirs\n{}\n```", ctx.theirs);
    s.push_str("\n위 두 수정(ours, theirs)을 모두 반영한 최종 파일을 반환하세요. 마커 없이 순수 코드만 반환합니다.");
    s
}
