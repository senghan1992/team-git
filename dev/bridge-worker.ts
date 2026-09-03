// 미리보기 브리지의 워커 스레드 진입점. `git-bridge.ts` 의 BridgePool 이
// esbuild 로 번들해 `worker_threads` 로 띄운다 — 저장소 명령(spawnSync git)이
// 여기서 돌아가므로 느린 SSH 저장소 하나가 개발 서버 전체를 막지 않는다.
import { parentPort } from "node:worker_threads";
import { dispatch, type InvokeArgs } from "./git-bridge";

parentPort?.on("message", async (msg: { id: number; body: InvokeArgs }) => {
  let result: unknown;
  try {
    result = await dispatch(msg.body);
  } catch (e) {
    result = { kind: "bad_request", message: (e as Error).message ?? String(e) };
  }
  parentPort?.postMessage({ id: msg.id, result });
});
