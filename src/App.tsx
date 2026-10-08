import { useEffect, useMemo, useRef, useState } from "react";
import type { ReactNode } from "react";
import { api } from "./api";
import type {
  DetectedAgent,
  DriveInfo,
  DrivesPayload,
  MigrateReport,
  Plan,
  Progress,
  Snapshot,
} from "./types";
import { agentState, human, stateLabel } from "./types";
import { Card, Confirm, Sidebar, Toast, type ToastItem } from "./components";

/** 各 Agent 的图标色 —— 纯展示，取不到的走默认灰。 */
const COLORS: Record<string, string> = {
  "workbuddy-cn": "#0f766e",
  "workbuddy-intl": "#0369a1",
  codex: "#111827",
  claude: "#b45309",
  cursor: "#4f46e5",
  "codebuddy-cn": "#0f766e",
  zcode: "#7c3aed",
  deepseek: "#1d4ed8",
  cherry: "#db2777",
  trae: "#ea580c",
  doubao: "#2563eb",
  utools: "#334155",
  "npm-cache": "#c2410c",
};

/** 迁移步骤的展示顺序（与 Rust `Progress.step` 对应）。 */
const STEP_ORDER = ["copying", "verifying", "snapshotting", "linking", "verifying-link"];
const STEP_LABEL: Record<string, string> = {
  copying: "复制",
  verifying: "校验",
  snapshotting: "快照",
  linking: "建联接",
  "verifying-link": "验证联接",
  done: "完成",
};

export default function App() {
  // ---- 数据
  const [agents, setAgents] = useState<DetectedAgent[]>([]);
  const [drives, setDrives] = useState<DrivesPayload | null>(null);
  const [snaps, setSnaps] = useState<Snapshot[]>([]);

  // ---- 选择
  const [sel, setSel] = useState<string | null>(null);
  const [destRoot, setDestRoot] = useState("");

  // ---- 计划 / 执行
  const [plan, setPlan] = useState<Plan | null>(null);
  const [planErr, setPlanErr] = useState<string | null>(null);
  const [planning, setPlanning] = useState(false);
  const [running, setRunning] = useState(false);
  const [progress, setProgress] = useState<Progress | null>(null);
  const [report, setReport] = useState<MigrateReport | null>(null);
  const [log, setLog] = useState<string[]>([]);
  const [rolling, setRolling] = useState(false);

  // ---- 界面
  const [scanning, setScanning] = useState(true);
  const [toasts, setToasts] = useState<ToastItem[]>([]);
  const [confirm, setConfirm] = useState<{
    title: string;
    body: ReactNode;
    okText: string;
    danger?: boolean;
    onOk: () => void;
  } | null>(null);
  const [ver, setVer] = useState<{ app: string; core: string } | null>(null);

  const toastSeq = useRef(0);
  function toast(msg: string, detail?: string, err = false) {
    const id = ++toastSeq.current;
    setToasts((t) => [...t, { id, msg, detail, err }]);
    setTimeout(() => setToasts((t) => t.filter((x) => x.id !== id)), 4200);
  }

  const cur = useMemo(() => agents.find((a) => a.id === sel) ?? null, [agents, sel]);

  // ---------------------------------------------------------------- 首屏

  useEffect(() => {
    void api.version().then(setVer).catch(() => {});
    void api.drives().then((d) => {
      setDrives(d);
      if (d.defaultDestRoot) setDestRoot((v) => v || d.defaultDestRoot!);
    });
    void refreshSnapshots();
    void refreshAgents(true);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  /** 先拿「不量体积」的快速结果渲染首屏，再后台补体积。 */
  async function refreshAgents(twoPhase: boolean) {
    setScanning(true);
    try {
      const fast = await api.detect(false);
      setAgents(fast);
      // `?agent=<id>` 只在演示/调样式时用（Tauri 里没有 query，不影响正常路径）
      const q = new URLSearchParams(window.location.search).get("agent");
      setSel((s) => s ?? q ?? fast.find((a) => a.detected)?.id ?? fast[0]?.id ?? null);
      if (twoPhase) {
        const full = await api.detect(true);
        setAgents(full);
      }
    } catch (e) {
      toast("检测失败", String(e), true);
    } finally {
      setScanning(false);
    }
  }

  async function refreshSnapshots() {
    try {
      setSnaps(await api.snapshots());
    } catch {
      /* 快照读不到不影响主流程 */
    }
  }

  async function refreshDrives() {
    try {
      const d = await api.drives();
      setDrives(d);
    } catch {
      /* 忽略 */
    }
  }

  // ---------------------------------------------------------------- 计划

  useEffect(() => {
    if (!sel || !destRoot) {
      setPlan(null);
      return;
    }
    let alive = true;
    setPlanning(true);
    setPlanErr(null);
    api
      .plan(sel, destRoot)
      .then((p) => alive && setPlan(p))
      .catch((e) => {
        if (!alive) return;
        setPlan(null);
        setPlanErr(String(e));
      })
      .finally(() => alive && setPlanning(false));
    return () => {
      alive = false;
    };
  }, [sel, destRoot, agents]);

  // ---------------------------------------------------------------- 迁移

  async function doMigrate() {
    if (!sel || !destRoot) return;
    setRunning(true);
    setReport(null);
    setProgress(null);
    setLog([]);

    const un = await api.onProgress((p) => {
      setProgress(p);
      setLog((l) => [...l.slice(-200), `${p.step.padEnd(14)} ${p.detail}`]);
    });

    try {
      const r = await api.migrateAgent(sel, destRoot);
      setReport(r);
      if (r.ok) {
        toast("迁移完成", `${r.src} → ${r.dest}`);
      } else if (r.rolledBack) {
        toast("迁移失败（已自动回滚）", r.error ?? "", true);
      } else {
        toast("迁移失败", r.error ?? "", true);
      }
    } catch (e) {
      toast("迁移异常", String(e), true);
    } finally {
      un();
      setRunning(false);
      setProgress(null);
      await refreshAgents(false);
      await refreshSnapshots();
      await refreshDrives();
    }
  }

  async function doRollback(id: string) {
    setRolling(true);
    try {
      const msg = await api.rollback(id);
      toast("已回滚", msg);
      await refreshAgents(false);
      await refreshSnapshots();
    } catch (e) {
      toast("回滚失败", String(e), true);
    } finally {
      setRolling(false);
    }
  }

  async function doDeleteSnapshot(path: string) {
    try {
      const r = await api.deleteSnapshot(path, true);
      toast("已删除快照", `回收 ${r.freedText}`);
      await refreshSnapshots();
      await refreshDrives();
    } catch (e) {
      toast("删除失败", String(e), true);
    }
  }

  // ---------------------------------------------------------------- 渲染

  const detected = agents.filter((a) => a.detected);

  return (
    <div className="app">
      <Sidebar
        agents={agents}
        colors={COLORS}
        selected={sel}
        scanning={scanning}
        onPick={(id) => {
          setSel(id);
          setReport(null);
          setLog([]);
          setProgress(null);
        }}
        onRescan={() => {
          void refreshAgents(true);
          void refreshSnapshots();
        }}
        version={ver?.app}
      />

      <div className="main">
        <header className="head">
          <h1>
            {cur ? cur.name : "选择一个 Agent"}
            {cur && (
              <>
                <span className={`badge ${badgeOf(agentState(cur))}`}>{stateLabel(agentState(cur))}</span>
                {cur.running.length > 0 && (
                  <span className="badge b-warn">运行中：{cur.running.join(", ")}</span>
                )}
              </>
            )}
          </h1>
          <p>
            {cur
              ? `${classHint(cur.class)} · ${cur.kind} · 检测到 ${cur.paths.filter((p) => p.kind !== "missing").length} 个数据目录`
              : "左边选要迁移的 Agent，右边选目标盘"}
          </p>
        </header>

        <div className="body">
          {!cur && <div className="empty">没有检测到任何 Agent。</div>}

          {cur && (
            <>
              <StatusCard a={cur} />
              <DestCard
                drives={drives}
                destRoot={destRoot}
                onDestRoot={setDestRoot}
                plan={plan}
                planErr={planErr}
                planning={planning}
                onOpen={(p) => void api.openPath(p).catch((e) => toast("打不开", String(e), true))}
              />
              <RunCard
                plan={plan}
                running={running}
                progress={progress}
                report={report}
                log={log}
                onRun={() => {
                  if (!plan || plan.blockers.length) return;
                  setConfirm({
                    title: "开始迁移？",
                    danger: false,
                    okText: "开始迁移",
                    body: (
                      <>
                        <div className="rows">
                          <Row k="源" v={<span className="mono">{plan.src}</span>} />
                          <Row k="目标" v={<span className="mono">{plan.dest}</span>} />
                          <Row k="占用" v={human(plan.srcMeasure.bytes)} />
                          <Row k="快照" v={<span className="mono">{plan.snapshotPrefix}&lt;时间戳&gt;</span>} />
                        </div>
                        <div className="note info" style={{ marginTop: 10 }}>
                          执行顺序：复制 → <b>校验 0 差异</b> → 原目录改名保留 → 建目录联接 → 穿过联接验证。
                          任何一步失败会自动回滚。<b>原目录不会删除</b>，只是改名成快照。
                        </div>
                      </>
                    ),
                    onOk: () => void doMigrate(),
                  });
                }}
              />

              {cur.envRelocate && (
                <div className="note info">
                  这个 Agent 官方支持用环境变量重定位：<code>{cur.envRelocate.env}</code>
                  {cur.envRelocate.note ? ` —— ${cur.envRelocate.note}` : ""}。
                  本工具默认走「搬目录 + 建联接」（对应用透明、更通用）。
                </div>
              )}

              {cur.brokenLinks.length > 0 && (
                <div className="note danger">
                  <b>发现 {cur.brokenLinks.length} 个断链联接</b> —— 目标已经不存在了。
                  这类残留会让应用每次启动都重新解压运行时（表现为反复「正在准备环境」）。建议手工摘除：
                  <ul style={{ margin: "6px 0 0", paddingLeft: 18 }}>
                    {cur.brokenLinks.map((b) => (
                      <li key={b.path} className="mono">
                        {b.path} → {b.target ?? "?"}
                      </li>
                    ))}
                  </ul>
                </div>
              )}
            </>
          )}

          <SnapshotCard
            snaps={snaps}
            rolling={rolling}
            onRollback={(id) => {
              const a = agents.find((x) => x.id === id);
              setConfirm({
                title: "回滚这次迁移？",
                okText: "回滚",
                body: (
                  <>
                    会把 <span className="mono">{a?.name ?? id}</span> 的联接摘掉、快照改回原名。
                    只有在应用里确认数据异常时才需要这么做。
                  </>
                ),
                onOk: () => void doRollback(id),
              });
            }}
            onDelete={(s) =>
              setConfirm({
                title: "永久删除这个快照？",
                okText: "永久删除",
                danger: true,
                body: (
                  <>
                    <div className="rows">
                      <Row k="快照" v={<span className="mono">{s.path}</span>} />
                      <Row k="占用" v={`${human(s.measure.bytes)} · ${s.measure.files} 文件`} />
                    </div>
                    <div className="note danger" style={{ marginTop: 10 }}>
                      删掉之后<b>这次迁移就不能回滚了</b>，而且不可恢复。确认迁移后一切都正常再删。
                    </div>
                  </>
                ),
                onOk: () => void doDeleteSnapshot(s.path),
              })
            }
          />
        </div>

        {/* 状态栏固定在底部（放在 .body 里面会跟着内容一起滚走） */}
        <div className="foot">
          <span className="sp" />
          <span>
            共 {agents.length} 项：检测到 {detected.length}，已迁移{" "}
            {agents.filter((a) => agentState(a) === "moved").length}，运行中{" "}
            {agents.filter((a) => a.running.length > 0).length}
          </span>
          {api.DEMO && <span className="badge b-warn">演示模式</span>}
          {ver && (
            <span className="mono">
              v{ver.app} · core {ver.core}
            </span>
          )}
        </div>
      </div>

      {confirm && (
        <Confirm
          title={confirm.title}
          okText={confirm.okText}
          danger={confirm.danger}
          onCancel={() => setConfirm(null)}
          onOk={() => {
            const f = confirm.onOk;
            setConfirm(null);
            f();
          }}
        >
          {confirm.body}
        </Confirm>
      )}

      <Toast items={toasts} />
    </div>
  );
}

// ---------------------------------------------------------------- 子卡片

function Row({ k, v }: { k: string; v: ReactNode }) {
  return (
    <div className="row">
      <div className="k">{k}</div>
      <div className="v">{v}</div>
    </div>
  );
}

function badgeOf(s: ReturnType<typeof agentState>): string {
  return s === "moved" ? "b-ok" : s === "free" ? "b-gray" : "b-gray";
}

function classHint(c: string): string {
  switch (c) {
    case "safe":
      return "纯缓存，可放心迁移";
    case "special":
      return "特殊：建议走官方流程";
    case "system":
      return "系统目录：不建议迁移";
    default:
      return "应用数据：迁移前需完全退出";
  }
}

/** 当前状态卡片。 */
function StatusCard({ a }: { a: DetectedAgent }) {
  const st = agentState(a);
  const shown = a.paths.filter((p) => p.kind !== "missing");

  return (
    <Card title="当前状态" sub="只读检测，不改任何东西">
      <div className="rows">
        <Row
          k="数据目录"
          v={
            shown.length === 0 ? (
              <span style={{ color: "var(--muted)" }}>未检测到（应用可能没装）</span>
            ) : (
              shown.map((p) => (
                <div key={p.resolved} style={{ marginBottom: 3 }}>
                  <span className="mono">{p.resolved}</span>{" "}
                  {p.isLink ? (
                    p.linkBroken ? (
                      <span className="badge b-danger">联接已断</span>
                    ) : (
                      <span className="badge b-ok">联接</span>
                    )
                  ) : (
                    <span className="badge b-gray">真实目录</span>
                  )}
                </div>
              ))
            )
          }
        />
        {shown.some((p) => p.linkTarget) && (
          <Row
            k="实际指向"
            v={
              <span className="mono">
                {shown.map((p) => p.linkTarget).filter(Boolean).join("、")}
              </span>
            }
          />
        )}
        <Row
          k="占用"
          v={
            a.total.files === 0 && !a.detected ? (
              "—"
            ) : (
              <>
                {a.total.capped ? "≥" : ""}
                {human(a.total.bytes)} · {a.total.files.toLocaleString()} 个文件
                {a.total.capped && (
                  <span style={{ color: "var(--muted)" }}>（到上限就停了，是下限）</span>
                )}
              </>
            )
          }
        />
        {a.updaters.length > 0 && (
          <Row k="更新器" v={<span className="mono">{a.updaters.join("、")}</span>} />
        )}
        <Row
          k="运行状态"
          v={
            a.running.length > 0 ? (
              <span className="badge b-warn">运行中：{a.running.join(", ")}</span>
            ) : (
              <span className="badge b-ok">未运行</span>
            )
          }
        />
        {a.note && <Row k="提示" v={a.note} />}
      </div>

      {st === "moved" && (
        <div className="note ok" style={{ marginTop: 11 }}>
          已经迁移过了 —— 数据在目标盘，应用通过联接无感读写。
          <b>设置页里显示的仍是原路径，这是正常的</b>（联接对应用透明）。
        </div>
      )}
      {st === "absent" && (
        <div className="note info" style={{ marginTop: 11 }}>
          没有检测到这个 Agent 的数据目录。如果确实装了，可能是装在自定义位置 ——
          可以在 <code>%APPDATA%\AgentCache\agents.toml</code> 里补一条。
        </div>
      )}
    </Card>
  );
}

/** 目标盘 + 路径卡片。 */
function DestCard(props: {
  drives: DrivesPayload | null;
  destRoot: string;
  onDestRoot: (v: string) => void;
  plan: Plan | null;
  planErr: string | null;
  planning: boolean;
  onOpen: (p: string) => void;
}) {
  const { drives, destRoot, onDestRoot, plan, planErr, planning, onOpen } = props;

  return (
    <Card title="迁移到" sub="左边选盘，右边确认路径">
      {!drives ? (
        <div className="empty">
          <span className="spin" />
          正在读取磁盘…
        </div>
      ) : (
        <>
          <div className="drives">
            {drives.drives.map((d) => (
              <DriveBtn
                key={d.letter}
                d={d}
                on={isSelected(d, destRoot)}
                onPick={() => onDestRoot(deriveRootOf(d, destRoot))}
              />
            ))}
          </div>

          <div style={{ marginTop: 13 }}>
            <Row
              k="目标路径"
              v={
                <div className="field">
                  <input
                    type="text"
                    value={destRoot}
                    onChange={(e) => onDestRoot(e.target.value)}
                    placeholder="例如 E:\AgentCache"
                  />
                  <button
                    className="btn sm"
                    onClick={() => drives.defaultDestRoot && onDestRoot(drives.defaultDestRoot)}
                  >
                    用默认
                  </button>
                  <button className="btn sm" onClick={() => destRoot && onOpen(destRoot)}>
                    打开
                  </button>
                </div>
              }
            />
            <Row
              k="最终落点"
              v={
                <span className="mono">
                  {plan ? plan.dest : destRoot ? `${destRoot}\\<agent-id>` : "—"}
                </span>
              }
            />
          </div>

          {planning && (
            <div className="note info" style={{ marginTop: 11 }}>
              <span className="spin" />
              正在生成迁移计划…
            </div>
          )}

          {planErr && (
            <div className="note warn" style={{ marginTop: 11 }}>
              生成计划失败：{planErr}
            </div>
          )}

          {plan && !planning && (
            <>
              {plan.blockers.length === 0 ? (
                <div className="note ok" style={{ marginTop: 11 }}>
                  <b>可以迁移。</b>占用 {human(plan.srcMeasure.bytes)}，目标盘可用{" "}
                  {human(plan.destFree)}。
                </div>
              ) : (
                <div className="note danger" style={{ marginTop: 11 }}>
                  <b>还不能迁移 —— 先处理这些：</b>
                  <ul style={{ margin: "6px 0 0", paddingLeft: 18 }}>
                    {plan.blockers.map((b) => (
                      <li key={b}>{b}</li>
                    ))}
                  </ul>
                  <div style={{ marginTop: 6, opacity: 0.85 }}>
                    窗口关掉不算退出 —— 托盘图标右键退出，或任务管理器里结束进程。
                  </div>
                </div>
              )}

              {plan.warnings.length > 0 && (
                <div className="note warn" style={{ marginTop: 9 }}>
                  <ul style={{ margin: 0, paddingLeft: 18 }}>
                    {plan.warnings.map((w) => (
                      <li key={w}>{w}</li>
                    ))}
                  </ul>
                </div>
              )}
            </>
          )}
        </>
      )}
    </Card>
  );
}

/** 判断某个位置是不是当前选中的目标。
 *
 * Windows 比盘符（`E://`）；unix 比挂载点 —— 而且要用**前缀**比，
 * 因为 `/mnt/data` 和 `/` 会同时存在。
 */
function isSelected(d: DriveInfo, destRoot: string): boolean {
  if (!destRoot) return false;
  if (d.letter.includes("\\")) {
    return destRoot.slice(0, 2).toUpperCase() === d.letter.slice(0, 2).toUpperCase();
  }
  const m = d.letter.replace(/[\\/]+$/, "");
  return destRoot === m || destRoot.startsWith(m + "/");
}

function deriveRootOf(d: DriveInfo, cur: string): string {
  // 换位置时把原来的最后一段（一般就是 `AgentCache`）带过去，省得用户重打。
  // 分隔符按目标位置自己判断 —— Windows 是 `E:\`，unix 是 `/mnt/data`。
  const leaf = cur.split(/[\\/]/).filter(Boolean).pop() ?? "AgentCache";
  const sep = d.letter.includes("\\") ? "\\" : "/";
  return d.letter.replace(/[\\/]+$/, "") + sep + leaf;
}

function DriveBtn({ d, on, onPick }: { d: DriveInfo; on: boolean; onPick: () => void }) {
  const full = d.usedPercent >= 92;
  return (
    <button
      className={`drv${on ? " on" : ""}${full ? " full" : ""}`}
      onClick={onPick}
      title={d.isSystem ? "系统盘，不建议往这里写" : undefined}
    >
      <span className="t">
        <span>
          {d.letter}{" "}
          <span className="tag">
            {d.isSystem ? "系统盘（不建议）" : d.isDefaultTarget ? "推荐" : ""}
          </span>
        </span>
        {on && <span className="badge b-info">已选</span>}
      </span>
      <span className="bar">
        <i style={{ width: `${d.usedPercent}%` }} />
      </span>
      <span className="m">
        {d.freeText} 可用 · 共 {human(d.total)}
      </span>
    </button>
  );
}

/** 执行 + 进度卡片。 */
function RunCard(props: {
  plan: Plan | null;
  running: boolean;
  progress: Progress | null;
  report: MigrateReport | null;
  log: string[];
  onRun: () => void;
}) {
  const { plan, running, progress, report, log, onRun } = props;
  const can = !!plan && plan.blockers.length === 0 && !running;

  const doneSteps = new Set((report?.steps ?? []).filter((s) => s.ok).map((s) => nameToStep(s.name)));
  const nowStep = progress?.step ?? "";

  return (
    <Card title="执行" sub="原目录只改名不删除，随时可回滚">
      <div style={{ display: "flex", alignItems: "center", gap: 10 }}>
        <button className="btn pri" disabled={!can} onClick={onRun}>
          {running ? (
            <>
              <span className="spin" style={{ borderTopColor: "#fff", borderColor: "#4b5563" }} />
              正在迁移…
            </>
          ) : (
            "开始迁移"
          )}
        </button>
        <span style={{ color: "var(--muted)", fontSize: 12 }}>
          {running
            ? "不要关闭应用，也不要打开被迁移的 Agent"
            : plan?.blockers.length
              ? "有拦路虎，先按上面的提示处理"
              : "点击后会先让你确认一遍"}
        </span>
      </div>

      <div className="steps">
        {STEP_ORDER.map((s) => {
          const cls = doneSteps.has(s) ? "done" : nowStep === s ? "now" : "";
          return (
            <span key={s} className={`step ${cls}`}>
              {STEP_LABEL[s]}
            </span>
          );
        })}
      </div>

      {(running || progress) && (
        <>
          <div className="bar-prog" style={{ marginTop: 10 }}>
            <i style={{ width: `${progress?.percent ?? 3}%` }} />
          </div>
          <div style={{ marginTop: 6, fontSize: 12, color: "var(--muted)" }}>
            {progress ? `${STEP_LABEL[progress.step] ?? progress.step} · ${progress.detail}` : "准备中…"}
          </div>
        </>
      )}

      {report && (
        <div
          className={`note ${report.ok ? "ok" : report.rolledBack ? "warn" : "danger"}`}
          style={{ marginTop: 11 }}
        >
          {report.ok ? (
            <>
              <b>迁移完成。</b>数据在 <span className="mono">{report.dest}</span>，原路径已成为联接。
              确认应用一切正常后，可以在下面把快照删掉回收空间。
            </>
          ) : report.rolledBack ? (
            <>
              <b>迁移失败，已自动回滚</b> —— 原目录已改回原名，没有动你的数据。
              <div style={{ marginTop: 4 }}>{report.error}</div>
            </>
          ) : (
            <>
              <b>迁移失败（未回滚）</b>
              <div style={{ marginTop: 4 }}>{report.error}</div>
              {report.steps.length > 0 && (
                <div style={{ marginTop: 6, opacity: 0.85 }}>
                  目标盘里的副本是<b>有效增量</b>，下次重试会接着补，不用删。
                </div>
              )}
            </>
          )}
          {report.steps.length > 0 && (
            <div className="steps" style={{ marginTop: 8 }}>
              {report.steps.map((s) => (
                <span key={s.name} className={`step ${s.ok ? "done" : "fail"}`} title={s.detail}>
                  {s.ok ? "✓" : "✕"} {s.name}
                </span>
              ))}
            </div>
          )}
        </div>
      )}

      {log.length > 0 && <pre className="log">{log.join("\n")}</pre>}
    </Card>
  );
}

function nameToStep(name: string): string {
  switch (name) {
    case "复制":
      return "copying";
    case "校验":
      return "verifying";
    case "快照":
      return "snapshotting";
    case "建联接":
      return "linking";
    case "验证联接":
      return "verifying-link";
    default:
      return "";
  }
}

/** 快照卡片。 */
function SnapshotCard(props: {
  snaps: Snapshot[];
  rolling: boolean;
  onRollback: (id: string) => void;
  onDelete: (s: Snapshot) => void;
}) {
  const { snaps, rolling, onRollback, onDelete } = props;
  const total = snaps.reduce((n, s) => n + s.measure.bytes, 0);

  return (
    <Card title="快照" sub="迁移时原目录改名保留的那份，确认没问题再清">
      {snaps.length === 0 ? (
        <div className="note info">
          没有快照。迁移成功后这里会出现原目录的备份（<code>&lt;名字&gt;.moved-&lt;时间戳&gt;</code>）。
        </div>
      ) : (
        <>
          <table className="t">
            <thead>
              <tr>
                <th>Agent</th>
                <th>目录</th>
                <th>占用</th>
                <th>时间</th>
                <th style={{ textAlign: "right" }}>操作</th>
              </tr>
            </thead>
            <tbody>
              {snaps.map((s) => (
                <tr key={s.path}>
                  <td>{s.agentId}</td>
                  <td className="mono">{s.name}</td>
                  <td className="mono">
                    {human(s.measure.bytes)} · {s.measure.files.toLocaleString()}
                  </td>
                  <td className="mono">{s.created}</td>
                  <td style={{ textAlign: "right", whiteSpace: "nowrap" }}>
                    <button className="btn sm" disabled={rolling} onClick={() => onRollback(s.agentId)}>
                      回滚
                    </button>{" "}
                    <button className="btn sm" onClick={() => onDelete(s)}>
                      彻底删除
                    </button>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
          <div className="note warn" style={{ marginTop: 11 }}>
            共 {snaps.length} 个快照，合计 <b>{human(total)}</b>。删除快照后<b>这次迁移就不能回滚</b>了
            —— 建议先用几天确认没问题再删。
          </div>
        </>
      )}
    </Card>
  );
}
