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
}
