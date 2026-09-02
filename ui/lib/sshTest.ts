// SSH connection test helpers (Termius-style proof report).
import { ipc, type SshTestReport } from "./ipc";

export interface SshTestInput {
  host: string;
  user: string;
  port: number;
  key_path: string;
  password: string;
}

/** Parse a port input; anything outside 1-65535 (or unparseable) falls back to 22. */
export function normalizePort(raw: string): number {
  const n = parseInt(raw, 10);
  if (Number.isNaN(n) || n < 1 || n > 65535) return 22;
  return n;
}

export function runSshTest(input: SshTestInput): Promise<SshTestReport> {
  return ipc.testSshConnection({ ...input, timeout_secs: 5 });
}

function escape(s: string): string {
  return s
    .replace(/&/g, "&amp;")
    .replace(/"/g, "&quot;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;");
}

/** Render a proof report into a result container. */
export function renderSshTestReport(container: HTMLElement, report: SshTestReport): void {
  const title = report.ok
    ? `연결 성공 (${report.latency_ms} ms)`
    : "연결 실패";
  const titleClass = report.ok ? "ssh-test-ok font-bold" : "ssh-test-fail font-bold";
  const fp = report.fingerprint || "—";
  const rows = [
    ["사용자", report.user],
    ["호스트명", report.hostname],
    ["시스템", report.system],
    ["호스트 키 지문", fp],
  ]
    .map(
      ([k, v]) =>
        `<div class="ssh-test-row"><span class="ssh-test-label">${escape(k)}</span><span>${escape(v)}</span></div>`
    )
    .join("");
  const errorHtml = report.error
    ? `<pre class="ssh-test-error">${escape(report.error)}</pre>`
    : "";
  container.innerHTML = `
    <div class="${titleClass}">${escape(title)}</div>
    <div class="ssh-test-details">${rows}</div>
    ${errorHtml}
  `;
}