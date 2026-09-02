//! Tauri commands for SSH profile and external tool management.
use serde::{Deserialize, Serialize};

use crate::config_store::{self, SshProfile};
use crate::error::AppResult;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct SshProfilePatch {
    pub default_user: Option<String>,
    pub default_key_path: Option<String>,
    pub default_password: Option<String>,
    pub default_host: Option<String>,
    pub connect_timeout: Option<String>,
    pub default_port: Option<u16>,
}

#[tauri::command]
pub fn get_ssh_profile() -> AppResult<SshProfile> {
    let cfg = config_store::load()?;
    Ok(cfg.ssh_profile)
}

#[tauri::command]
pub fn set_ssh_profile(patch: SshProfilePatch) -> AppResult<SshProfile> {
    let mut cfg = config_store::load()?;
    if let Some(u) = patch.default_user {
        cfg.ssh_profile.default_user = u;
    }
    if let Some(k) = patch.default_key_path {
        cfg.ssh_profile.default_key_path = k;
    }
    if let Some(h) = patch.default_host {
        cfg.ssh_profile.default_host = h;
    }
    if let Some(t) = patch.connect_timeout {
        cfg.ssh_profile.connect_timeout = t;
    }
    if let Some(p) = patch.default_port {
        cfg.ssh_profile.default_port = p;
    }
    config_store::save(&cfg)?;
    Ok(cfg.ssh_profile)
}

#[tauri::command]
pub fn list_external_tools() -> AppResult<Vec<crate::config_store::ExternalTool>> {
    let cfg = config_store::load()?;
    Ok(cfg.external_tools)
}

#[tauri::command]
pub fn set_external_tool(
    tool: crate::config_store::ExternalTool,
) -> AppResult<crate::config_store::ExternalTool> {
    let mut cfg = config_store::load()?;
    if let Some(pos) = cfg.external_tools.iter().position(|t| t.id == tool.id) {
        cfg.external_tools[pos] = tool.clone();
    } else {
        cfg.external_tools.push(tool.clone());
    }
    config_store::save(&cfg)?;
    Ok(tool)
}

#[tauri::command]
pub fn remove_external_tool(id: String) -> AppResult<()> {
    let mut cfg = config_store::load()?;
    cfg.external_tools.retain(|t| t.id != id);
    config_store::save(&cfg)?;
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestSshArgs {
    pub host: String,
    #[serde(default)]
    pub user: String,
    #[serde(default = "default_args_ssh_port")]
    pub port: u16,
    #[serde(default)]
    pub key_path: String,
    #[serde(default)]
    pub password: String,
    #[serde(default = "default_test_timeout")]
    pub timeout_secs: u16,
}

#[tauri::command]
pub fn get_ai_config() -> AppResult<crate::config_store::AiConfig> {
    Ok(config_store::load()?.ai)
}

#[tauri::command]
pub fn set_ai_config(cfg: crate::config_store::AiConfig) -> AppResult<()> {
    let mut s = config_store::load()?;
    s.ai = cfg;
    if s.ai.binary_strategy.trim().is_empty() {
        s.ai.binary_strategy = "theirs".into();
    }
    config_store::save(&s)?;
    Ok(())
}

/// The built-in resolver prompt, so Settings can show it as the placeholder
/// and offer "기본값으로 되돌리기" without duplicating the text in the UI.
#[tauri::command]
pub fn ai_default_prompt() -> String {
    crate::ai::DEFAULT_SYSTEM_PROMPT.to_string()
}

fn default_args_ssh_port() -> u16 {
    22
}

fn default_test_timeout() -> u16 {
    5
}

#[derive(Debug, Clone, Serialize)]
pub struct SshTestReport {
    pub ok: bool,
    pub latency_ms: u64,
    pub user: String,
    pub hostname: String,
    pub system: String,
    pub fingerprint: String,
    pub error: Option<String>,
}

/// Build the ssh command line for `test_ssh_connection`. Password auth is
/// driven by `sshpass -e` (password via `SSHPASS` env, never on argv); the
/// key branch is fully non-interactive (`BatchMode=yes`).
fn build_test_cmd(args: &TestSshArgs, use_password: bool) -> std::process::Command {
    let mut cmd = if use_password {
        let mut c = std::process::Command::new("sshpass");
        c.arg("-e").arg("ssh");
        c.env("SSHPASS", &args.password);
        c.arg("-o")
            .arg("StrictHostKeyChecking=accept-new")
            .arg("-o")
            .arg("PreferredAuthentications=password")
            .arg("-o")
            .arg("PubkeyAuthentication=no")
            .arg("-o")
            .arg("NumberOfPasswordPrompts=1");
        c
    } else {
        let mut c = std::process::Command::new("ssh");
        if !args.key_path.is_empty() {
            c.arg("-i").arg(&args.key_path);
        }
        c.arg("-o").arg("BatchMode=yes");
        c.arg("-o").arg("StrictHostKeyChecking=accept-new");
        c
    };
    if args.port != 22 {
        cmd.arg("-p").arg(args.port.to_string());
    }
    cmd.arg("-o")
        .arg(format!("ConnectTimeout={}", args.timeout_secs));
    if args.user.is_empty() {
        cmd.arg(&args.host);
    } else {
        cmd.arg(format!("{}@{}", args.user, args.host));
    }
    cmd.arg("echo __GC_OK__; whoami; hostname; uname -sr");
    cmd
}

#[tauri::command]
pub async fn test_ssh_connection(args: TestSshArgs) -> AppResult<SshTestReport> {
    use std::time::Instant;

    // -- probe --
    let start = Instant::now();
    let password_auth = !args.password.is_empty();
    // 키와 비밀번호가 모두 있으면 비밀번호를 먼저 시도하고, 서버가 거부하면
    // (예: Ubuntu 기본 `PermitRootLogin prohibit-password`) 키로 재시도한다.
    let wants_fallback = password_auth && !args.key_path.is_empty();
    let sequence: &[bool] = if wants_fallback {
        &[true, false]
    } else {
        &[password_auth]
    };
    let mut probe: Option<std::process::Output> = None;
    let mut last_spawn_err: Option<std::io::Error> = None;
    for use_password in sequence {
        match build_test_cmd(&args, *use_password).output() {
            Ok(o) => {
                let denied = !o.status.success()
                    && String::from_utf8_lossy(&o.stderr).contains("Permission denied");
                probe = Some(o);
                // 성공이거나 인증 거부가 아닌 실패면 여기서 멈춘다.
                if !(wants_fallback && denied) {
                    break;
                }
            }
            Err(e) => {
                last_spawn_err = Some(e);
                break;
            }
        }
    }
    let probe = match probe {
        Some(o) => o,
        None => {
            let sshpass_missing = last_spawn_err
                .as_ref()
                .map(|e| e.kind() == std::io::ErrorKind::NotFound)
                .unwrap_or(false);
            return Ok(SshTestReport {
                ok: false,
                latency_ms: start.elapsed().as_millis() as u64,
                user: String::new(),
                hostname: String::new(),
                system: String::new(),
                fingerprint: String::new(),
                error: Some(if sshpass_missing {
                    "비밀번호 인증에는 sshpass가 필요합니다. (sudo apt install sshpass)".to_string()
                } else if let Some(e) = last_spawn_err {
                    format!("ssh spawn failed: {}", e)
                } else {
                    "ssh failed to start".to_string()
                }),
            });
        }
    };
    let latency_ms = start.elapsed().as_millis() as u64;

    let stdout = String::from_utf8_lossy(&probe.stdout);
    let stderr = String::from_utf8_lossy(&probe.stderr);
    if probe.status.success() && stdout.contains("__GC_OK__") {
        match parse_probe_output(&stdout) {
            Ok((user, hostname, system)) => Ok(SshTestReport {
                ok: true,
                latency_ms,
                user,
                hostname,
                system,
                fingerprint: fetch_ed25519_fingerprint(&args.host, args.port),
                error: None,
            }),
            Err(msg) => Ok(SshTestReport {
                ok: false,
                latency_ms,
                user: String::new(),
                hostname: String::new(),
                system: String::new(),
                fingerprint: String::new(),
                error: Some(msg),
            }),
        }
    } else {
        let err = stderr.trim();
        let error = if err.is_empty() {
            format!("SSH connect failed: exit code {:?}", probe.status.code())
        } else {
            format!("SSH connect failed: {}", err)
        };
        Ok(SshTestReport {
            ok: false,
            latency_ms,
            user: String::new(),
            hostname: String::new(),
            system: String::new(),
            fingerprint: String::new(),
            error: Some(error),
        })
    }
}

fn parse_probe_output(stdout: &str) -> Result<(String, String, String), String> {
    if !stdout.contains("__GC_OK__") {
        return Err("Unexpected probe output (missing __GC_OK__ marker).".into());
    }
    let mut lines = stdout.lines().skip_while(|l| l.trim() != "__GC_OK__");
    // skip the marker line itself
    lines.next();
    let user = lines.next().unwrap_or("").trim().to_string();
    let hostname = lines.next().unwrap_or("").trim().to_string();
    let mut sys_parts: Vec<String> = lines.map(|l| l.trim().to_string()).collect();
    // drop a trailing empty line from the final newline
    while let Some(last) = sys_parts.last() {
        if last.is_empty() {
            sys_parts.pop();
        } else {
            break;
        }
    }
    let system = sys_parts.join(" ");
    if user.is_empty() && hostname.is_empty() && system.is_empty() {
        Err("Unexpected probe output (missing fields).".into())
    } else {
        Ok((user, hostname, system))
    }
}

pub fn fetch_ed25519_fingerprint(host: &str, port: u16) -> String {
    use std::io::Write;
    use std::process::{Command, Stdio};

    if host.is_empty() {
        return String::new();
    }
    let scan_out = match Command::new("ssh-keyscan")
        .arg("-p")
        .arg(port.to_string())
        .arg("-T")
        .arg("5")
        .arg("-t")
        .arg("ed25519")
        .arg(host)
        .output()
    {
        Ok(o) => o,
        Err(_) => return String::new(),
    };
    let mut keygen = match Command::new("ssh-keygen")
        .arg("-lf")
        .arg("-")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(c) => c,
        Err(_) => return String::new(),
    };
    if let Some(mut stdin) = keygen.stdin.take() {
        let _ = stdin.write_all(&scan_out.stdout);
    }
    let out = match keygen.wait_with_output() {
        Ok(o) => o,
        Err(_) => return String::new(),
    };
    parse_fingerprint_line(&String::from_utf8_lossy(&out.stdout)).unwrap_or_default()
}

fn parse_fingerprint_line(keygen_stdout: &str) -> Option<String> {
    keygen_stdout
        .lines()
        .find(|l| l.contains("(ED25519)"))
        .and_then(|l| {
            let idx = l.find("SHA256:")?;
            let rest = &l[idx..];
            let end = rest
                .find(|c: char| !(c.is_ascii_alphanumeric() || c == '+' || c == '/' || c == ':'))
                .unwrap_or(rest.len());
            Some(rest[..end].to_string())
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_probe_output_extracts_fields() {
        let out = "__GC_OK__\nubuntu\nhost-42\nLinux ubuntu 6.8.0-45-generic x86_64\n";
        let (user, hostname, system) = parse_probe_output(out).unwrap();
        assert_eq!(user, "ubuntu");
        assert_eq!(hostname, "host-42");
        assert!(system.contains("Linux"));
        assert!(system.contains("6.8.0-45-generic"));
    }

    #[test]
    fn parse_probe_output_missing_marker_errors() {
        assert!(parse_probe_output("just some text\n").is_err());
        assert!(parse_probe_output("").is_err());
    }

    #[test]
    fn fingerprint_line_parses_sha256() {
        let line = "host ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAI... comment\n";
        let keygen = "256 SHA256:abc123+ABC/xyz host (ED25519)\n";
        assert_eq!(
            parse_fingerprint_line(keygen).unwrap(),
            "SHA256:abc123+ABC/xyz"
        );
        assert!(parse_fingerprint_line(line).is_none());
    }
}
