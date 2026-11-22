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

/** 「应用本身还装没装」的判定线索（`agents.toml` 的 `install`）。 */
export interface InstallHint {
  uninstall: string[];
  exe: string[];
}

/** 注册表条目（`agents.toml` 的一条）。 */
export interface AgentDef {
  id: string;
  name: string;
  vendor: string | null;
  kind: string;
  /** agent（真的是个 Agent）| cache（纯缓存/工具链目录） */
  category: string;
  /** safe | normal | special | system */
  class: string;
  paths: string[];
  updater: string[];
  envRelocate: EnvRelocate | null;
  processes: string[];
  skip: string[];
  runtimeLinks: string[];
  note: string | null;
  install: InstallHint | null;
}

export interface RelocatePlan {
  /** env | launch_arg | junction | setting */
  kind: string;
  label: string;
  note: string;
  detail: string | null;
  auto: boolean;
  alternatives: string[];
}

export interface DetectedAgent {
  id: string;
  name: string;
  vendor: string | null;
  kind: string;
  /** agent | cache —— 侧栏分组用 */
  category: string;
  class: string;
  note: string | null;
  envRelocate: EnvRelocate | null;
  detected: boolean;
  /** 应用本身还装没装：false = 已卸载只剩缓存；null = 判断不了 */
  installed: boolean | null;
  how: string[];
  paths: PathHit[];
  updaters: string[];
  brokenLinks: BrokenLink[];
  running: string[];
  /** 命中了但被 `ignore_processes` 排除的常驻服务（不算应用在跑） */
  ignored: string[];
  /** 内核按 agents.toml 的声明自动推荐的最优迁移方式 */
  strategy: RelocatePlan;
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
  /** ≥1GB 的大文件数（用于预估复制耗时；undefined = 旧版后端） */
  largeFiles?: number;
  largeFilesCapped?: boolean;
}

export interface StepResult {
  name: string;
  ok: boolean;
  detail: string;
}

export interface RepairReport {
  ok: boolean;
  agentId: string;
  src: string;
  dest: string;
  /** rebuilt（坏链接删掉重建）| created（链接缺失直接建）| noop（本来就是好的） */
  action: "rebuilt" | "created" | "noop" | string;
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

/** 一个「将被清理」的目录（清理缓存预览 / 报告的一项）。 */
export interface PurgeItem {
  path: string;
  /** 数据目录 | 更新器目录 */
  label: string;
  bytes: number;
  files: number;
  capped: boolean;
  /** pending（将被清理）| trashed（已移入回收站）| skip（不动）| failed */
  action: string;
  note: string | null;
}

/** 清理缓存的结果。`freed` 是移入回收站的字节数 —— 清空回收站才真正释放。 */
export interface PurgeReport {
  ok: boolean;
  items: PurgeItem[];
  freed: number;
  freedText: string;
  problems: string[];
}

/** 迁移状态的展示口径（与 Rust `DetectedAgent::state()` 一致）。 */
export type AgentState = "moved" | "free" | "orphan" | "absent";

export function agentState(a: DetectedAgent): AgentState {
  if (!a.detected) return "absent";
  // 应用都不在了，还谈「迁移过没」没意义 —— 这是「已卸载，只剩缓存」
  if (a.installed === false) return "orphan";
  // 🔴 断链不算已迁移 —— 数据其实没接上
  if (a.paths.some((p) => p.isLink && !p.linkBroken)) return "moved";
  return "free";
}

export function stateLabel(s: AgentState): string {
  return s === "moved"
    ? "已迁移"
    : s === "free"
      ? "未迁移"
      : s === "orphan"
        ? "已卸载"
        : "未检测到";
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
