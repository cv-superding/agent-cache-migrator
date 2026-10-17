//! 体检（doctor）：**不动任何东西**，把当前状态里的毛病一次性报出来。
//!
//! 覆盖四类问题：
//! 1. 每个 Agent 数据目录的健康状态（断链 / 缺失 / 正常）
//! 2. 正在运行的进程（迁移前必须退出）
//! 3. 快照的脏状态（重复快照 / 原位与快照并存 / 孤儿快照）
//! 4. 环境变量重定位的当前值（信息展示）
//!
//! 每条发现都带 `hint` —— 一条**可以直接复制执行**的修复命令。

use crate::fsutil;
use crate::migrate::{list_snapshots, Snapshot, SNAPSHOT_MARK};
use crate::AgentDef;
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// 一条体检发现。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Finding {
    /// `ok` | `warn` | `error` | `info`
    pub level: String,
    /// 归属区域：agent id / `snapshot` / `env`
    pub area: String,
    pub message: String,
    /// 可直接执行的修复命令（如 `acm repair codex`）
    pub hint: Option<String>,
}

fn ok(area: &str, msg: String) -> Finding {
    Finding {
        level: "ok".into(),
        area: area.into(),
        message: msg,
        hint: None,
    }
}
fn info(area: &str, msg: String) -> Finding {
    Finding {
        level: "info".into(),
        area: area.into(),
        message: msg,
        hint: None,
    }
}
fn warn(area: &str, msg: String, hint: Option<String>) -> Finding {
    Finding {
        level: "warn".into(),
        area: area.into(),
        message: msg,
        hint,
    }
}
fn error(area: &str, msg: String, hint: Option<String>) -> Finding {
    Finding {
        level: "error".into(),
        area: area.into(),
        message: msg,
        hint,
    }
}

/// 由快照路径反推原位：`~/.codex.moved-20261017-000000` → `~/.codex`。
/// 命名不符合约定的返回 None（本模块不负责校验，交给 delete 侧）。
fn snapshot_original_path(snapshot: &Path) -> Option<PathBuf> {
    let name = snapshot.file_name()?.to_string_lossy();
    let idx = name.find(SNAPSHOT_MARK)?;
    let parent = snapshot.parent()?;
    Some(parent.join(&name[..idx]))
}

/// 跑全部检查。
pub fn run_checks(defs: &[AgentDef]) -> Vec<Finding> {
    let mut out = Vec::new();
    let procs = crate::procs::snapshot();

    // ① 每 Agent 的数据目录状态 + 运行中进程
    for def in defs {
        for src in def.paths.iter().filter_map(|t| crate::paths::expand(t)) {
            match fsutil::classify(&src) {
                fsutil::EntryKind::Link {
                    broken: false,
                    target,
                } => {
                    out.push(ok(
                        &def.id,
                        format!(
                            "联接正常：{} → {}",
                            src.display(),
                            target
                                .as_ref()
                                .map(|t| t.to_string_lossy())
                                .unwrap_or_default()
                        ),
                    ));
                }
                fsutil::EntryKind::Link { broken: true, .. } => {
                    out.push(error(
                        &def.id,
                        format!(
                            "联接已断：{}（应用每次启动都会白做一遍准备工作）",
                            src.display()
                        ),
                        Some(format!("acm repair {}", def.id)),
                    ));
                }
                fsutil::EntryKind::Missing => {
                    // 目录缺失本身可能是「还没启动过」—— 交给 updaters 指纹说话
                    out.push(info(
                        &def.id,
                        format!("数据目录不存在：{}（应用可能还没启动过）", src.display()),
                    ));
                }
                fsutil::EntryKind::Dir => {
                    out.push(ok(
                        &def.id,
                        format!(
                            "真实目录：{}（未迁移，可以用 acm plan {} 看迁移计划）",
                            src.display(),
                            def.id
                        ),
                    ));
                }
                fsutil::EntryKind::File => {
                    out.push(warn(
                        &def.id,
                        format!(
                            "{} 是一个文件而不是目录 —— 路径可能被别的程序占了",
                            src.display()
                        ),
                        None,
                    ));
                }
            }

            let running = crate::procs::matching_in(&procs, &def.processes);
            if !running.is_empty() {
                out.push(warn(
                    &def.id,
                    format!("正在运行：{}", running.join(", ")),
                    Some("迁移前请退出该应用".to_string()),
                ));
            }
        }
    }

    // ② 快照状态
    let all = list_snapshots(defs);
    let known: std::collections::BTreeSet<&str> = defs.iter().map(|d| d.id.as_str()).collect();

    // 2a) 按 agent 统计快照数 —— 多个快照 = 重复迁移留下的脏状态
    let mut by_agent: BTreeMap<String, Vec<&Snapshot>> = BTreeMap::new();
    for s in &all {
        by_agent.entry(s.agent_id.clone()).or_default().push(s);
    }
    for (id, snaps) in &by_agent {
        if snaps.len() > 1 {
            let bytes: u64 = snaps.iter().map(|s| s.measure.bytes).sum();
            out.push(warn(
                "snapshot",
                format!(
                    "{} 有 {} 个快照（共 {}）—— 重复迁移留下的旧快照，状态不干净",
                    id,
                    snaps.len(),
                    fsutil::human(bytes)
                ),
                Some("在界面「快照清理」里删除不再需要的旧快照".to_string()),
            ));
        }
    }

    // 2b) 原位与快照并存 —— 迁移可能中途失败留下的状态
    for s in &all {
        let Some(orig) = snapshot_original_path(Path::new(&s.path)) else {
            continue;
        };
        if matches!(fsutil::classify(&orig), fsutil::EntryKind::Dir) {
            out.push(error(
                "snapshot",
                format!(
                    "{} 的原位 {} 现在是真实目录、但快照 {} 也在 —— 迁移可能中途失败，两边都有数据",
                    s.agent_id,
                    orig.display(),
                    s.path
                ),
                Some(
                    "先确认哪边的数据是新的，再决定：保留原位就用界面清理快照；\
                     想要快照内容就手工把快照目录的内容合并回去"
                        .to_string(),
                ),
            ));
        }
    }

    // 2c) 孤儿快照 —— 注册表里已经没有对应 agent 的快照
    // 扫已注册 agent 的父目录 + 各 var/家目录：快照一定出现在「原路径的同层」
    let mut parents: Vec<PathBuf> = Vec::new();
    for def in defs {
        for tpl in &def.paths {
            if let Some(src) = crate::paths::expand(tpl) {
                if let Some(p) = src.parent() {
                    if !parents.contains(&p.to_path_buf()) {
                        parents.push(p.to_path_buf());
                    }
                }
            }
        }
    }
    for parent in &parents {
        let Ok(rd) = std::fs::read_dir(parent) else {
            continue;
        };
        for entry in rd.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            let Some(idx) = name.find(SNAPSHOT_MARK) else {
                continue;
            };
            // 按命名约定反解 agent id：`.codex.moved-…` → `.codex` → 与注册表比对
            let dir_prefix = &name[..idx];
            let agent_id = dir_prefix.trim_start_matches('.');
            if !known.contains(agent_id) {
                out.push(warn(
                    "snapshot",
                    format!(
                        "孤儿快照：{}（注册表里没有 id = {agent_id} 的 Agent —— 可能是自定义 Agent 已从 agents.toml 移除）",
                        entry.path().display()
                    ),
                    Some("确认无用后在界面「快照清理」删除（或手工改回原名恢复数据）".to_string()),
                ));
            }
        }
    }

    // ③ 环境变量重定位的当前值（信息展示，帮助回忆「我当时设过什么」）
    for def in defs {
        let Some(env) = &def.env_relocate else {
            continue;
        };
        match crate::envvar::get(&env.env) {
            Some(v) => out.push(info(
                &def.id,
                format!(
                    "{} = {v}（环境变量重定位已设置；`acm env {} --unset` 可恢复默认路径）",
                    env.env, def.id
                ),
            )),
            None => out.push(info(&def.id, format!("{} 未设置（走联接方式）", env.env))),
        }
    }

    out
}

/// 汇总：`(errors, warnings)` —— CLI/GUI 拿它决定退出码与文案。
pub fn summarize(findings: &[Finding]) -> (usize, usize) {
    (
        findings.iter().filter(|f| f.level == "error").count(),
        findings.iter().filter(|f| f.level == "warn").count(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::migrate::SNAPSHOT_MARK;

    /// 快照反推原位：标准命名 + 非法命名。
    #[test]
    fn snapshot_original_path_roundtrip() {
        let base = std::env::temp_dir().join(format!("acm-doctor-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        // 快照路径 = <base>/.codex.moved-ts
        let snap = base.join(format!(".codex{SNAPSHOT_MARK}20261017-000000"));
        std::fs::create_dir_all(&snap).unwrap();
        let orig = snapshot_original_path(&snap).expect("合法命名应当能反推");
        assert_eq!(orig, base.join(".codex"));

        // 没有 mark 的目录 → None
        let plain = base.join("plain");
        std::fs::create_dir_all(&plain).unwrap();
        assert!(snapshot_original_path(&plain).is_none());

        let _ = std::fs::remove_dir_all(&base);
    }

    /// 孤儿快照：注册表里没有的 agent 快照要报出来。
    #[test]
    fn orphan_snapshot_is_reported() {
        let base = std::env::temp_dir().join(format!("acm-doctor-orphan-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        // 孤儿快照 = 快照名字里的 agent 不在注册表里。
        // 注册 fakeagent（数据目录 missing，走 missing 分支），
        // 但同层放一个 ghostagent 的快照 —— ghost 不在注册表，应被报出。
        std::fs::create_dir_all(base.join(".ghostagent.moved-20261017-000000")).unwrap();

        let cfg = base.join("agents.toml");
        // 🔴 TOML 里用正斜杠写 Windows 路径（合法且不用纠结转义）；
        //    反斜杠经过 format! + TOML 双层转义会把 `C:\U` 吃成 `C:Users`（实测踩过）
        let cfg_text = format!(
            "schema_version = 1\n[[agent]]\nid = \"fakeagent\"\nname = \"FA\"\npaths = [\"{}/.fakeagent\"]\nprocesses = []\n",
            base.to_string_lossy().replace('\\', "/")
        );
        std::fs::write(&cfg, cfg_text).unwrap();
        let defs = crate::agents::load(Some(&cfg)).unwrap();
        let findings = run_checks(&defs);
        assert!(
            findings.iter().any(|f| f.area == "snapshot"
                && f.message.contains("孤儿快照")
                && f.message.contains("ghostagent")),
            "应当报出 ghostagent 的孤儿快照，实际：{findings:?}"
        );
        let _ = std::fs::remove_dir_all(&base);
    }
}
