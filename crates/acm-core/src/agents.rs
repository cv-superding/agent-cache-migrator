//! Agent 注册表：加载、合并、检测。
//!
//! 内核**不硬编码任何 Agent** —— 全部来自 `agents.toml`。
//! 内置一份（编译进二进制），用户目录同名文件可覆盖或追加。

use crate::fsutil::{self, EntryKind, Measure};
use crate::model::{AgentDef, BrokenLink, DetectedAgent, PathHit, RegistryFile};
use crate::paths;
use crate::procs;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// 内置注册表（编译进二进制，保证「刚下下来就能用」）。
pub const BUILTIN_TOML: &str = include_str!("../../../agents.toml");

/// 检测选项。
#[derive(Debug, Clone, Copy)]
pub struct DetectOpts {
    /// 是否量体积。关掉的话检测几乎是瞬时的（只做存在性判断）
    pub measure: bool,
    /// 单目录最多数多少个文件
    pub cap_files: u64,
    /// 单目录最多花多少秒
    pub cap_secs: u64,
}

impl Default for DetectOpts {
    fn default() -> Self {
        Self { measure: true, cap_files: 250_000, cap_secs: 20 }
    }
}

/// 用户级注册表的位置：`%APPDATA%/AgentCache/agents.toml`
pub fn user_registry_path() -> Option<PathBuf> {
    Some(paths::roaming_dir()?.join("AgentCache").join("agents.toml"))
}

/// 解析一份注册表文本。
pub fn parse(text: &str) -> Result<Vec<AgentDef>, String> {
    let f: RegistryFile = toml::from_str(text).map_err(|e| format!("agents.toml 解析失败：{e}"))?;
    for a in &f.agents {
        validate(a)?;
    }
    Ok(f.agents)
}

/// 校验单条定义。**只拦真正会出事的**（id 会被用作目录名），其余靠默认值兜。
fn validate(a: &AgentDef) -> Result<(), String> {
    if a.id.trim().is_empty() {
        return Err("有 Agent 缺少 id".into());
    }
    if !a
        .id
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        return Err(format!(
            "id 只能用小写字母/数字/连字符（会被当作目录名）：{}",
            a.id
        ));
    }
    for p in &a.paths {
        if paths::expand(p).is_none() {
            return Err(format!(
                "{} 的路径模板认不出来：{p}（只支持 ~ / %APPDATA% / %LOCALAPPDATA% / %USERPROFILE%）",
                a.id
            ));
        }
    }
    Ok(())
}

/// 加载注册表：内置 + 用户覆盖。
///
/// 合并规则：按 `id` 覆盖（用户的赢），内置里没有的追加在后面。
/// **不改变内置条目的相对顺序**，这样界面顺序稳定。
pub fn load(user: Option<&Path>) -> Result<Vec<AgentDef>, String> {
    let mut out = parse(BUILTIN_TOML)?;

    let path = match user {
        Some(p) => Some(p.to_path_buf()),
        None => user_registry_path().filter(|p| p.is_file()),
    };
    let Some(p) = path.filter(|p| p.is_file()) else {
        return Ok(out);
    };

    let text = std::fs::read_to_string(&p)
        .map_err(|e| format!("读不了 {}：{e}", p.to_string_lossy()))?;
    for custom in parse(&text)? {
        match out.iter_mut().find(|a| a.id == custom.id) {
            Some(slot) => *slot = custom,
            None => out.push(custom),
        }
    }
    Ok(out)
}

/// 检测一个 Agent。
pub fn detect(def: &AgentDef, opts: &DetectOpts) -> DetectedAgent {
    let mut how: Vec<String> = Vec::new();
    let mut hits: Vec<PathHit> = Vec::new();
    let mut total = Measure::default();

    for tpl in &def.paths {
        let Some(resolved) = paths::expand(tpl) else {
            continue; // validate() 已拦过，这里再兜一次
        };
        let kind = fsutil::classify(&resolved);
        let is_link = matches!(kind, EntryKind::Link { .. });
        let link_target = match &kind {
            EntryKind::Link { target, .. } => {
                target.as_ref().map(|p| p.to_string_lossy().to_string())
            }
            _ => None,
        };
        let link_broken = matches!(kind, EntryKind::Link { broken: true, .. });

        let m = if opts.measure && !matches!(kind, EntryKind::Missing | EntryKind::File) {
            fsutil::measure(&resolved, opts.cap_files, Duration::from_secs(opts.cap_secs))
        } else {
            Measure::default()
        };
        total.files += m.files;
        total.bytes += m.bytes;
        total.capped |= m.capped;

        let kind_str = match &kind {
            EntryKind::Missing => "missing",
            EntryKind::File => "file",
            EntryKind::Dir => "dir",
            EntryKind::Link { .. } => "link",
        };
        if !matches!(kind, EntryKind::Missing) {
            how.push(format!("存在 {tpl}"));
        }

        hits.push(PathHit {
            template: tpl.clone(),
            resolved: resolved.to_string_lossy().to_string(),
            kind: kind_str.to_string(),
            is_link,
            link_target,
            link_broken,
            measure: m,
            size_text: size_text(&m, kind_str),
        });
    }

    // 更新器指纹：装过就有，所以数据目录还没生成时它更早能判出来
    let mut updaters = Vec::new();
    if let Some(local) = paths::local_dir() {
        for u in &def.updater {
            let p = local.join(u);
            if p.exists() {
                how.push(format!("更新器目录 {u} 存在"));
                updaters.push(p);
            }
        }
    }

    let broken_links = check_runtime_links(def, &hits);
    let running = procs::running_matching(&def.processes);

    let detected = hits.iter().any(|h| h.kind != "missing") || !updaters.is_empty();

    DetectedAgent {
        id: def.id.clone(),
        name: def.name.clone(),
        vendor: def.vendor.clone(),
        kind: def.kind.clone(),
        class: def.class.clone(),
        note: def.note.clone(),
        env_relocate: def.env_relocate.clone(),
        detected,
        how,
        paths: hits,
        updaters,
        broken_links,
        running,
        total,
    }
}

fn size_text(m: &Measure, kind: &str) -> String {
    if kind == "missing" {
        return "—".into();
    }
    if m.files == 0 && m.bytes == 0 {
        return "空".into();
    }
    let s = fsutil::human(m.bytes);
    if m.capped {
        format!("≥{s}（扫描被上限截断）")
    } else {
        s
    }
}

/// 按 `runtime_links` 模板找出「预期是目录或联接、但已经断链」的路径。
///
/// 这类断链会让应用「解压到临时名 → 改名落地」的准备工作**永久失败**，
/// 于是每次启动都重做一遍。属于必须提前告警的情况。
fn check_runtime_links(def: &AgentDef, hits: &[PathHit]) -> Vec<BrokenLink> {
    let mut out = Vec::new();
    if def.runtime_links.is_empty() {
        return out;
    }
    // 以第一个真实存在的数据目录为基准
    let Some(base) = hits
        .iter()
        .filter(|h| h.kind != "missing")
        .map(|h| PathBuf::from(&h.resolved))
        .next()
    else {
        return out;
    };

    for tpl in &def.runtime_links {
        for p in expand_glob(&base, tpl) {
            if let EntryKind::Link { target, broken: true } = fsutil::classify(&p) {
                out.push(BrokenLink {
                    path: p.to_string_lossy().to_string(),
                    target: target.map(|t| t.to_string_lossy().to_string()),
                });
            }
        }
    }
    out
}

/// 展开带 `*` 的路径模板。
///
/// 🔴 **只支持最后一段是 `*`** —— 刻意不做完整 glob：这个功能只需要
/// 「列出某目录下的所有版本目录」，能力越小越好预测，也少一个依赖。
pub fn expand_glob(base: &Path, pattern: &str) -> Vec<PathBuf> {
    let segs: Vec<&str> = pattern
        .trim_start_matches(['/', '\\'])
        .split(['/', '\\'])
        .filter(|s| !s.is_empty())
        .collect();
    let star_last = segs.last().is_some_and(|s| *s == "*");
    // 中间通配不支持：明确不支持 > 悄悄给错结果
    if segs[..segs.len().saturating_sub(1)].iter().any(|s| *s == "*") {
        return Vec::new();
    }

    let head = if star_last { &segs[..segs.len() - 1] } else { &segs[..] };
    let mut cur = vec![base.to_path_buf()];
    for s in head {
        cur = cur.into_iter().map(|d| d.join(s)).collect();
    }

    if !star_last {
        return cur;
    }
    let mut next = Vec::new();
    for d in &cur {
        let Ok(rd) = std::fs::read_dir(d) else { continue };
        for e in rd.flatten() {
            next.push(e.path());
        }
    }
    next.sort();
    next
}

/// 检测全部（顺序与注册表一致）。
pub fn detect_all(defs: &[AgentDef], opts: &DetectOpts) -> Vec<DetectedAgent> {
    defs.iter().map(|d| detect(d, opts)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_registry_is_valid() {
        let defs = parse(BUILTIN_TOML).expect("内置 agents.toml 必须能解析");
        assert!(defs.len() >= 10, "内置条目太少：{}", defs.len());

        // id 唯一
        let mut ids: Vec<&str> = defs.iter().map(|d| d.id.as_str()).collect();
        ids.sort();
        let n = ids.len();
        ids.dedup();
        assert_eq!(ids.len(), n, "内置注册表有重复 id");

        // class 合法
        for d in &defs {
            assert!(
                ["safe", "normal", "special", "system"].contains(&d.class.as_str()),
                "{} 的 class 非法：{}",
                d.id,
                d.class
            );
        }

        // WorkBuddy 两个版本**必须**不能有 env_relocate（同一个环境变量会让两份数据互踩）
        for d in defs.iter().filter(|d| d.id.starts_with("workbuddy")) {
            assert!(
                d.env_relocate.is_none(),
                "{} 不该提供 env_relocate：环境变量是用户级的，多版本会互相踩数据",
                d.id
            );
        }
    }

    #[test]
    fn detects_workbuddy_entries_on_this_machine_if_present() {
        // 只在真装了的情况下断言，避免把机器状态写进测试
        let defs = parse(BUILTIN_TOML).unwrap();
        let opts = DetectOpts { measure: false, ..Default::default() };
        let all = detect_all(&defs, &opts);
        assert_eq!(all.len(), defs.len());

        for d in &all {
            if !d.detected {
                continue;
            }
            assert!(!d.how.is_empty(), "{} 判为已安装，就该给出依据", d.id);
            assert!(d.paths.iter().any(|p| p.kind != "missing"));
        }
    }

    #[test]
    fn glob_expands_last_segment_only() {
        let tmp = std::env::temp_dir().join("acm-glob-test");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(tmp.join("a").join("v1")).unwrap();
        std::fs::create_dir_all(tmp.join("a").join("v2")).unwrap();
        std::fs::write(tmp.join("a").join("note.txt"), b"x").unwrap();

        let got = expand_glob(&tmp, "a/*");
        assert_eq!(got.len(), 3, "应当列出 v1 / v2 / note.txt");
        assert!(got.iter().any(|p| p.ends_with("v1")));

        // 中间通配刻意不支持 → 空结果（而不是错结果）
        assert!(expand_glob(&tmp, "*/a").is_empty());

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn validate_rejects_bad_id_and_unknown_var() {
        let bad_id = r#"
schema_version = 1
[[agent]]
id = "Bad_ID"
name = "x"
paths = ["~/.x"]
"#;
        assert!(parse(bad_id).is_err(), "大写/下划线的 id 应当被拒");

        let bad_var = r#"
schema_version = 1
[[agent]]
id = "ok"
name = "x"
paths = ["%NOPE%/x"]
"#;
        assert!(parse(bad_var).is_err(), "认不出来的变量应当被拒");
    }

    #[test]
    fn user_registry_overrides_by_id() {
        let dir = std::env::temp_dir().join("acm-registry-merge");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("agents.toml");
        std::fs::write(
            &f,
            r#"
schema_version = 1
[[agent]]
id = "codex"
name = "改过的名字"
paths = ["~/.codex"]
[[agent]]
id = "brand-new"
name = "新加的"
paths = ["~/.brand-new"]
"#,
        )
        .unwrap();

        let merged = load(Some(&f)).unwrap();
        let codex = merged.iter().find(|a| a.id == "codex").unwrap();
        assert_eq!(codex.name, "改过的名字", "同 id 应当被用户配置覆盖");
        assert!(merged.iter().any(|a| a.id == "brand-new"), "新 id 应当追加");

        // 覆盖不该改变内置顺序（界面顺序要稳定）
        let builtin = parse(BUILTIN_TOML).unwrap();
        let pos = |v: &[AgentDef], id: &str| v.iter().position(|a| a.id == id).unwrap();
        assert_eq!(pos(&merged, "codex"), pos(&builtin, "codex"));

        let _ = std::fs::remove_dir_all(&dir);
    }
}
