/**
 * Agent 图标。
 *
 * 原则：能用「应用自己的图标」就不用占位字母 —— 侧栏、标题、快照表、历史里
 * 显示的都是各家的真图标，用户一眼能认出是哪个 Agent。
 *
 * 来源分两类：
 *   · png   —— 官方应用图标（桌面端 / 官网 app icon），打包进安装包，离线可用
 *   · mark  —— 官方品牌矢量标志（simple-icons 导出，见 icons-data.ts），任意 DPI 都清晰
 *
 * 没登记的 id（用户自己在 agents.toml 里加的）走首字母徽章兜底。
 */

import chatboxPng from "./assets/agents/chatbox.png";
import cherryPng from "./assets/agents/cherry.png";
import codebuddyPng from "./assets/agents/codebuddy.png";
import doubaoPng from "./assets/agents/doubao.png";
import feishuPng from "./assets/agents/feishu.png";
import janPng from "./assets/agents/jan.png";
import lingmaPng from "./assets/agents/lingma.png";
import openaiPng from "./assets/agents/openai.png";
import utoolsPng from "./assets/agents/utools.png";
import workbuddyPng from "./assets/agents/workbuddy.png";
import zcodePng from "./assets/agents/zcode.png";
import { BRAND_MARKS } from "./icons-data";

interface IconSpec {
  /** 官方应用图标（位图） */
  png?: string;
  /** 官方品牌矢量标志 */
  mark?: keyof typeof BRAND_MARKS;
  /** 矢量标志的自定义颜色（默认取品牌色） */
  color?: string;
}

/** id → 图标。同一个产品的不同版本（国内版 / 国际版）共用一张图。 */
const AGENT_ICONS: Record<string, IconSpec> = {
  "workbuddy-cn": { png: workbuddyPng },
  "workbuddy-intl": { png: workbuddyPng },
  codex: { png: openaiPng },
  claude: { mark: "claude" },
  cursor: { mark: "cursor" },
  "codebuddy-cn": { png: codebuddyPng },
  "codebuddy-intl": { png: codebuddyPng },
  trae: { mark: "trae" },
  "trae-intl": { mark: "trae" },
  zcode: { png: zcodePng },
  deepseek: { mark: "deepseek" },
  doubao: { png: doubaoPng },
  "gemini-cli": { mark: "gemini" },
  lmstudio: { mark: "lmstudio" },
  ollama: { mark: "ollama" },
  chatbox: { png: chatboxPng },
  jan: { png: janPng },
  feishu: { png: feishuPng },
  cherry: { png: cherryPng },
  utools: { png: utoolsPng },
  windsurf: { mark: "windsurf" },
  lingma: { png: lingmaPng },
  "npm-cache": { mark: "npm" },
};

/** 兜底徽章的底色（用户自己加的 Agent 才会用到）。 */
const FALLBACK_COLORS = ["#475569", "#0f766e", "#b45309", "#7c3aed", "#0369a1", "#be123c"];

/** 名字里第一个拉丁字母，否则第一个字 —— 兜底徽章用。 */
function monogram(id: string, name?: string): string {
  const s = name?.trim() || id;
  const latin = s.match(/[A-Za-z]/);
  return latin ? latin[0].toUpperCase() : s.slice(0, 1);
}

function fallbackColor(id: string): string {
  let h = 0;
  for (let i = 0; i < id.length; i += 1) h = (h * 31 + id.charCodeAt(i)) % 997;
  return FALLBACK_COLORS[h % FALLBACK_COLORS.length];
}

/** 某个 Agent 有没有登记真图标（没登记就画徽章）。 */
export function hasRealIcon(id: string): boolean {
  return Boolean(AGENT_ICONS[id]);
}

/**
 * Agent 图标。
 *
 * `size` 是图标外框边长（px）；文字徽章会按它自动缩放。
 */
export function AgentIcon({
  id,
  name,
  size = 22,
  radius,
}: {
  id: string;
  name?: string;
  size?: number;
  /** 圆角：给位图裁一个圆角方框（macOS 那种图标观感） */
  radius?: number;
}) {
  const spec = AGENT_ICONS[id];
  const box = { width: size, height: size, flex: `${size}px 0 0` } as const;

  if (spec?.png) {
    return (
      <img
        className="aic"
        src={spec.png}
        alt=""
        aria-hidden
        style={{ ...box, borderRadius: radius ?? Math.round(size * 0.22), background: "transparent" }}
      />
    );
  }

  if (spec?.mark) {
    const m = BRAND_MARKS[spec.mark];
    return (
      <svg className="aic" viewBox="0 0 24 24" style={box} aria-hidden>
        <path d={m.path} fill={spec.color ?? m.hex} />
      </svg>
    );
  }

  return (
    <span
      className="aic aic-mono"
      style={{
        ...box,
        borderRadius: radius ?? Math.round(size * 0.3),
        background: fallbackColor(id),
        fontSize: Math.max(9, Math.round(size * 0.46)),
      }}
      aria-hidden
    >
      {monogram(id, name)}
    </span>
  );
}

/** 应用自己的标记（侧栏左上角），跟 installer 里的图标同源观感：深色圆角块 + 搬运箭头。 */
export function AppMark({ size = 34 }: { size?: number }) {
  return (
    <svg className="appmark" width={size} height={size} viewBox="0 0 40 40" aria-hidden>
      <defs>
        <linearGradient id="appmark-bg" x1="0" y1="0" x2="1" y2="1">
          <stop offset="0%" stopColor="#1f2937" />
          <stop offset="55%" stopColor="#134e4a" />
          <stop offset="100%" stopColor="#0f766e" />
        </linearGradient>
      </defs>
      <rect x="0" y="0" width="40" height="40" rx="11" fill="url(#appmark-bg)" />
      {/* 左边三个数据块，箭头指向右边的空位 —— 就是「把数据搬过去」 */}
      <g fill="#ffffff">
        <rect x="9" y="9.5" width="8.5" height="5" rx="2" opacity="0.55" />
        <rect x="9" y="17.5" width="8.5" height="5" rx="2" opacity="0.75" />
        <rect x="9" y="25.5" width="8.5" height="5" rx="2" />
      </g>
      <g stroke="#5eead4" strokeWidth="1.9" fill="none" strokeLinecap="round">
        <path d="M21 20h7.5" />
        <path d="m25.4 16.4 3.6 3.6-3.6 3.6" />
      </g>
      <circle cx="31" cy="20" r="3.2" fill="#5eead4" opacity="0.22" />
    </svg>
  );
}
