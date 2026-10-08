/**
 * 与 Rust 侧输出对齐的类型。
 *
 * 🔴 所有结构体在 Rust 那边都是 `#[serde(rename_all = "camelCase")]`，
 * 所以这里一律用驼峰。改动 Rust 输出字段时**必须同步这里**。
 */

/** 体积统计。`capped` = 因为到了上限而提前停下，界面上要标「≥」而不是假装是精确值。 */
export interface Measure {
  files: number;
  bytes: number;
  capped: boolean;
}

/** 一个数据目录的检测结果。 */
export interface PathHit {
  template: string;
  resolved: string;
  kind: string;
  isLink: boolean;
  linkTarget: string | null;
  linkBroken: boolean;
  measure: Measure;
  sizeText: string;
}

export interface BrokenLink {
  path: string;
  target: string | null;
}

export interface EnvRelocate {
  env: string;
  note: string | null;
}

/** 注册表条目（`agents.toml` 的一条）。 */
export interface AgentDef {
  id: string;
  name: string;
  vendor: string | null;
  kind: string;
  /** safe | normal | special | system */
  class: string;
  paths: string[];
  updater: string[];
  envRelocate: EnvRelocate | null;
  processes: string[];
  skip: string[];
  runtimeLinks: string[];
  note: string | null;
}

export interface DetectedAgent {
  id: string;
  name: string;
  vendor: string | null;
  kind: string;
  class: string;
  note: string | null;
  envRelocate: EnvRelocate | null;
  detected: boolean;
  how: string[];
  paths: PathHit[];
  updaters: string[];
  brokenLinks: BrokenLink[];
  running: string[];
  total: Measure;
}

export interface DriveInfo {
  letter: string;
  free: number;
  total: number;
  freeText: string;
  usedPercent: number;
  isSystem: boolean;
  isDefaultTarget: boolean;
}

export interface DrivesPayload {
  drives: DriveInfo[];
  defaultDestRoot: string | null;
  systemDrive: string | null;
}

export interface Plan {
  agentId: string;
  agentName: string;
  src: string;
  dest: string;
  snapshotPrefix: string;
  srcMeasure: Measure;
  destFree: number;
  destExisting: Measure;
  blockers: string[];
  warnings: string[];
  fits: boolean;
}

export interface StepResult {
  name: string;
  ok: boolean;
  detail: string;
}

export interface Progress {
  /** copying | verifying | snapshotting | linking | verifying-link | done */
  step: string;
  detail: string;
  percent: number | null;
}

export interface MigrateReport {
  ok: boolean;
  src: string;
  dest: string;
  snapshot: string | null;
  steps: StepResult[];
  error: string | null;
  rolledBack: boolean;
}

export interface Snapshot {
  path: string;
  name: string;
  agentId: string;
  created: string;
  measure: Measure;
}

/** 迁移状态的展示口径（与 Rust `DetectedAgent::state()` 一致）。 */
export type AgentState = "moved" | "free" | "absent";

export function agentState(a: DetectedAgent): AgentState {
  if (!a.detected) return "absent";
  // 🔴 断链不算已迁移 —— 数据其实没接上
  if (a.paths.some((p) => p.isLink && !p.linkBroken)) return "moved";
  return "free";
}

export function stateLabel(s: AgentState): string {
  return s === "moved" ? "已迁移" : s === "free" ? "未迁移" : "未检测到";
}

/** 人类可读体积。与 Rust `fsutil::human` 保持同样的口径（1024 进制、两位小数）。 */
export function human(bytes: number): string {
  const U = ["B", "KB", "MB", "GB", "TB"];
  if (bytes === 0) return "0 B";
  let v = bytes;
  let i = 0;
  while (v >= 1024 && i < U.length - 1) {
    v /= 1024;
    i += 1;
  }
  return i === 0 ? `${bytes} B` : `${v.toFixed(2)} ${U[i]}`;
}
