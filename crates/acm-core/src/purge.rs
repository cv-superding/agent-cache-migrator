//! 清理缓存：把一个 Agent 的数据目录 / 更新器目录**整体移入回收站**。
//!
//! 用途是「卸载残留」：应用早就卸了，但 `%APPDATA%\Xxx` 之类还留着几百 MB；
//! 或者单纯想把这个 Agent 重置回干净状态。清理完再扫一次，它就落到「未安装」。
//!
//! 🔴 三条硬规则：
//! 1. **只动注册表里声明过的路径**（`paths` / `updater`），绝不接受任意路径；
//! 2. **联接一律跳过**：联接背后是真数据，删链接等于把应用的数据「抽走」，
//!    而删联接目标又会连带删掉目标盘上的数据 —— 这种情况该先回滚，不是清理；
//! 3. **走回收站**（`trash` crate），删错了还能从系统回收站捞回来。

use crate::fsutil::{self, EntryKind};
use crate::model::AgentDef;
use crate::paths;
use crate::procs;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// 清理前量体积的上限 —— 比检测宽松（这里量不准就没法报告回收了多少）。
const MEASURE_CAP_FILES: u64 = 2_000_000;
const MEASURE_CAP_SECS: u64 = 90;

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PurgeItem {
    pub path: String,
    /// `数据目录` / `更新器目录`
    pub label: String,
    pub bytes: u64,
    pub files: u64,
    /// 体积是下限（扫描被上限截断）
    pub capped: bool,
    /// `pending`（将被清理）/ `trashed`（已移入回收站）/ `skip`（不动）/ `failed`
    pub action: String,
    pub note: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PurgeReport {
    pub ok: bool,
    pub items: Vec<PurgeItem>,
    /// 移入回收站的字节数（不是真正释放，清空回收站才释放）
    pub freed: u64,
    pub freed_text: String,
    /// 没清成的说明（联接跳过、失败原因等）
    pub problems: Vec<String>,
}

/// 列出「会被清理」的目标 —— **只读**，给确认框用。
pub fn preview(def: &AgentDef) -> Vec<PurgeItem> {
    candidates(def)
        .into_iter()
        .map(|(label, p)| item_for(&p, label))
        .collect()
}

/// 执行清理。
pub fn run(def: &AgentDef) -> Result<PurgeReport, String> {
    // 应用还在跑就别动 —— 文件被占用，删一半会留下更烂的状态
    let snapshot = procs::snapshot();
    let running = procs::running_for(def, &snapshot);
    if !running.is_empty() {
        return Err(format!(
            "{} 还在运行（{}）—— 先完全退出（托盘图标也要退）再清理",
            def.name,
            running.join("、")
        ));
    }

    let mut items = Vec::new();
    let mut problems = Vec::new();

    for (label, p) in candidates(def) {
        let mut it = item_for(&p, label);
        if it.action == "skip" {
            problems.push(it.note.clone().unwrap_or_default());
            items.push(it);
            continue;
        }
        let shown = p.to_string_lossy().to_string();
        match trash::delete(&p) {
            Ok(()) => it.action = "trashed".into(),
            Err(e) => {
                it.action = "failed".into();
                it.note = Some(format!("移入回收站失败：{e}"));
                problems.push(format!("{shown}：移入回收站失败（{e}）"));
            }
        }
        items.push(it);
    }

    if items.is_empty() {
        return Err(format!(
            "{} 没有可清理的目录（可能已经清过了）",
            def.name
        ));
    }

    let freed: u64 = items
        .iter()
        .filter(|i| i.action == "trashed")
        .map(|i| i.bytes)
        .sum();
    Ok(PurgeReport {
        ok: !items.iter().any(|i| i.action == "failed"),
        items,
        freed,
        freed_text: fsutil::human(freed),
        problems,
    })
}

/// 按 id 清理（加载注册表 → 找到 → 执行）。GUI 与 CLI 共用这一条路径。
pub fn purge_by_id(id: &str) -> Result<PurgeReport, String> {
    let defs = crate::agents::load(None)?;
    let def = defs
        .iter()
        .find(|d| d.id == id)
        .ok_or_else(|| format!("注册表里没有 id = {id}"))?;
    run(def)
}

/// 注册表里声明过、且当前**真实存在**的路径。
///
/// 🔴 **每个存在的目录都要清**，包括「只有配置」的小目录：很多应用（豆包就是）
/// 把数据拆在 `%LOCALAPPDATA%`（GB 级真数据）和 `%APPDATA%`（几个配置文件）两处，
/// 只清主目录会留下一份「看着空、其实还在」的残留，扫描时仍然被判成「已安装」。
///
/// 标签上区分主 / 附带，用户一眼能看出「那个 1.89 KB 的也会一起清」。
fn candidates(def: &AgentDef) -> Vec<(&'static str, PathBuf)> {
    let mut out: Vec<(&'static str, PathBuf)> = Vec::new();
    let first_existing = def
        .paths
        .iter()
        .filter_map(|tpl| paths::expand(tpl))
        .find(|p| p.symlink_metadata().is_ok());
    for tpl in &def.paths {
        if let Some(p) = paths::expand(tpl) {
            if p.symlink_metadata().is_ok() {
                let label = if Some(&p) == first_existing.as_ref() {
                    "主数据目录"
                } else {
                    "附带目录（多为配置）"
                };
                out.push((label, p));
            }
        }
    }
    if let Some(local) = paths::local_dir() {
        for u in &def.updater {
            let p = local.join(u);
            if p.symlink_metadata().is_ok() {
                out.push(("更新器目录", p));
            }
        }
    }
    out
}

fn item_for(p: &Path, label: &str) -> PurgeItem {
    let shown = p.to_string_lossy().to_string();
    match fsutil::classify(p) {
        // 联接：绝不碰。要么是迁移留下的（数据在目标盘），要么是坏链
        EntryKind::Link { broken, target } => {
            let note = if broken {
                format!(
                    "{} 是断链联接（指向 {}，已不存在）—— 本次跳过，可用「修复」或手工摘除",
                    shown,
                    target
                        .map(|t| t.to_string_lossy().to_string())
                        .unwrap_or_else(|| "?".into())
                )
            } else {
                format!("{shown} 是联接（已迁移，数据在目标盘）—— 先回滚再清理")
            };
            PurgeItem {
                path: shown,
                label: label.into(),
                bytes: 0,
                files: 0,
                capped: false,
                action: "skip".into(),
                note: Some(note),
            }
        }
        EntryKind::File => {
            let len = std::fs::metadata(p).map(|m| m.len()).unwrap_or(0);
            PurgeItem {
                path: shown,
                label: label.into(),
                bytes: len,
                files: 1,
                capped: false,
                action: "pending".into(),
                note: None,
            }
        }
        EntryKind::Dir => {
            let m = fsutil::measure(
                p,
                MEASURE_CAP_FILES,
                Duration::from_secs(MEASURE_CAP_SECS),
            );
            PurgeItem {
                path: shown,
                label: label.into(),
                bytes: m.bytes,
                files: m.files,
                capped: m.capped,
                action: "pending".into(),
                note: None,
            }
        }
        // candidates() 只放进来的存在项，不会走到这里
        EntryKind::Missing => PurgeItem {
            path: shown,
            label: label.into(),
            bytes: 0,
            files: 0,
            capped: false,
            action: "skip".into(),
            note: Some("路径已经不存在".into()),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn agent(paths_tpl: Vec<String>) -> AgentDef {
        AgentDef {
            id: "t".into(),
            name: "T".into(),
            vendor: None,
            kind: "generic".into(),
            class: "normal".into(),
            category: "agent".into(),
            paths: paths_tpl,
            updater: Vec::new(),
            env_relocate: None,
            launch_args: Vec::new(),
            shortcut_exes: Vec::new(),
            processes: Vec::new(),
            ignore_processes: Vec::new(),
            skip: Vec::new(),
            runtime_links: Vec::new(),
            note: None,
            install: None,
        }
    }

    /// preview 只列真实存在的路径，且对联接一律 skip —— **绝不**把联接当数据删。
    #[test]
    fn preview_lists_existing_dir_and_skips_missing() {
        let tmp = std::env::temp_dir().join("acm-purge-preview");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(tmp.join("data")).unwrap();
        std::fs::write(tmp.join("data/a.bin"), vec![0u8; 100]).unwrap();

        let abs = tmp.join("data").to_string_lossy().to_string();
        let missing = tmp.join("nope").to_string_lossy().to_string();
        let def = agent(vec![abs.clone(), missing]);

        let got = preview(&def);
        assert_eq!(got.len(), 1, "只有真实存在的路径该出现在清单里");
        assert_eq!(got[0].bytes, 100);
        assert_eq!(got[0].files, 1);
        assert_eq!(got[0].action, "pending");

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// 🔴 主目录 + 「只有配置」的小目录**都要进清单**。
    ///
    /// 豆包里就是这样：`%LOCALAPPDATA%\Doubao` 是 3.8 GB 真数据、
    /// `%APPDATA%\Doubao` 只有 1.89 KB 的 public_config.json ——
    /// 只清前者会留下一份「看着空、其实还在」的残留。
    #[test]
    fn preview_includes_config_only_dir_with_its_own_label() {
        let tmp = std::env::temp_dir().join("acm-purge-two-dirs");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(tmp.join("main")).unwrap();
        std::fs::create_dir_all(tmp.join("cfg")).unwrap();
        std::fs::write(tmp.join("main/big.bin"), vec![0u8; 4096]).unwrap();
        std::fs::write(tmp.join("cfg/public_config.json"), b"{}").unwrap();

        let def = agent(vec![
            tmp.join("main").to_string_lossy().to_string(),
            tmp.join("cfg").to_string_lossy().to_string(),
        ]);

        let got = preview(&def);
        assert_eq!(got.len(), 2, "两个存在的目录都该被清理，实际 {got:#?}");
        assert_eq!(got[0].label, "主数据目录");
        assert_eq!(got[1].label, "附带目录（多为配置）");
        assert!(got.iter().all(|i| i.action == "pending"), "都该是待清理");

        let _ = std::fs::remove_dir_all(&tmp);
    }
    #[cfg(windows)]
    #[test]
    fn preview_skips_junction() {
        let tmp = std::env::temp_dir().join("acm-purge-link");
        let _ = std::fs::remove_dir_all(&tmp);
        let target = tmp.join("target");
        let link = tmp.join("link");
        std::fs::create_dir_all(&target).unwrap();
        // 用 cmd 的 mklink /J 建目录联接（不需要管理员权限）
        let st = std::process::Command::new("cmd")
            .args([
                "/C",
                "mklink",
                "/J",
                &link.to_string_lossy(),
                &target.to_string_lossy(),
            ])
            .status();
        if !matches!(st, Ok(s) if s.success()) {
            return; // 建不出来（权限/文件系统不支持）就跳过这条
        }

        let def = agent(vec![link.to_string_lossy().to_string()]);
        let got = preview(&def);
        assert_eq!(got[0].action, "skip", "联接不该被当成可清理的数据");

        let _ = std::fs::remove_dir(&link);
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
