/**
 * 共享的小组件。刻意不引 UI 库 —— 这个应用的视觉很轻，
 * 手写 CSS + 这几个组件就够了，还能少一堆依赖。
 */

import type { ReactNode } from "react";
import type { DetectedAgent } from "./types";
import { agentState, human } from "./types";
import { t, useLang, LangSwitch, stateLabelL } from "./i18n";
import { AgentIcon, AppMark } from "./icons";

// ---------------------------------------------------------------- 卡片

export function Card({
  title,
  sub,
  right,
  children,
}: {
  title: string;
  sub?: string;
  /** 标题右侧的附加内容（统计、按钮之类） */
  right?: ReactNode;
  children: ReactNode;
}) {
  return (
    <section className="card">
      <header className="cardhead">
        <span className="cardbar" />
        <h2>{title}</h2>
        {sub && <span className="sub">{sub}</span>}
        {right && <span className="cardright">{right}</span>}
      </header>
      <div className="inner">{children}</div>
    </section>
  );
}

// ---------------------------------------------------------------- 侧栏

export function Sidebar({
  agents,
  selected,
  scanning,
  onPick,
  onRescan,
  version,
}: {
  agents: DetectedAgent[];
  selected: string | null;
  scanning: boolean;
  onPick: (id: string) => void;
  onRescan: () => void;
  version?: string;
}) {
  // 侧栏分四组：Agent / 已卸载·只剩缓存 / 缓存目录（不是 Agent）/ 未安装。
  // `category` 来自 agents.toml，别用名字或 kind 猜 —— npm 缓存就不是 Agent。
  const present = agents.filter((a) => a.detected);
  const cachesOn = present.filter((a) => a.category === "cache");
  const orphans = present.filter(
    (a) => a.category !== "cache" && a.installed === false,
  );
  const agentsOn = present.filter(
    (a) => a.category !== "cache" && a.installed !== false,
  );
  const absent = agents.filter((a) => !a.detected);
  const grouped = cachesOn.length > 0 || orphans.length > 0;
  useLang(); // 语言切换时重渲染

  return (
    <aside className="side">
      <div className="brand">
        <AppMark size={34} />
        <div className="brandtx">
          <b>AgentCache</b>
          <span>{t("brandSub")}</span>
        </div>
      </div>

      <div className="nav">
        <div className="navlabel">
          <span>{t("detectedSummary", { n: present.length, total: agents.length })}</span>
          {scanning && <span className="spin" style={{ marginRight: 0 }} />}
        </div>

        {present.length === 0 && !scanning && (
          <div className="opt" style={{ padding: "8px 10px", color: "var(--muted)" }}>
            {t("noneDetected")}
          </div>
        )}

        {/* 分组标签只在「确实混着别的类别」时出现，纯 Agent 列表不多余提示 */}
        {grouped && (
          <div className="navlabel sub">{t("groupAgents", { n: agentsOn.length })}</div>
        )}
        {agentsOn.map((a) => (
          <button
            key={a.id}
            className={`item${selected === a.id ? " on" : ""}`}
            onClick={() => onPick(a.id)}
            title={a.paths.map((p) => p.resolved).join("\n")}
          >
            <AgentIcon id={a.id} name={a.name} size={22} />
            <span className="nm">{a.name}</span>
            <span className="sz">
              {a.total.bytes > 0 ? `${a.total.capped ? "≥" : ""}${human(a.total.bytes)}` : ""}
            </span>
            <span
              className={`dot ${a.running.length > 0 ? "running" : agentState(a)}`}
              title={
                a.running.length > 0
                  ? t("runningTip", { list: a.running.join(", ") })
                  : stateLabelL(agentState(a))
              }
            />
          </button>
        ))}

        {/* 应用已经卸载、只剩数据 —— 独立成组，别混在「Agent」里让人以为还装着 */}
        {orphans.length > 0 && (
          <>
            <div className="navlabel sub">{t("groupOrphans", { n: orphans.length })}</div>
            {orphans.map((a) => (
              <button
                key={a.id}
                className={`item orph${selected === a.id ? " on" : ""}`}
                onClick={() => onPick(a.id)}
                title={a.paths.map((p) => p.resolved).join("\n")}
              >
                <AgentIcon id={a.id} name={a.name} size={22} />
                <span className="nm">{a.name}</span>
                <span className="sz">
              {a.total.bytes > 0 ? `${a.total.capped ? "≥" : ""}${human(a.total.bytes)}` : ""}
            </span>
                <span className="dot orphan" title={stateLabelL(agentState(a))} />
              </button>
            ))}
          </>
        )}

        {cachesOn.length > 0 && (
          <div className="navlabel sub">{t("groupCaches", { n: cachesOn.length })}</div>
        )}
        {cachesOn.map((a) => (
          <button
            key={a.id}
            className={`item${selected === a.id ? " on" : ""}`}
            onClick={() => onPick(a.id)}
            title={a.paths.map((p) => p.resolved).join("\n")}
          >
            <AgentIcon id={a.id} name={a.name} size={22} />
            <span className="nm">{a.name}</span>
            <span className="sz">
              {a.total.bytes > 0 ? `${a.total.capped ? "≥" : ""}${human(a.total.bytes)}` : ""}
            </span>
            <span
              className={`dot ${a.running.length > 0 ? "running" : agentState(a)}`}
              title={
                a.running.length > 0
                  ? t("runningTip", { list: a.running.join(", ") })
                  : stateLabelL(agentState(a))
              }
            />
          </button>
        ))}

        {absent.length > 0 && (
          <>
            <div className="navlabel">{t("notInstalled", { n: absent.length })}</div>
            {absent.map((a) => (
              <button
                key={a.id}
                className={`item off${selected === a.id ? " on" : ""}`}
                onClick={() => onPick(a.id)}
                title={a.paths.map((p) => p.resolved).join("\n")}
              >
                <AgentIcon id={a.id} name={a.name} size={22} />
                <span className="nm">{a.name}</span>
              </button>
            ))}
          </>
        )}
      </div>

      <div className="sidefoot">
        <button className="btn sm" onClick={onRescan} disabled={scanning}>
          {scanning ? t("scanning") : t("rescan")}
        </button>
        <LangSwitch />
        <span className="sp" />
        {version && <span className="mono">v{version}</span>}
      </div>
    </aside>
  );
}

// ---------------------------------------------------------------- 确认弹窗

export function Confirm({
  title,
  okText,
  danger,
  extra,
  onCancel,
  onOk,
  children,
}: {
  title: string;
  okText: string;
  danger?: boolean;
  /** 次要动作（如「移入回收站」）—— 介于取消与主按钮之间 */
  extra?: { text: string; onOk: () => void };
  onCancel: () => void;
  onOk: () => void;
  children: ReactNode;
}) {
  return (
    <div className="modal" onClick={onCancel}>
      <div className="modalcard" onClick={(e) => e.stopPropagation()}>
        <div className={`modalhead${danger ? " danger" : ""}`}>
          <span className="modaldot" />
          {title}
        </div>
        <div className="modalbody">{children}</div>
        <div className="modalfoot">
          <button className="btn" onClick={onCancel}>
            {t("cancel")}
          </button>
          {extra && (
            <button
              className="btn"
              onClick={() => {
                const f = extra.onOk;
                onCancel(); // 次要动作也要关掉对话框，否则弹窗会一直压在界面上
                f();
              }}
            >
              {extra.text}
            </button>
          )}
          <button className={`btn pri${danger ? " danger" : ""}`} onClick={onOk}>
            {okText}
          </button>
        </div>
      </div>
    </div>
  );
}

// ---------------------------------------------------------------- Toast

export interface ToastItem {
  id: number;
  msg: string;
  detail?: string;
  err?: boolean;
}

export function Toast({ items }: { items: ToastItem[] }) {
  if (items.length === 0) return null;
  return (
    <div className="toast">
      {items.map((t) => (
        <div key={t.id} className={`t${t.err ? " err" : ""}`}>
          <span className="ti" aria-hidden>
            {t.err ? "!" : "✓"}
          </span>
          <div className="tx">
            {t.msg}
            {t.detail && <div className="d">{t.detail}</div>}
          </div>
        </div>
      ))}
    </div>
  );
}
