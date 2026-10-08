/**
 * Tauri 命令封装。
 *
 * 所有实际逻辑都在 `acm-core` 里，这里只负责把参数递过去、把结果取回来。
 * 命令名必须与 `src-tauri/src/lib.rs` 里 `generate_handler!` 注册的一致。
 *
 * `VITE_DEMO_MODE=1` 时全部走 `demo.ts` 的假数据 —— 这样界面能脱离 Rust 壳
 * 在浏览器里渲染，方便截图、评审和调样式（改 CSS 不用每次等 cargo 编译）。
 */

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import * as demoData from "./demo";
import type {
  AgentDef,
  DetectedAgent,
  DrivesPayload,
  MigrateReport,
  Plan,
  Progress,
  Snapshot,
} from "./types";

/** 是否演示模式（界面右上角会显示角标，避免和真实数据混淆）。 */
export const DEMO: boolean = import.meta.env.VITE_DEMO_MODE === "1";

/** 检测全部 Agent。`measure=false` 时只判存在性（很快）。 */
export function detect(measure: boolean, only?: string): Promise<DetectedAgent[]> {
  if (DEMO) {
    const list = only ? demoData.AGENTS.filter((a) => a.id === only) : demoData.AGENTS;
    // 演示模式里「不量体积」就把体积清零，忠实模拟真机的两段式加载
    const out = measure
      ? list
      : list.map((a) => ({ ...a, total: { files: 0, bytes: 0, capped: false } }));
    return Promise.resolve(out);
  }
  return invoke("detect", { measure, only: only ?? null });
}

export function listAgents(): Promise<AgentDef[]> {
  if (DEMO) return Promise.resolve(demoData.DEFS);
  return invoke("list_agents");
}

export function drives(): Promise<DrivesPayload> {
  if (DEMO) return Promise.resolve(demoData.DRIVES);
  return invoke("drives");
}

/** 生成迁移计划 —— 不会动任何文件。 */
export function plan(agentId: string, destRoot: string): Promise<Plan> {
  if (DEMO) return Promise.resolve(demoData.demoPlan(agentId, destRoot));
  return invoke("plan", { agentId, destRoot });
}

export function migrateAgent(agentId: string, destRoot: string): Promise<MigrateReport> {
  if (DEMO) {
    // 播放一遍进度，让界面上的步骤条和日志真的动起来
    demoData.PROGRESS_SEQ.forEach((p, i) => {
      setTimeout(() => demoHandlers.forEach((h) => h(p)), i * 260);
    });
    return new Promise((r) =>
      setTimeout(() => r({ ...demoData.REPORT, src: demoData.demoPlan(agentId, destRoot).src }), 2400),
    );
  }
  return invoke("migrate_agent", { agentId, destRoot });
}

export function rollback(agentId: string): Promise<string> {
  if (DEMO) return Promise.resolve(`已回滚：${agentId}（演示模式，没有真的动文件）`);
  return invoke("rollback", { agentId });
}

export function snapshots(): Promise<Snapshot[]> {
  if (DEMO) return Promise.resolve(demoData.SNAPSHOTS);
  return invoke("snapshots");
}

export function deleteSnapshot(
  path: string,
  permanent: boolean,
): Promise<{ ok: boolean; freed: number; freedText: string }> {
  if (DEMO) {
    return Promise.resolve({ ok: true, freed: 3.42 * 1024 ** 3, freedText: "3.42 GB" });
  }
  return invoke("delete_snapshot", { path, permanent });
}

export function openPath(path: string): Promise<void> {
  if (DEMO) {
    console.info("[demo] 打开路径：", path);
    return Promise.resolve();
  }
  return invoke("open_path", { path });
}

export function version(): Promise<{ app: string; core: string }> {
  if (DEMO) return Promise.resolve({ app: "0.1.0-demo", core: "0.1.0" });
  return invoke("version");
}

/** 演示模式的进度订阅者（没有真实的 Tauri 事件源，用本地数组转发）。 */
const demoHandlers: ((p: Progress) => void)[] = [];

/** 订阅迁移进度。返回取消订阅的函数。 */
export function onProgress(cb: (p: Progress) => void): Promise<UnlistenFn> {
  if (DEMO) {
    demoHandlers.push(cb);
    return Promise.resolve(() => {
      const i = demoHandlers.indexOf(cb);
      if (i >= 0) demoHandlers.splice(i, 1);
    });
  }
  return listen<Progress>("migrate-progress", (e) => cb(e.payload));
}

/** 命名空间形式的导出 —— 组件里写 `api.detect(...)` 更像「这是一个后端接口」。 */
export const api = {
  DEMO,
  detect,
  listAgents,
  drives,
  plan,
  migrateAgent,
  rollback,
  snapshots,
  deleteSnapshot,
  openPath,
  version,
  onProgress,
};
