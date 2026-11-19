/**
 * 演示数据 —— `VITE_DEMO_MODE=1` 时启用，让前端能脱离 Tauri 在浏览器里渲染。
 *
 * 用途：截图、评审界面、改样式时不用每次跑起来整个 Rust 壳。
 * **数值取自真机实测**（`acm detect` 的输出），不是随手编的 —— 这样布局问题一眼能看出来。
 */

import type {
  AgentDef,
  DetectedAgent,
  DrivesPayload,
  MigrateReport,
  Plan,
  Progress,
  Snapshot,
} from "./types";

const GB = 1024 ** 3;

function m(bytes: number, files: number, capped = false) {
  return { files, bytes, capped };
}

const DEFS_RAW: Omit<AgentDef, "category" | "install">[] = [
  { id: "workbuddy-cn", name: "WorkBuddy 国内版", vendor: "腾讯", kind: "electron", class: "normal", paths: ["~/.workbuddy"], updater: ["@genieworkbuddy-desktop-updater"], envRelocate: null, processes: ["WorkBuddy.exe"], skip: [], runtimeLinks: [], note: null },
  { id: "codex", name: "Codex CLI", vendor: "OpenAI", kind: "cli", class: "normal", paths: ["~/.codex"], updater: [], envRelocate: { env: "CODEX_HOME", note: "官方支持" }, processes: ["codex*"], skip: [], runtimeLinks: [], note: null },
];

/** 演示数据里哪些不是 Agent（与 agents.toml 的 `category` 对齐）。 */
const NON_AGENT = new Set(["npm-cache"]);
/** 演示数据里哪些「已经卸载、只剩缓存」（与 agents.toml 的 `install` 判定对齐）。 */
const UNINSTALLED = new Set(["utools", "trae-intl"]);

export const DEFS: AgentDef[] = DEFS_RAW.map((d) => ({
  ...d,
  category: NON_AGENT.has(d.id) ? "cache" : "agent",
  install: null,
}));

const AGENTS_RAW: Omit<DetectedAgent, "category" | "installed" | "ignored">[] = [
  {
    id: "workbuddy-cn",
    name: "WorkBuddy 国内版",
    vendor: "腾讯",
    kind: "electron",
    class: "normal",
    note: null,
    envRelocate: null,
    detected: true,
    how: ["存在 ~/.workbuddy", "存在更新器目录"],
    paths: [
      {
        template: "~/.workbuddy",
        resolved: "C:\\Users\\29436\\.workbuddy",
        kind: "link",
        isLink: true,
        linkTarget: "F:\\.WBcache\\.workbuddy",
        linkBroken: false,
        measure: m(18.6 * GB, 221_045, true),
        sizeText: "≥18.60 GB",
      },
    ],
    updaters: ["C:\\Users\\29436\\AppData\\Local\\@genieworkbuddy-desktop-updater"],
    brokenLinks: [],
    running: [],
    total: m(18.6 * GB, 221_045, true),
  },
  {
    id: "workbuddy-intl",
    name: "WorkBuddy 国际版",
    vendor: "腾讯",
    kind: "electron",
    class: "normal",
    note: null,
    envRelocate: null,
    detected: true,
    how: ["存在 ~/.workbuddy-ai"],
    paths: [
      {
        template: "~/.workbuddy-ai",
        resolved: "C:\\Users\\29436\\.workbuddy-ai",
        kind: "link",
        isLink: true,
        linkTarget: "F:\\.WBcache\\.workbuddy-ai\\.workbuddy-ai",
        linkBroken: false,
        measure: m(5.46 * GB, 91_233),
        sizeText: "5.46 GB",
      },
    ],
    updaters: ["C:\\Users\\29436\\AppData\\Local\\@genieworkbuddy-desktop-updater"],
    brokenLinks: [],
    running: [],
    total: m(5.46 * GB, 91_233),
  },
  {
    id: "codex",
    name: "Codex CLI",
    vendor: "OpenAI",
    kind: "cli",
    class: "normal",
    note: "CLI 类 Agent 通常没有常驻进程；但正在跑的会话会让校验永远看不到 0 差异。",
    envRelocate: { env: "CODEX_HOME", note: "官方支持" },
    detected: true,
    how: ["存在 ~/.codex"],
    paths: [
      {
        template: "~/.codex",
        resolved: "C:\\Users\\29436\\.codex",
        kind: "dir",
        isLink: false,
        linkTarget: null,
        linkBroken: false,
        measure: m(1.08 * GB, 9_155),
        sizeText: "1.08 GB",
      },
    ],
    updaters: [],
    brokenLinks: [],
    running: ["codex-windows-sandbox-service.exe"],
    total: m(1.08 * GB, 9_155),
  },
  {
    id: "claude",
    name: "Claude Code",
    vendor: "Anthropic",
    kind: "cli",
    class: "normal",
    note: null,
    envRelocate: { env: "CLAUDE_CONFIG_DIR", note: "changelog 提到支持" },
    detected: true,
    how: ["存在 ~/.claude"],
    paths: [
      {
        template: "~/.claude",
        resolved: "C:\\Users\\29436\\.claude",
        kind: "dir",
        isLink: false,
        linkTarget: null,
        linkBroken: false,
        measure: m(0.32 * GB, 4_102),
        sizeText: "0.32 GB",
      },
    ],
    updaters: [],
    brokenLinks: [],
    running: [],
    total: m(0.32 * GB, 4_102),
  },
  {
    id: "cursor",
    name: "Cursor",
    vendor: "Anysphere",
    kind: "electron",
    class: "normal",
    note: null,
    envRelocate: null,
    detected: true,
    how: ["存在 %APPDATA%\\Cursor"],
    paths: [
      {
        template: "%APPDATA%\\Cursor",
        resolved: "C:\\Users\\29436\\AppData\\Roaming\\Cursor",
        kind: "dir",
        isLink: false,
        linkTarget: null,
        linkBroken: false,
        measure: m(1.18 * GB, 12_840),
        sizeText: "1.18 GB",
      },
    ],
    updaters: ["C:\\Users\\29436\\AppData\\Local\\cursor-updater"],
    brokenLinks: [],
    running: [],
    total: m(1.18 * GB, 12_840),
  },
  {
    id: "trae",
    name: "Trae CN",
    vendor: "字节跳动",
    kind: "electron",
    class: "normal",
    note: null,
    envRelocate: null,
    detected: true,
    how: ["存在 %APPDATA%\\Trae CN"],
    paths: [
      {
        template: "%APPDATA%\\Trae CN",
        resolved: "C:\\Users\\29436\\AppData\\Roaming\\Trae CN",
        kind: "dir",
        isLink: false,
        linkTarget: null,
        linkBroken: false,
        measure: m(0.24 * GB, 3_204),
        sizeText: "0.24 GB",
      },
    ],
    updaters: [],
    brokenLinks: [],
    running: [],
    total: m(0.24 * GB, 3_204),
  },
  {
    id: "zcode",
    name: "ZCode",
    vendor: null,
    kind: "electron",
    class: "normal",
    note: null,
    envRelocate: null,
    detected: true,
    how: ["存在 %APPDATA%\\ZCode"],
    paths: [
      {
        template: "%APPDATA%\\ZCode",
        resolved: "C:\\Users\\29436\\AppData\\Roaming\\ZCode",
        kind: "dir",
        isLink: false,
        linkTarget: null,
        linkBroken: false,
        measure: m(0.19 * GB, 2_811),
        sizeText: "0.19 GB",
      },
    ],
    updaters: ["C:\\Users\\29436\\AppData\\Local\\@zcodedesktop-updater"],
    brokenLinks: [],
    running: [],
    total: m(0.19 * GB, 2_811),
  },
  {
    id: "codebuddy-cn",
    name: "CodeBuddy 国内版",
    vendor: "腾讯",
    kind: "electron",
    class: "normal",
    note: null,
    envRelocate: null,
    detected: true,
    how: ["存在 %APPDATA%\\CodeBuddy CN"],
    paths: [
      {
        template: "%APPDATA%\\CodeBuddy CN",
        resolved: "C:\\Users\\29436\\AppData\\Roaming\\CodeBuddy CN",
        kind: "dir",
        isLink: false,
        linkTarget: null,
        linkBroken: false,
        measure: m(0.14 * GB, 1_902),
        sizeText: "0.14 GB",
      },
    ],
    updaters: [],
    brokenLinks: [],
    running: [],
    total: m(0.14 * GB, 1_902),
  },
  {
    id: "deepseek",
    name: "DeepSeek Harness",
    vendor: "深度求索",
    kind: "electron",
    class: "normal",
    note: "产品名是 DeepSeek Harness（DSH），不是 DeepSeek 网页版客户端。",
    envRelocate: null,
    detected: true,
    how: ["存在 %APPDATA%\\@deepseek-ai"],
    paths: [
      {
        template: "%APPDATA%\\@deepseek-ai",
        resolved: "C:\\Users\\29436\\AppData\\Roaming\\@deepseek-ai",
        kind: "dir",
        isLink: false,
        linkTarget: null,
        linkBroken: false,
        measure: m(0.08 * GB, 1_044),
        sizeText: "0.08 GB",
      },
    ],
    updaters: ["C:\\Users\\29436\\AppData\\Local\\@deepseek-aidsh-desktop-updater"],
    brokenLinks: [],
    running: [],
    total: m(0.08 * GB, 1_044),
  },
  {
    id: "npm-cache",
    name: "npm 缓存",
    vendor: "npm",
    kind: "cli",
    class: "safe",
    note: "纯缓存，删了也会重新下载 —— 迁走是纯赚。",
    envRelocate: null,
    detected: true,
    how: ["存在 %LOCALAPPDATA%\\npm-cache"],
    paths: [
      {
        template: "%LOCALAPPDATA%\\npm-cache",
        resolved: "C:\\Users\\29436\\AppData\\Local\\npm-cache",
        kind: "dir",
        isLink: false,
        linkTarget: null,
        linkBroken: false,
        measure: m(2.4 * GB, 48_134),
        sizeText: "2.40 GB",
      },
    ],
    updaters: [],
    brokenLinks: [],
    running: [],
    total: m(2.4 * GB, 48_134),
  },
  {
    id: "cherry",
    name: "Cherry Studio",
    vendor: null,
    kind: "electron",
    class: "normal",
    note: null,
    envRelocate: null,
    detected: false,
    how: [],
    paths: [
      {
        template: "%APPDATA%\\CherryStudio",
        resolved: "C:\\Users\\29436\\AppData\\Roaming\\CherryStudio",
        kind: "missing",
        isLink: false,
        linkTarget: null,
        linkBroken: false,
        measure: m(0, 0),
        sizeText: "—",
      },
    ],
    updaters: [],
    brokenLinks: [],
    running: [],
    total: m(0, 0),
  },
  {
    id: "doubao",
    name: "豆包",
    vendor: "字节跳动",
    kind: "electron",
    class: "normal",
    note: "大头在 LocalAppData 的 User Data；Roaming 下那个只有配置。",
    envRelocate: null,
    detected: true,
    how: ["存在 %LOCALAPPDATA%\\Doubao"],
    paths: [
      {
        template: "%LOCALAPPDATA%\\Doubao",
        resolved: "C:\\Users\\29436\\AppData\\Local\\Doubao",
        kind: "dir",
        isLink: false,
        linkTarget: null,
        linkBroken: false,
        measure: m(3.75 * GB, 37_636),
        sizeText: "3.75 GB",
      },
      {
        template: "%APPDATA%\\Doubao",
        resolved: "C:\\Users\\29436\\AppData\\Roaming\\Doubao",
        kind: "dir",
        isLink: false,
        linkTarget: null,
        linkBroken: false,
        measure: m(1939, 2),
        sizeText: "1.89 KB",
      },
    ],
    updaters: [],
    brokenLinks: [],
    running: [],
    total: m(3.75 * GB, 37_638),
  },
  // 下面两条是「应用早就卸载了、只剩缓存」的样子（`installed: false` → 状态 orphan）
  {
    id: "utools",
    name: "uTools",
    vendor: null,
    kind: "electron",
    class: "normal",
    note: null,
    envRelocate: null,
    detected: true,
    how: ["存在更新器目录（应用本体已不在）"],
    paths: [
      {
        template: "%LOCALAPPDATA%\\utools-updater",
        resolved: "C:\\Users\\29436\\AppData\\Local\\utools-updater",
        kind: "dir",
        isLink: false,
        linkTarget: null,
        linkBroken: false,
        measure: m(0, 0),
        sizeText: "0 B",
      },
    ],
    updaters: ["C:\\Users\\29436\\AppData\\Local\\utools-updater"],
    brokenLinks: [],
    running: [],
    total: m(0, 0),
  },
  {
    id: "trae-intl",
    name: "Trae 国际版",
    vendor: "字节跳动",
    kind: "electron",
    class: "normal",
    note: null,
    envRelocate: null,
    detected: true,
    how: ["存在 %APPDATA%\\Trae"],
    paths: [
      {
        template: "%APPDATA%\\Trae",
        resolved: "C:\\Users\\29436\\AppData\\Roaming\\Trae",
        kind: "dir",
        isLink: false,
        linkTarget: null,
        linkBroken: false,
        measure: m(72.1 * GB / 1024, 355),
        sizeText: "72.09 MB",
      },
    ],
    updaters: [],
    brokenLinks: [],
    running: [],
    total: m(72.1 * GB / 1024, 355),
  },
];

export const AGENTS: DetectedAgent[] = AGENTS_RAW.map((a) => ({
  ...a,
  category: NON_AGENT.has(a.id) ? "cache" : "agent",
  installed: UNINSTALLED.has(a.id) ? false : true,
  // Codex 的沙箱服务常驻：演示「后台服务不算运行中」这条提示
  ignored: a.id === "codex" ? ["codex-windows-sandbox-service.exe"] : [],
}));

const GIB = 1024 ** 3;

export const DRIVES: DrivesPayload = {
  systemDrive: "C:",
  defaultDestRoot: "E:\\AgentCache",
  drives: [
    { letter: "C:\\", free: 54.43 * GIB, total: 400.9 * GIB, freeText: "54.43 GB", usedPercent: 86, isSystem: true, isDefaultTarget: false },
    { letter: "D:\\", free: 61.53 * GIB, total: 200.0 * GIB, freeText: "61.53 GB", usedPercent: 69, isSystem: false, isDefaultTarget: false },
    { letter: "E:\\", free: 288.4 * GIB, total: 400.0 * GIB, freeText: "288.40 GB", usedPercent: 28, isSystem: false, isDefaultTarget: true },
    { letter: "F:\\", free: 277.22 * GIB, total: 931.5 * GIB, freeText: "277.22 GB", usedPercent: 70, isSystem: false, isDefaultTarget: false },
  ],
};

/** 按 agent id 给一个「看起来真实」的计划。 */
export function demoPlan(agentId: string, destRoot: string): Plan {
  const a = AGENTS.find((x) => x.id === agentId) ?? AGENTS[0];
  const dest = `${destRoot.replace(/[\\/]+$/, "")}\\${a.id}`;

  const blockers: string[] = [];
  const warnings: string[] = [];
  if (a.running.length > 0) {
    blockers.push(
      `${a.running.length} 个进程正在使用这个目录，必须先完全退出：${a.running.join(", ")}`,
    );
  }
  if (a.envRelocate) {
    warnings.push(
      "这个 Agent 官方支持用环境变量重定位数据目录；当前走的是「搬目录 + 建联接」（对应用透明、更通用）。",
    );
  }
  return {
    agentId: a.id,
    agentName: a.name,
    src: a.paths[0].resolved,
    dest,
    snapshotPrefix: `${a.paths[0].resolved}.moved-`,
    srcMeasure: a.total,
    destFree: 288.4 * GIB,
    destExisting: m(0, 0),
    blockers,
    warnings,
    fits: true,
  };
}

export const SNAPSHOTS: Snapshot[] = [
  {
    path: "C:\\Users\\29436\\.workbuddy-ai.moved-20261001-165033",
    name: ".workbuddy-ai.moved-20261001-165033",
    agentId: "workbuddy-intl",
    created: "20261001-165033",
    measure: m(3.42 * GIB, 44_120),
  },
];

/** 演示用的迁移进度序列。 */
export const PROGRESS_SEQ: Progress[] = [
  { step: "copying", detail: "正在复制到 E:\\AgentCache\\codex", percent: 5 },
  { step: "copying", detail: "复制完成（只补不删）", percent: 68 },
  { step: "verifying", detail: "正在核对差异", percent: 72 },
  { step: "verifying", detail: "源里已无待复制文件（0 差异）", percent: 78 },
  { step: "snapshotting", detail: "原目录改名保留为 .codex.moved-20261008-223000", percent: 84 },
  { step: "linking", detail: "C:\\Users\\29436\\.codex → E:\\AgentCache\\codex", percent: 92 },
  { step: "verifying-link", detail: "直接读原路径，看到 12 个条目", percent: 98 },
  { step: "done", detail: "迁移完成", percent: 100 },
];

export const REPORT: MigrateReport = {
  ok: true,
  src: "C:\\Users\\29436\\.codex",
  dest: "E:\\AgentCache\\codex",
  snapshot: "C:\\Users\\29436\\.codex.moved-20261008-223000",
  steps: [
    { name: "复制", ok: true, detail: "复制完成（只补不删）" },
    { name: "校验", ok: true, detail: "源里已无待复制文件（0 差异）" },
    { name: "快照", ok: true, detail: "原目录改名保留" },
    { name: "建联接", ok: true, detail: "C:\\Users\\29436\\.codex → E:\\AgentCache\\codex" },
    { name: "验证联接", ok: true, detail: "直接读原路径，看到 12 个条目" },
  ],
  error: null,
  rolledBack: false,
};
