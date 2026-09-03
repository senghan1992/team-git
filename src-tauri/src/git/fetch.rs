use crate::error::AppResult;
use crate::git::run;

/// `git fetch --prune origin` — best-effort (오프라인이어도 이어지는 병합이
/// 마지막 fetch 시점의 트래킹 ref 로 계속 동작해야 하므로 종료 코드를 따지지
/// 않는다). 호출부가 실패를 알아야 하면 `fetch_target` 을 쓴다.
///
/// 예전에는 `token` 인자가 있어 origin URL 을
/// `https://oauth2:<token>@placeholder.invalid` 로 **영구히 바꿔치기**했다 —
/// 토큰이 git config 에 평문으로 남고, 이후 모든 원격 동작이 깨졌다.
/// 어떤 커맨드도 그 경로를 쓰지 않았으므로 제거했다. HTTPS 자격증명은
/// push 와 같은 GIT_ASKPASS 방식만 쓴다.
pub fn fetch_origin(repo_path: &std::path::Path) -> AppResult<String> {
    let out = run(Some(repo_path), ["fetch", "--prune", "origin"])?;
    Ok(out.stderr)
}

/// `git fetch --prune <remote>` — used by the merge-center when the user hits
/// "가져오기". Returns the stderr stream for the UI to surface on failure.
pub fn fetch_target(target: &crate::git::Target, remote: &str) -> AppResult<String> {
    let out = crate::git::run_at_target(target, ["fetch", "--prune", remote])?;
    if !out.ok() {
        return Err(crate::error::AppError::Git(
            crate::git::ops::friendly_git_error(&out.stderr),
        ));
    }
    Ok(out.stderr)
}
