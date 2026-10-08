//! 数据模型：注册表（输入）与检测结果（输出）。
//!
//! 输入侧对应 `agents.toml`；输出侧是给界面直接用的形状。
//! 两侧都 `Serialize`，方便 CLI 出 JSON、Tauri 直接透给前端。

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

// ============================================================ 输入：注册表

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RegistryFile {
    pub schema_version: u32,
    #[serde(default, rename = "agent")]
    pub agents: Vec<AgentDef>,
}

/// 一个 Agent 的声明。字段全部带默认值 —— 配置文件是给人手写、手改的，
/// 少写一个字段不该直接报错。
///
/// 🔴 `rename_all(serialize = "camelCase")` 是**只针对序列化**的：
/// 读 `agents.toml` 时字段名仍是蛇形（`env_relocate`，TOML 的惯例），
/// 但发给前端时变成驼峰（`envRelocate`），跟别的输出模型保持一致。
/// 用普通的 `rename_all` 会顺手改掉反序列化侧，导致配置读不出来。
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all(serialize = "camelCase"))]
pub struct AgentDef {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub vendor: Option<String>,

    /// `electron` | `cli` | `generic`
    #[serde(default = "d_kind")]
    pub kind: String,

    /// 迁移建议等级：`safe` | `normal` | `special` | `system`
    #[serde(default = "d_class")]
    pub class: String,

    /// 数据目录模板，支持 `~` / `%APPDATA%` / `%LOCALAPPDATA%` / `%USERPROFILE%`
    #[serde(default)]
    pub paths: Vec<String>,

    /// Electron 更新器目录名前缀（在 `%LOCALAPPDATA%` 下）。**仅用于检测**。
    #[serde(default)]
    pub updater: Vec<String>,

    /// 该应用**官方支持**的数据目录重定位开关
    #[serde(default)]
    pub env_relocate: Option<EnvRelocate>,

    /// 迁移前需要退出的进程（不区分大小写，支持 `*`）
    #[serde(default)]
    pub processes: Vec<String>,

    /// 校验时跳过的 glob
    #[serde(default)]
    pub skip: Vec<String>,

    /// 预期是「目录或联接」的路径模板，`*` 表示任意一层。断链会告警。
    #[serde(default)]
    pub runtime_links: Vec<String>,

    #[serde(default)]
    pub note: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all(serialize = "camelCase"))]
pub struct EnvRelocate {
    /// 环境变量名，例如 `CLAUDE_CONFIG_DIR`
    pub env: String,
    #[serde(default)]
    pub note: Option<String>,
}

fn d_kind() -> String {
    "generic".into()
}
fn d_class() -> String {
    "normal".into()
}

// ============================================================ 输出：检测结果

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DetectedAgent {
    pub id: String,
    pub name: String,
    pub vendor: Option<String>,
    pub kind: String,
    pub class: String,
    pub note: Option<String>,
    pub env_relocate: Option<EnvRelocate>,

    /// 综合判定：装了没
    pub detected: bool,
    /// 判定依据（给用户看的，别让他猜）
    pub how: Vec<String>,

    pub paths: Vec<PathHit>,
    /// 命中的更新器目录
    pub updaters: Vec<PathBuf>,
    /// 预期是联接、但已经断链的路径
    pub broken_links: Vec<BrokenLink>,
    /// 匹配到的运行中进程名
    pub running: Vec<String>,
    /// 该 Agent 的总占用（只统计真实存在的、且不跟随联接）
    pub total: crate::fsutil::Measure,
}

/// 一个数据目录的命中情况。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PathHit {
    /// 模板原文（`~/.codex`）
    pub template: String,
    pub resolved: String,
    pub kind: String,
    pub is_link: bool,
    pub link_target: Option<String>,
    pub link_broken: bool,
    pub measure: crate::fsutil::Measure,
    /// 真实的文件数/字节数在界面上要标「约」还是精确
    pub size_text: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BrokenLink {
    pub path: String,
    pub target: Option<String>,
}

impl DetectedAgent {
    /// **迁移状态**：`moved`（已迁移）/ `free`（未迁移）/ `absent`（没装）。
    ///
    /// 🔴 刻意**不**把「正在运行」混进来 —— 那是另一个维度的事实。
    /// 早先把 running 优先返回，导致「已迁移但正在跑」的条目被算成没迁移，
    /// 统计数字（已迁移 N）直接不对。状态和运行与否要分开表达。
    pub fn state(&self) -> &'static str {
        if !self.detected {
            return "absent";
        }
        if self.paths.iter().any(|p| p.is_link && !p.link_broken) {
            return "moved";
        }
        "free"
    }

    /// 迁移状态的中文标签。
    pub fn state_label(&self) -> &'static str {
        match self.state() {
            "moved" => "已迁移",
            "free" => "未迁移",
            _ => "未检测到",
        }
    }
}

#[cfg(test)]
mod state_tests {
    use super::*;
    use crate::fsutil::Measure;

    fn hit(is_link: bool, broken: bool) -> PathHit {
        PathHit {
            template: "~/.x".into(),
            resolved: r"C:\x".into(),
            kind: if is_link { "link".into() } else { "dir".into() },
            is_link,
            link_target: Some(r"D:\x".into()),
            link_broken: broken,
            measure: Measure::default(),
            size_text: "—".into(),
        }
    }

    fn agent(detected: bool, hits: Vec<PathHit>, running: Vec<String>) -> DetectedAgent {
        DetectedAgent {
            id: "x".into(),
            name: "X".into(),
            vendor: None,
            kind: "generic".into(),
            class: "normal".into(),
            note: None,
            env_relocate: None,
            detected,
            how: Vec::new(),
            paths: hits,
            updaters: Vec::new(),
            broken_links: Vec::new(),
            running,
            total: Measure::default(),
        }
    }

    #[test]
    fn state_is_about_migration_not_running() {
        // 已迁移 + 正在运行 → 仍然是「已迁移」（这是之前统计错的那个 case）
        let a = agent(true, vec![hit(true, false)], vec!["X.exe".into()]);
        assert_eq!(a.state(), "moved");
        assert_eq!(a.state_label(), "已迁移");

        // 断链的联接**不算**已迁移 —— 数据其实没接上
        let b = agent(true, vec![hit(true, true)], vec![]);
        assert_eq!(b.state(), "free");

        // 普通目录 → 未迁移
        let c = agent(true, vec![hit(false, false)], vec![]);
        assert_eq!(c.state(), "free");

        // 没装
        let d = agent(false, vec![], vec![]);
        assert_eq!(d.state(), "absent");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_forgiving() {
        // 只写 id / name / paths 也要能读进来（配置文件是人手写的）
        let f: RegistryFile = toml::from_str(
            r#"
schema_version = 1
[[agent]]
id = "x"
name = "X"
paths = ["~/.x"]
"#,
        )
        .unwrap();
        assert_eq!(f.agents.len(), 1);
        assert_eq!(f.agents[0].kind, "generic");
        assert_eq!(f.agents[0].class, "normal");
        assert!(f.agents[0].env_relocate.is_none());
        assert!(f.agents[0].processes.is_empty());
    }

    #[test]
    fn env_relocate_parses() {
        let f: RegistryFile = toml::from_str(
            r#"
schema_version = 1
[[agent]]
id = "codex"
name = "Codex"
paths = ["~/.codex"]
env_relocate = { env = "CODEX_HOME", note = "官方支持" }
"#,
        )
        .unwrap();
        assert_eq!(f.agents[0].env_relocate.as_ref().unwrap().env, "CODEX_HOME");
    }
}
