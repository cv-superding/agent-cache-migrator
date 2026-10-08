/**
 * 共享的小组件。刻意不引 UI 库 —— 这个应用的视觉很轻，
 * 手写 CSS + 这几个组件就够了，还能少一堆依赖。
 */

import type { ReactNode } from "react";
import type { DetectedAgent } from "./types";
import { agentState, human, stateLabel } from "./types";

// ---------------------------------------------------------------- 卡片

export function Card({
  title,
  sub,
  children,
}: {
  title: string;
  sub?: string;
  children: ReactNode;
}) {
  return (
    <section className="card">
      <h2>
        {title}
        {sub && <span className="sub">{sub}</span>}
      </h2>
      <div className="inner">{children}</div>
    </section>
  );
}

// ---------------------------------------------------------------- 侧栏

export function Sidebar({
  agents,
  colors,
  selected,
  scanning,
  onPick,
  onRescan,
  version,
}: {
  agents: DetectedAgent[];
  colors: Record<string, string>;
  selected: string | null;
  scanning: boolean;
  onPick: (id: string) => void;
  onRescan: () => void;
  version?: string;
}) {
  const present = agents.filter((a) => a.detected);
  const absent = agents.filter((a) => !a.detected);

  return (
    <aside className="side">
      <div className="brand">
        <div className="logo">⬒</div>
        <div>
          <b>AgentCache</b>
          <span>Agent 缓存迁移器</span>
        </div>
      </div>

      <div className="nav">
        <div className="navlabel">
          检测到 {present.length} 个 Agent
          {scanning && (
            <>
              {" "}
              <span className="spin" style={{ marginRight: 0 }} />
            </>
          )}
        </div>

        {present.length === 0 && !scanning && (
          <div className="opt" style={{ padding: "8px 9px", color: "var(--muted)" }}>
            没有检测到任何 Agent。
          </div>
        )}

        {present.map((a) => (
          <button
            key={a.id}
            className={`item${selected === a.id ? " on" : ""}`}
            onClick={() => onPick(a.id)}
            title={a.paths.map((p) => p.resolved).join("\n")}
          >
            <span className="ic" style={{ background: colors[a.id] ?? "#94a3b8" }}>
              {glyph(a)}
            </span>
            <span className="nm">{a.name}</span>
            <span className="sz">{a.total.bytes > 0 ? human(a.total.bytes) : ""}</span>
            <span
              className={`dot ${a.running.length > 0 ? "running" : agentState(a)}`}
              title={a.running.length > 0 ? `运行中：${a.running.join(", ")}` : stateLabel(agentState(a))}
            />
          </button>
        ))}

        {absent.length > 0 && (
          <>
            <div className="navlabel">未安装（{absent.length}）</div>
            {absent.map((a) => (
              <button
                key={a.id}
                className="item"
                onClick={() => onPick(a.id)}
                style={{ opacity: 0.55 }}
                title={a.paths.map((p) => p.resolved).join("\n")}
              >
                <span className="ic" style={{ background: "#d4d4d8", color: "#71717a" }}>
                  {glyph(a)}
                </span>
                <span className="nm">{a.name}</span>
              </button>
            ))}
          </>
        )}
      </div>

      <div className="sidefoot">
        <button className="btn sm" onClick={onRescan} disabled={scanning}>
          {scanning ? "扫描中…" : "重新扫描"}
        </button>
        <span className="sp" />
        {version && <span className="mono">v{version}</span>}
      </div>
    </aside>
  );
}

/** 图标里的那个字：优先取名字里的拉丁字母，否则取第一个汉字。 */
function glyph(a: DetectedAgent): string {
  const latin = a.name.match(/[A-Za-z]/);
  if (latin) return latin[0].toUpperCase();
  return a.name.slice(0, 1);
}

// ---------------------------------------------------------------- 确认弹窗

export function Confirm({
  title,
  okText,
  danger,
  onCancel,
  onOk,
  children,
}: {
  title: string;
  okText: string;
  danger?: boolean;
  onCancel: () => void;
  onOk: () => void;
  children: ReactNode;
}) {
  return (
    <div
      style={{
        position: "fixed",
        inset: 0,
        background: "rgba(0,0,0,.34)",
        display: "grid",
        placeItems: "center",
        zIndex: 80,
        padding: 24,
      }}
      onClick={onCancel}
    >
      <div
        style={{
          background: "#fff",
          borderRadius: 13,
          width: 520,
          maxWidth: "100%",
          maxHeight: "100%",
          overflow: "auto",
          boxShadow: "0 16px 48px rgba(0,0,0,.24)",
        }}
        onClick={(e) => e.stopPropagation()}
      >
        <div style={{ padding: "16px 18px 0", fontSize: 15, fontWeight: 600 }}>{title}</div>
        <div style={{ padding: "10px 18px 4px", fontSize: 12.5, color: "#3f3f46" }}>{children}</div>
        <div
          style={{
            padding: "14px 18px 16px",
            display: "flex",
            gap: 8,
            justifyContent: "flex-end",
          }}
        >
          <button className="btn" onClick={onCancel}>
            取消
          </button>
          <button
            className="btn pri"
            onClick={onOk}
            style={danger ? { background: "#b91c1c", borderColor: "#b91c1c" } : undefined}
          >
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
          {t.msg}
          {t.detail && <div className="d">{t.detail}</div>}
        </div>
      ))}
    </div>
  );
}
