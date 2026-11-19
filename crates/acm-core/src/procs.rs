//! 进程探测：迁移前要确认目标应用没在跑，否则「校验到 0 差异」永远等不到。
//!
//! 🔴 平台分支**签名一致**（见 `fsutil` 顶部说明）。
//!
//! ⚠️ 这里是**兜底**手段（按进程名匹配）。更精确的做法是 Windows 的
//! **Restart Manager API** —— 它直接告诉你「谁锁着这个文件夹」，
//! 而不是靠名字猜。那部分在 `locks.rs` 里实现，这里负责名字匹配与展示。
//!
//! 🔴 **一次快照，多处复用**：`tasklist` 单次要 100~300 ms，而检测是
//! 「每个 Agent 看一遍进程表」。按 Agent 各起一次 `tasklist` 既慢又可观 ——
//! 内置 13 个 Agent 就是 13 个子进程（在 GUI 里还会各弹一个控制台窗口）。
//! 所以拆成 [`snapshot`] + [`matching_in`]：调用方在最外层取一次快照。
//! [`running_matching`] 只用于「本来就只调一次」的场景（如 `plan`）。

/// 列当前进程名（带扩展名）。
///
/// 非 Windows 返回空 —— 调用方据此自然退化成「不检查」，不需要额外的 cfg。
///
/// ⚠️ 这是个**子进程调用**，不要在循环里用；循环请取一次后交给 [`matching_in`]。
#[cfg(windows)]
pub fn snapshot() -> Vec<String> {
    // 🔴 必须走 `syscmd::command`：GUI 进程直接 `Command::new` 会弹控制台窗口。
    let Ok(out) = crate::syscmd::command("tasklist")
        .args(["/FO", "CSV", "/NH"])
        .output()
    else {
        return Vec::new();
    };
    let text = String::from_utf8_lossy(&out.stdout);
    text.lines()
        .filter_map(|l| {
            // "WorkBuddy.exe","1234","Console","1","1,234 K"
            let l = l.trim();
            let first = l.strip_prefix('"')?;
            let end = first.find('"')?;
            Some(first[..end].to_string())
        })
        .collect()
}

/// 见 Windows 版说明。
#[cfg(not(windows))]
pub fn snapshot() -> Vec<String> {
    Vec::new()
}

/// 通配匹配：大小写不敏感，`*` 匹配任意串（可多个）。
pub fn matches(pattern: &str, name: &str) -> bool {
    let p = pattern.to_ascii_lowercase();
    let n = name.to_ascii_lowercase();
    if !p.contains('*') {
        return p == n;
    }
    // 逐段匹配，避免引入正则依赖
    let parts: Vec<&str> = p.split('*').collect();
    let mut rest = n.as_str();
    for (i, part) in parts.iter().enumerate() {
        if part.is_empty() {
            continue;
        }
        match rest.find(part) {
            Some(idx) => {
                // 第一段必须从头匹配
                if i == 0 && idx != 0 {
                    return false;
                }
                rest = &rest[idx + part.len()..];
            }
            None => return false,
        }
    }
    // 最后一段（若模式不以 * 结尾）必须落在末尾
    if !p.ends_with('*') {
        if let Some(last) = parts.last() {
            return n.ends_with(last);
        }
    }
    true
}

/// 在一份**已有的**进程快照里挑出命中这些模式的。
///
/// 这是循环里该用的那个 —— 不碰子进程。
pub fn matching_in(names: &[String], patterns: &[String]) -> Vec<String> {
    if patterns.is_empty() {
        return Vec::new();
    }
    let mut hit: Vec<String> = names
        .iter()
        .filter(|n| patterns.iter().any(|p| matches(p, n)))
        .cloned()
        .collect();
    hit.sort();
    hit.dedup();
    hit
}

/// 快照一次 + 匹配。**只适合调用一次的场景**（如 `plan` / `run` 里查占用）。
///
/// ⚠️ 别在遍历多个 Agent 的循环里用它 —— 那就退化成「每个 Agent 起一次
/// `tasklist`」了（慢，而且 GUI 里会连弹控制台窗口）。循环请自己
/// `let s = procs::snapshot();` 然后反复 `matching_in(&s, ..)`。
pub fn running_matching(patterns: &[String]) -> Vec<String> {
    matching_in(&snapshot(), patterns)
}

/// 某个 Agent「算在跑」的进程 —— 命中 `processes` 且**不在** `ignore_processes` 里。
///
/// 检测、体检、迁移阻断、清理、修复**全部走这一个函数**：口径必须一致，
/// 否则会出现「详情页说没在跑、点迁移却说被占用」这种自相矛盾。
pub fn running_for(def: &crate::model::AgentDef, names: &[String]) -> Vec<String> {
    matching_in(names, &def.processes)
        .into_iter()
        .filter(|n| !def.ignore_processes.iter().any(|p| matches(p, n)))
        .collect()
}

/// 见 [`running_for`]，自己取一次进程快照（只适合调用一次的场景）。
pub fn running_agent(def: &crate::model::AgentDef) -> Vec<String> {
    running_for(def, &snapshot())
}

/// 「结束进程」的结果。
#[derive(Debug, Clone, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KillReport {
    /// 真的发过结束请求的进程名
    pub killed: Vec<String>,
    /// 结束失败 / 拒绝结束的（名字 + 原因）
    pub failed: Vec<KillFailure>,
    /// 结束前就没在跑的（正常现象：用户已经自己关了）
    pub not_running: Vec<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KillFailure {
    pub name: String,
    pub reason: String,
}

/// 结束某个 Agent 的进程。
///
/// - `patterns`：算「应用在跑」的进程（与 `running` 同一套匹配）
/// - `ignore`：**绝不杀**的常驻服务（`ignore_processes`）—— 杀服务可能让应用起不来
///
/// 为什么需要这个功能：`tasklist` 是名字匹配的兜底手段，偶尔会漏（进程刚起、
/// 权限、名字对不上），于是界面上「明明关了还说在跑」。与其让用户去任务管理器
/// 里翻，不如给一个明确目标：「结束这些进程」。
pub fn kill_agent_processes(patterns: &[String], ignore: &[String]) -> KillReport {
    let names = matching_in(&snapshot(), patterns);
    let mut rep = KillReport::default();
    if names.is_empty() {
        rep.not_running.push("（没有匹配到进程）".to_string());
        return rep;
    }
    for name in names {
        if ignore.iter().any(|p| matches(p, &name)) {
            rep.failed.push(KillFailure {
                name,
                reason: "这是后台服务（ignore_processes），不自动结束".to_string(),
            });
            continue;
        }
        match imp_kill::kill_by_name(&name) {
            Ok(()) => rep.killed.push(name),
            Err(e) => rep.failed.push(KillFailure { name, reason: e }),
        }
    }
    rep
}

#[cfg(windows)]
mod imp_kill {
    /// Windows：`taskkill /F /T /IM <name>`（`/T` 连带子进程树）。
    pub fn kill_by_name(name: &str) -> Result<(), String> {
        let out = crate::syscmd::command("taskkill")
            .args(["/F", "/T", "/IM", name])
            .output()
            .map_err(|e| format!("起不了 taskkill：{e}"))?;
        if out.status.success() {
            return Ok(());
        }
        // taskkill 失败时错误在 stdout（中文机器上是 GBK，按 lossy 读够用）
        let msg = String::from_utf8_lossy(&out.stdout).trim().to_string();
        let msg = if msg.is_empty() {
            format!("taskkill 退出码 {}", out.status.code().unwrap_or(-1))
        } else {
            msg
        };
        // 「找不到进程」= 已经退出了，不算失败
        if msg.contains("128") || msg.contains("not found") || msg.contains("没有找到") {
            return Ok(());
        }
        Err(msg)
    }
}

#[cfg(not(windows))]
mod imp_kill {
    pub fn kill_by_name(_name: &str) -> Result<(), String> {
        Err("当前平台不支持一键结束进程（请手动退出应用）".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_and_wildcard_matching() {
        assert!(matches("WorkBuddy.exe", "workbuddy.exe"));
        assert!(matches("WorkBuddy.EXE", "WorkBuddy.exe"));
        assert!(!matches("WorkBuddy.exe", "WorkBuddyAI.exe"));

        // 前缀通配：codex* 应当命中 codex.exe / codex-runner.exe
        assert!(matches("codex*", "codex.exe"));
        assert!(matches("codex*", "codex-runner.exe"));
        assert!(!matches("codex*", "my-codex.exe"), "前导段必须从头匹配");

        // 中间通配
        assert!(matches("DeepSeek*.exe", "DeepSeekDesktop.exe"));
        assert!(matches("*seek*", "mydsepseekx.exe"));

        // 不含通配的 `*` 边界
        assert!(matches("*", "anything.exe"));
    }

    #[test]
    fn empty_patterns_never_match() {
        assert!(running_matching(&[]).is_empty());
        assert!(matching_in(&["a.exe".into()], &[]).is_empty());
    }

    /// `matching_in` 是纯函数：同一份快照可以被多个 Agent 反复查询，
    /// **不再各起一次 `tasklist`**（这是黑窗口与首屏慢的根因）。
    #[test]
    fn matching_in_reuses_one_snapshot() {
        let snap: Vec<String> = ["WorkBuddy.exe", "Codex.exe", "chrome.exe", "node.exe"]
            .iter()
            .map(|s| s.to_string())
            .collect();

        assert_eq!(
            matching_in(&snap, &["WorkBuddy.exe".into(), "WorkBuddyAI.exe".into()]),
            vec!["WorkBuddy.exe".to_string()]
        );
        assert_eq!(
            matching_in(&snap, &["codex*".into()]),
            vec!["Codex.exe".to_string()]
        );
        // 同一份快照可复用，结果互不干扰
        assert!(matching_in(&snap, &["nothere.exe".into()]).is_empty());
        assert_eq!(
            matching_in(&snap, &["*.exe".into()]).len(),
            4,
            "快照里 4 个进程名都以 .exe 结尾"
        );
        // `*e.exe` 是「以 `e.exe` 结尾」—— 只有 chrome/node 符合，WorkBuddy/Codex 不符合
        assert_eq!(matching_in(&snap, &["*e.exe".into()]).len(), 2);

        // 去重：快照里同名进程出现多次只算一个
        let dup: Vec<String> = ["a.exe", "a.exe", "b.exe"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(
            matching_in(&dup, &["*.exe".into()]),
            vec!["a.exe".to_string(), "b.exe".to_string()]
        );
    }

    /// `ignore_processes` 里的常驻服务不能算「应用在跑」——
    /// 否则 Codex 关了还永远显示运行中、迁移被永久拦住。
    #[test]
    fn ignore_processes_drops_background_service() {
        let def: crate::model::AgentDef = toml::from_str(
            r#"
id = "codex"
name = "Codex CLI"
processes = ["codex*"]
ignore_processes = ["codex-windows-sandbox-service.exe"]
"#,
        )
        .unwrap();

        let snap: Vec<String> = [
            "codex.exe",
            "Codex-Windows-Sandbox-Service.exe",
            "explorer.exe",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();

        // 用户真的开着 codex.exe → 算运行中
        assert_eq!(running_for(&def, &snap), vec!["codex.exe".to_string()]);

        // 只剩后台服务（大小写不同也要认出）→ 不算运行中
        let only_service: Vec<String> = ["Codex-Windows-Sandbox-Service.exe"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert!(running_for(&def, &only_service).is_empty());
        // 但它要在 ignored 里露个脸，别装看不见
        assert_eq!(
            matching_in(&only_service, &def.ignore_processes),
            vec!["Codex-Windows-Sandbox-Service.exe".to_string()]
        );
    }
}
