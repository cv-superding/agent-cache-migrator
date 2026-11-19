/**
 * 轻量 i18n —— 刻意不引 i18next 这类库：字典就百来条，一个文件说清楚，
 * 还能少一堆依赖。中文默认；检测系统语言是英文时自动切英文，可在侧栏手动切换。
 *
 * 覆盖范围（v0.9）：界面操作层（侧栏 / 卡片标题 / 主按钮 / 确认框 / 提示徽章 / 空态）。
 * 长说明性文案（风险解释等）暂保持中文 —— 它们和 README 联动，双语维护成本高。
 */

import { useSyncExternalStore } from "react";

export type Lang = "zh" | "en";

const STORE_KEY = "acm-lang";

function detect(): Lang {
  try {
    const saved = localStorage.getItem(STORE_KEY);
    if (saved === "zh" || saved === "en") return saved;
    if (navigator.language?.toLowerCase().startsWith("en")) return "en";
  } catch {
    /* localStorage 不可用（如 file:// 某些环境）—— 用默认 */
  }
  return "zh";
}

let lang: Lang = detect();
const subs = new Set<() => void>();

/** 切换语言并持久化。 */
export function setLang(l: Lang): void {
  lang = l;
  try {
    localStorage.setItem(STORE_KEY, l);
  } catch {
    /* 同上 */
  }
  subs.forEach((f) => f());
}

/** 当前语言（响应式：语言切换时组件会重渲染）。 */
export function useLang(): Lang {
  return useSyncExternalStore(
    (cb) => {
      subs.add(cb);
      return () => subs.delete(cb);
    },
    () => lang,
  );
}

const zh = {
  // 侧栏
  brandSub: "Agent 缓存迁移器",
  detectedCount: "检测到 {n} 个 Agent",
  detectedSummary: "已安装 {n} 项 · 共 {total} 项",
  groupAgents: "Agent（{n}）",
  groupCaches: "缓存目录（{n}）",
  groupOrphans: "已卸载 · 只剩缓存（{n}）",
  cacheTag: "缓存",
  noneDetected: "没有检测到任何 Agent。",
  notInstalled: "未安装（{n}）",
  rescan: "重新扫描",
  scanning: "扫描中…",
  runningTip: "运行中：{list}",
  // 头部
  pickAgent: "选择一个 Agent",
  pickHint: "左边选要迁移的 Agent，右边选目标盘",
  runningBadge: "运行中：{list}",
  dirsDetected: "检测到 {n} 个数据目录",
  selectHint: "左边选要迁移的 Agent，右边选目标盘",
  empty: "没有检测到任何 Agent。",
  // 状态徽章
  stateMoved: "已迁移",
  stateFree: "未迁移",
  stateAbsent: "未检测到",
  stateOrphan: "已卸载",
  orphanNote:
    "这个应用已经不在本机了（卸载清单里没有它），这里只剩它留下的数据。点下面的「清理缓存」把它移入回收站，再扫一次就彻底干净了。",
  orphanSub: "本机已卸载，只剩数据",
  // 状态卡片
  statusTitle: "当前状态",
  statusSub: "只读检测，不改任何东西",
  rowPaths: "数据目录",
  noPaths: "未检测到（应用可能没装）",
  linkBroken: "联接已断",
  linkOk: "联接",
  realDir: "真实目录",
  rowTarget: "实际指向",
  rowSize: "占用",
  statFiles: "文件",
  statDirs: "目录",
  filesUnit: " 个文件",
  sizeCapped: "（到上限就停了，是下限）",
  sizeExact: "（已精确统计）",
  measureExact: "精确统计",
  measuring: "统计中…",
  measureTip: "列表里的数字是「扫到 25 万文件 / 20 秒就停」的下限；这里不设上限，慢慢算准数（大目录可能要几分钟）",
  pathMove: "迁移它",
  pathStay: "留在原地（只有配置）",
  toastMeasured: "精确统计完成",
  toastMeasureFailed: "统计失败",
  rowUpdater: "更新器",
  rowRunning: "运行状态",
  notRunning: "未运行",
  runningState: "运行中：{list}",
  rowNote: "提示",
  // 清理缓存（卸载残留）
  purgeTitle: "清理缓存？",
  purgeOk: "移入回收站",
  purgeBtn: "清理缓存",
  purging: "清理中…",
  purgeHint: "数据目录整体移入回收站 —— 卸载残留清完就变「未安装」",
  purgeWarn:
    "会把这个 Agent 的数据目录整体移入系统回收站：登录状态、聊天记录、配置都会没。空间要清空回收站才真正释放；已迁移（联接）的目录会自动跳过（先回滚再清）。",
  purgeSkipTag: "跳过",
  purgeNothing: "没有可清理的目录（可能已经清过了）",
  purgeRunning: "应用还在运行 —— 先完全退出（含托盘图标）再清理",
  purgeTotal: "合计 {size}",
  toastPurged: "已清理缓存",
  toastPurgeFailed: "清理失败",
  toastHistoryCleared: "迁移历史已清空",
  toastHistoryClearedSub: "删掉 {n} 条记录",
  toastClearFailed: "清空失败",
  movedNote: "已经迁移过了 —— 数据在目标盘，应用通过联接无感读写。设置页里显示的仍是原路径，这是正常的（联接对应用透明）。",
  brokenCount: "检测到 {n} 个断链联接 —— 应用「解压到临时名再改名」的准备工作会永久失败，每次启动都重做一遍。如果数据还在目标位置，可以一键重建。",
  repairBtn: "修复链接（不动数据）",
  repairing: "修复中…",
  absentNote: "没有检测到这个 Agent 的数据目录。如果确实装了，可能是装在自定义位置 —— 可以在 %APPDATA%\\AgentCache\\agents.toml 里补一条。",
  envNote: "该应用官方支持用环境变量重定位数据目录 —— 比联接更干净（原路径不留任何东西）。",
  envBtn: "用环境变量重定位",
  envSetting: "设置中…",
  // 确认框
  repairTitle: "修复链接？",
  repairOk: "修复",
  envTitle: "用环境变量重定位？",
  envOk: "设置",
  cancel: "取消",
  // 目标盘卡片
  destTitle: "迁移到",
  destSub: "左边选盘，右边确认路径",
  readingDisks: "正在读取磁盘…",
  rowDestPath: "目标路径",
  driveFree: "可用",
  driveTotal: "共 {n}",
  useDefault: "用默认",
  open: "打开",
  rowFinal: "最终落点",
  planning: "正在生成迁移计划…",
  planFailed: "生成计划失败：",
  // 迁移卡片
  runTitle: "执行",
  runSub: "进度实时展示，任何一步失败自动回滚",
  startMigrate: "开始迁移",
  migrateTitle: "开始迁移？",
  runningStateBadge: "迁移中…",
  // 历史卡片
  historyTitle: "迁移历史",
  historySub: "何时对哪个目录做过什么（含失败的）",
  searchPh: "搜索 agent / 路径 / 错误…",
  filterAll: "全部",
  filterMigrate: "迁移",
  filterEnv: "环境变量",
  filterFailed: "失败",
  noRecords: "还没有记录 —— 每次迁移（含失败的）都会自动记到这里。",
  noMatch: "没有匹配「{q}」的记录。",
  badgeEnv: "环境变量",
  ok: "成功",
  fail: "失败",
  autoRolledBack: "（已自动回滚）",
  viewAll: "查看全部",
  recent15: "只看最近 15 条",
  clearHistory: "清空历史",
  clearHistoryTitle: "清空迁移历史？",
  // 快照卡片
  snapshotsTitle: "快照清理",
  snapshotsSub: "迁移后保留的原目录 —— 回滚靠它，确认无误再删",
  // env 说明（迁移区）
  envRelocateNote: "这个 Agent 官方支持用环境变量重定位",
  withEnvLabel: "迁移成功后同时设置该环境变量（联接 + 变量双保险，重启应用生效）",
  // toast 标题
  toastMigrated: "迁移完成",
  toastMigrateFailed: "迁移失败（已自动回滚）",
  toastMigrateErr: "迁移失败",
  toastMigrateAbnormal: "迁移异常",
  toastRolledBack: "已回滚",
  toastRollbackFailed: "回滚失败",
  toastRepairDone: "修复完成",
  toastRepairFailed: "修复失败",
  toastEnvSet: "已设置环境变量",
  toastEnvFailed: "设置失败",
  toastDeleted: "已永久删除快照",
  toastTrashed: "已移入回收站",
  toastDeleteFailed: "删除失败",
  toastCantOpen: "打不开",
  // 确认框正文（迁移）
  confirmOrder: "执行顺序：复制 → 校验 0 差异 → 原目录改名保留 → 建目录联接 → 穿过联接验证。任何一步失败会自动回滚。原目录不会删除，只是改名成快照。",
  // 确认框正文（删除快照）
  deleteTitle: "删除这个快照？",
  deleteOk: "永久删除",
  trashExtra: "移入回收站",
  deleteWarn: "永久删除后这次迁移就不能回滚了，且不可恢复；「移入回收站」可从系统回收站还原（仅 Windows）。确认迁移后一切都正常再删。",
  // 确认框正文（回滚 / 修复 / env）
  rollbackTitle: "回滚？",
  rollbackOk: "回滚",
  // 类别提示（classHint）
  classSafe: "纯缓存，可放心迁移",
  classSpecial: "特殊：建议走官方流程",
  classSystem: "系统目录：不建议迁移",
  classNormal: "应用数据：迁移前需完全退出",
} as const;

export type DictKey = keyof typeof zh;

const en: Record<DictKey, string> = {
  brandSub: "Agent cache migrator",
  detectedCount: "{n} agents detected",
  detectedSummary: "{n} of {total} installed",
  groupAgents: "Agents ({n})",
  groupCaches: "Cache dirs ({n})",
  groupOrphans: "Uninstalled · leftovers ({n})",
  cacheTag: "Cache",
  noneDetected: "No agents detected.",
  notInstalled: "Not installed ({n})",
  rescan: "Rescan",
  scanning: "Scanning…",
  runningTip: "Running: {list}",
  pickAgent: "Pick an agent",
  pickHint: "Pick an agent on the left, a target drive on the right",
  runningBadge: "Running: {list}",
  dirsDetected: "{n} data dirs detected",
  selectHint: "Pick an agent on the left, a target drive on the right",
  empty: "No agents detected.",
  stateMoved: "Migrated",
  stateFree: "Not migrated",
  stateAbsent: "Not found",
  stateOrphan: "Uninstalled",
  orphanNote:
    "This app is no longer installed (there is no uninstall entry for it) — only its data is left. Hit \"Purge cache\" below to move it to the Recycle Bin; one rescan and it is gone for good.",
  orphanSub: "Uninstalled — data left",
  statusTitle: "Current state",
  statusSub: "Read-only detection, changes nothing",
  rowPaths: "Data dirs",
  noPaths: "Not found (app may not be installed)",
  linkBroken: "Link broken",
  linkOk: "Linked",
  realDir: "Real dir",
  rowTarget: "Points to",
  rowSize: "Size",
  statFiles: "Files",
  statDirs: "Dirs",
  filesUnit: " files",
  sizeCapped: " (hit the cap, lower bound)",
  sizeExact: " (exact)",
  measureExact: "Measure exactly",
  measuring: "Measuring…",
  measureTip: "The listed number is a lower bound (scan stops at 250k files / 20s). This one has no cap — big dirs can take minutes",
  pathMove: "Moves this one",
  pathStay: "Stays (config only)",
  toastMeasured: "Exact size measured",
  toastMeasureFailed: "Measure failed",
  rowUpdater: "Updaters",
  rowRunning: "Running",
  notRunning: "Not running",
  runningState: "Running: {list}",
  rowNote: "Note",
  purgeTitle: "Purge the cache?",
  purgeOk: "Move to Recycle Bin",
  purgeBtn: "Purge cache",
  purging: "Purging…",
  purgeHint: "Moves the whole data dir to the Recycle Bin — leftovers gone, agent drops to \"not installed\"",
  purgeWarn:
    "This moves the agent's data dir to the system Recycle Bin: sign-in state, chat history and settings are all gone. Space is only freed once the bin is emptied. Migrated (junction) dirs are skipped automatically — roll back first if you want those gone.",
  purgeSkipTag: "skipped",
  purgeNothing: "Nothing to purge (maybe already cleaned)",
  purgeRunning: "The app is still running — quit it fully (tray icon too) first",
  purgeTotal: "{size} total",
  toastPurged: "Cache purged",
  toastPurgeFailed: "Purge failed",
  toastHistoryCleared: "History cleared",
  toastHistoryClearedSub: "{n} records removed",
  toastClearFailed: "Clear failed",
  movedNote: "Already migrated — data lives on the target drive; the app reads/writes through the link. The original path shown in system settings is normal (the link is transparent to the app).",
  brokenCount: "{n} broken link(s) found — the app's \"extract to temp then rename\" step will fail forever and redo on every launch. If the data is still on the target, rebuild with one click.",
  repairBtn: "Repair link (no data copied)",
  repairing: "Repairing…",
  absentNote: "No data dir detected for this agent. If it is installed, it may live at a custom path — add an entry in %APPDATA%\\AgentCache\\agents.toml.",
  envNote: "This app officially supports relocating its data dir via an env var — cleaner than a link (nothing left at the original path).",
  envBtn: "Relocate via env var",
  envSetting: "Setting…",
  repairTitle: "Repair the link?",
  repairOk: "Repair",
  envTitle: "Relocate via env var?",
  envOk: "Set",
  cancel: "Cancel",
  destTitle: "Migrate to",
  destSub: "Pick a drive on the left, confirm the path",
  readingDisks: "Reading disks…",
  rowDestPath: "Target path",
  driveFree: "free",
  driveTotal: "of {n}",
  useDefault: "Default",
  open: "Open",
  rowFinal: "Final location",
  planning: "Planning migration…",
  planFailed: "Plan failed: ",
  runTitle: "Execute",
  runSub: "Live progress; any failed step rolls back automatically",
  startMigrate: "Start migration",
  migrateTitle: "Start migration?",
  runningStateBadge: "Migrating…",
  historyTitle: "History",
  historySub: "When, on which dir, and how it went (failures included)",
  searchPh: "Search agent / path / error…",
  filterAll: "All",
  filterMigrate: "Migrations",
  filterEnv: "Env vars",
  filterFailed: "Failed",
  noRecords: "No records yet — every migration (failures included) lands here automatically.",
  noMatch: "No records matching \"{q}\".",
  badgeEnv: "Env var",
  ok: "OK",
  fail: "Failed",
  autoRolledBack: " (auto-rolled back)",
  viewAll: "Show all",
  recent15: "Recent 15 only",
  clearHistory: "Clear history",
  clearHistoryTitle: "Clear the migration history?",
  snapshotsTitle: "Snapshots",
  snapshotsSub: "Original dirs kept after migration — rollback needs them; delete only when sure",
  envRelocateNote: "This agent officially supports relocating via env var",
  withEnvLabel: "Also set the env var after a successful migration (link + var, both work; effective after app restart)",
  toastMigrated: "Migration complete",
  toastMigrateFailed: "Migration failed (auto-rolled back)",
  toastMigrateErr: "Migration failed",
  toastMigrateAbnormal: "Migration error",
  toastRolledBack: "Rolled back",
  toastRollbackFailed: "Rollback failed",
  toastRepairDone: "Repair complete",
  toastRepairFailed: "Repair failed",
  toastEnvSet: "Env var set",
  toastEnvFailed: "Failed to set",
  toastDeleted: "Snapshot permanently deleted",
  toastTrashed: "Moved to Recycle Bin",
  toastDeleteFailed: "Delete failed",
  toastCantOpen: "Can't open",
  confirmOrder: "Order: copy → verify 0 diff → rename original to snapshot → create link → verify through link. Any failed step rolls back automatically. The original dir is never deleted, only renamed to a snapshot.",
  deleteTitle: "Delete this snapshot?",
  deleteOk: "Delete forever",
  trashExtra: "Move to Recycle Bin",
  deleteWarn: "After permanent deletion this migration can no longer be rolled back, and it is unrecoverable; \"Move to Recycle Bin\" can be restored from the system bin (Windows only). Delete only after everything checks out.",
  rollbackTitle: "Roll back?",
  rollbackOk: "Roll back",
  classSafe: "Pure cache — safe to migrate",
  classSpecial: "Special: prefer the official flow",
  classSystem: "System dir: migration not recommended",
  classNormal: "App data: quit the app fully before migrating",
};

const dicts: Record<Lang, Record<DictKey, string>> = { zh, en };

/** 取文案；支持 `{n}`/`{list}`/`{q}` 这类占位符。 */
export function t(key: DictKey, vars?: Record<string, string | number>): string {
  let s: string = dicts[lang][key] ?? zh[key] ?? key;
  if (vars) {
    for (const [k, v] of Object.entries(vars)) {
      s = s.replaceAll(`{${k}}`, String(v));
    }
  }
  return s;
}

/** 语言切换按钮（放在侧栏底部）。 */
export function LangSwitch() {
  const cur = useLang();
  return (
    <button
      className="btn sm"
      title="Switch language / 切换语言"
      onClick={() => setLang(cur === "zh" ? "en" : "zh")}
    >
      {cur === "zh" ? "EN" : "中"}
    </button>
  );
}

/** AgentState 的双语标签（types.ts 里的 stateLabel 是中文单语）。 */
export function stateLabelL(s: string): string {
  if (s === "moved") return t("stateMoved");
  if (s === "free") return t("stateFree");
  if (s === "orphan") return t("stateOrphan");
  return t("stateAbsent");
}

/** class 的双语提示。 */
export function classHintL(c: string): string {
  switch (c) {
    case "safe":
      return t("classSafe");
    case "special":
      return t("classSpecial");
    case "system":
      return t("classSystem");
    default:
      return t("classNormal");
  }
}
