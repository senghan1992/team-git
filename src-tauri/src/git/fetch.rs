use crate::error::AppResult;
use crate::git::run;

/// `git fetch origin` — optionally with an inline credential for HTTPS remotes.
pub fn fetch_origin(repo_path: &std::path::Path, token: Option<&str>) -> AppResult<String> {
    let mut args: Vec<String> = vec!["fetch".into(), "--prune".into(), "origin".into()];
    if let Some(t) = token {
        if !t.is_empty() {
            args.insert(2, format!("https://oauth2:{t}@placeholder.invalid"));
            args.insert(2, "--upload-pack".into());
            // The url is only used to derive credentials; the actual fetch
            // target is `origin`. We rewrite the URL of `origin` instead so
            // git picks up the inline credential.
            let _ = run(
                Some(repo_path),
                [
                    "remote",
                    "set-url",
                    "origin",
                    &format!("https://oauth2:{t}@placeholder.invalid"),
                ],
            );
        }
    }
    let out = run(Some(repo_path), args)?;
    Ok(out.stderr)
}

/// Restore a placeholder origin URL back to whatever was in config.
pub fn fetch_origin_with_url(repo_path: &std::path::Path, url: &str) -> AppResult<String> {
    if !url.is_empty() {
        let _ = run(Some(repo_path), ["remote", "set-url", "origin", url]);
    }
    let out = run(Some(repo_path), ["fetch", "--prune", "origin"])?;
    Ok(out.stderr)
}

/// `git fetch --prune <remote>` — used by the merge-center when the user hits
/// "가져오기". Returns the stderr stream for the UI to surface on failure.
pub fn fetch_target(target: &crate::git::Target, remote: &str) -> AppResult<String> {
    let out = crate::git::run_at_target(target, ["fetch", "--prune", remote])?;
    if !out.ok() {
        return Err(crate::error::AppError::Git(out.stderr.trim().to_string()));
    }
    Ok(out.stderr)
}
