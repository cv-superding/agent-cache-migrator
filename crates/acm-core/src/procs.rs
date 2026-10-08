//! 进程探测：迁移前要确认目标应用没在跑，否则「校验到 0 差异」永远等不到。
//!
//! 🔴 平台分支**签名一致**（见 `fsutil` 顶部说明）。
//!
//! ⚠️ 这里是**兜底**手段（按进程名匹配）。更精确的做法是 Windows 的
//! **Restart Manager API** —— 它直接告诉你「谁锁着这个文件夹」，
//! 而不是靠名字猜。那部分在 `locks.rs` 里实现，这里负责名字匹配与展示。

/// 🔴 `Command` 只被 Windows 版的 [`list_names`]（跑 `tasklist`）用到。
///
/// 不按平台收这个 import 的话，在 mac/linux 上它就是「未使用的导入」——
/// 而这在本仓库会**直接挂 CI**（`-D warnings`），且只在非 Windows 上暴露。
/// 踩过一次，别再删这个 `#[cfg]`。
#[cfg(windows)]
use std::process::Command;

/// 列当前进程名（带扩展名）。
///
/// 非 Windows 返回空 —— 调用方据此自然退化成「不检查」，不需要额外的 cfg。
#[cfg(windows)]
pub fn list_names() -> Vec<String> {
    let Ok(out) = Command::new("tasklist")
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
pub fn list_names() -> Vec<String> {
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

/// 在运行中的进程里挑出命中这些模式的。
pub fn running_matching(patterns: &[String]) -> Vec<String> {
    if patterns.is_empty() {
        return Vec::new();
    }
    let names = list_names();
    let mut hit: Vec<String> = names
        .into_iter()
        .filter(|n| patterns.iter().any(|p| matches(p, n)))
        .collect();
    hit.sort();
    hit.dedup();
    hit
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
    }
}
