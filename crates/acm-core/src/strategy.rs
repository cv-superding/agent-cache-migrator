//! **每个 Agent 用哪种迁移方式最合适** —— 自动匹配，而不是一律建联接。
//!
//! 优先级（越靠前越干净、越不怕应用更新）：
//!
//! 1. `env` —— 应用**官方读环境变量**（`CLAUDE_CONFIG_DIR` / `CODEX_HOME` / `OLLAMA_MODELS`）。
//!    原路径什么都不留，换机也能带着变量走；代价是变量是持久系统变更，要能撤销。
//! 2. `launch_arg` —— Chromium / VS Code 系认 `--user-data-dir`。数据同样搬家、
//!    原路径不留东西；代价是要改快捷方式（桌面 / 开始菜单 / 任务栏）。
//! 3. `setting` —— 应用自己在设置页里能改目录（LM Studio / Chatbox / Jan）。
//!    内核代劳不了，只能给指引：硬塞联接反而可能和它自己的索引打架。
//! 4. `junction` —— **兜底**：透明、免管理员、对 99% 应用无感，就怕应用升级时重建目录。
//!
//! 内核不硬编码任何 Agent：这里只看 `agents.toml` 声明了哪些能力。

use crate::model::{AgentDef, RelocatePlan};

/// 按声明自动推荐一个 Agent 的最优迁移方式。
pub fn recommend(def: &AgentDef) -> RelocatePlan {
    if let Some(env) = &def.env_relocate {
        let mut alternatives = Vec::new();
        if !def.launch_args.is_empty() {
            alternatives.push("启动参数（--user-data-dir）".into());
        }
        alternatives.push("目录联接（兜底）".into());
        return RelocatePlan {
            kind: "env".into(),
            label: "环境变量".into(),
            note: env
                .note
                .clone()
                .unwrap_or_else(|| "应用官方读这个变量，原目录可以不留任何东西".into()),
            detail: Some(env.env.clone()),
            auto: true,
            alternatives,
        };
    }

    if !def.launch_args.is_empty() {
        let args: Vec<String> = def
            .launch_args
            .iter()
            .map(|a| a.replace("{dest}", "<目标目录>"))
            .collect();
        return RelocatePlan {
            kind: "launch_arg".into(),
            label: "启动参数".into(),
            note: "Chromium / VS Code 系认这个参数，数据搬家后原目录不留东西；会改快捷方式（已备份，可还原）".into(),
            detail: Some(args.join(" ")),
            auto: true,
            alternatives: vec!["目录联接（兜底）".into()],
        };
    }

    // 缓存类目录（npm 缓存之类）没有「官方开关」，联接就是最优解
    let is_cache = def.category == "cache";
    RelocatePlan {
        kind: "junction".into(),
        label: "目录联接".into(),
        note: if is_cache {
            "纯缓存目录，没有官方开关；目录联接透明、免管理员最省事".into()
        } else {
            "该应用没有公开的重定位开关；目录联接对应用透明，是通用兜底".into()
        },
        detail: None,
        auto: true,
        alternatives: Vec::new(),
    }
}

/// 「应用内设置」类：内核不执行，只给指引（保留给未来的 `setting` 声明）。
pub fn recommend_setting(note: &str) -> RelocatePlan {
    RelocatePlan {
        kind: "setting".into(),
        label: "应用内设置".into(),
        note: note.to_string(),
        detail: None,
        auto: false,
        alternatives: vec!["目录联接（兜底）".into()],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{AgentDef, EnvRelocate};

    fn def(toml: &str) -> AgentDef {
        toml::from_str(toml).unwrap()
    }

    #[test]
    fn env_wins_over_launch_arg() {
        let d = def(
            r#"
id = "claude"
name = "Claude Code"
[env_relocate]
env = "CLAUDE_CONFIG_DIR"
"#,
        );
        let r = recommend(&d);
        assert_eq!(r.kind, "env");
        assert_eq!(r.detail.as_deref(), Some("CLAUDE_CONFIG_DIR"));
        assert!(r.auto);
    }

    #[test]
    fn launch_arg_when_no_env() {
        let d = def(
            r#"
id = "trae"
name = "Trae CN"
launch_args = ["--user-data-dir={dest}"]
shortcut_exes = ["Trae.exe"]
"#,
        );
        let r = recommend(&d);
        assert_eq!(r.kind, "launch_arg");
        assert!(r.detail.unwrap().contains("<目标目录>"));
        assert!(r.auto);
    }

    #[test]
    fn junction_is_the_fallback() {
        let d = def(
            r#"
id = "unknown-agent"
name = "Unknown"
"#,
        );
        let r = recommend(&d);
        assert_eq!(r.kind, "junction");
        assert_eq!(r.label, "目录联接");
        assert!(r.note.contains("透明"));
    }

    #[test]
    fn cache_dir_still_junction_but_explains() {
        let d = def(
            r#"
id = "npm-cache"
name = "npm 缓存"
category = "cache"
"#,
        );
        let r = recommend(&d);
        assert_eq!(r.kind, "junction");
        assert!(r.note.contains("纯缓存"));
    }

    #[test]
    fn env_relocate_parses() {
        let e: EnvRelocate = toml::from_str(r#"env = "CODEX_HOME""#).unwrap();
        assert_eq!(e.env, "CODEX_HOME");
    }
}
