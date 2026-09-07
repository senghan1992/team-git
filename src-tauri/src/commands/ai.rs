//! Tauri commands for the optional AI conflict suggester.
//!
//! The credentials live in `AiConfig` (loaded from the global config store by
//! `ai::suggest`), so the command takes no auth-related arguments — only the
//! conflict body itself.
use crate::ai::{self, ConflictContext};
use crate::config_store::AiConfig;
use crate::error::AppResult;

#[tauri::command]
pub async fn ai_suggest_resolution(
    file_path: String,
    base: Option<String>,
    ours: String,
    theirs: String,
) -> AppResult<String> {
    let ctx = ConflictContext {
        file_path,
        base,
        ours,
        theirs,
    };
    ai::suggest(&ctx).await
}

/// 저장하지 않은 설정으로 연결을 시험한다 — 설정 화면의 "연결 테스트" 버튼은
/// 이 명령으로 성공/실패와 지연시간을 보여 준다. 저장 여부와 무관하게 동작하므로
/// 입력 중인 설정을 바로 확인할 수 있다.
#[tauri::command]
pub async fn ai_probe(cfg: AiConfig) -> ai::AiProbeResult {
    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            return ai::AiProbeResult {
                ok: false,
                latency_ms: 0,
                detail: format!("클라이언트 생성 실패: {e}"),
            }
        }
    };
    ai::probe_with(&client, &cfg).await
}
