//! 路径模板展开。
//!
//! `agents.toml` 里写的是 `~/.codex`、`%APPDATA%\Cursor` 这种可读形式，
//! 这里统一展开成绝对路径。**只认几个明确的变量**，不做 shell 展开 ——
//! 配置文件是别人会照着改的，行为必须可预测。

use std::path::PathBuf;

/// 当前用户家目录。
///
/// Windows 优先 `USERPROFILE`；类 Unix 用 `HOME`。
pub fn home_dir() -> Option<PathBuf> {
    let v = if cfg!(windows) {
        std::env::var("USERPROFILE").ok()
    } else {
        std::env::var("HOME").ok()
    };
    non_empty(v).map(PathBuf::from)
}

/// `%APPDATA%`（Windows 漫游目录）。文件系统上就是家目录下的 `AppData/Roaming`。
pub fn roaming_dir() -> Option<PathBuf> {
    non_empty(std::env::var("APPDATA").ok())
        .map(PathBuf::from)
        .or_else(|| home_dir().map(|h| h.join("AppData").join("Roaming")))
}

/// `%LOCALAPPDATA%`。
pub fn local_dir() -> Option<PathBuf> {
    non_empty(std::env::var("LOCALAPPDATA").ok())
        .map(PathBuf::from)
        .or_else(|| home_dir().map(|h| h.join("AppData").join("Local")))
}

fn non_empty(v: Option<String>) -> Option<String> {
    v.map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

/// 展开一个路径模板。
///
/// 支持的前缀：
/// - `~`             → 家目录
/// - `%APPDATA%`     → 漫游目录
/// - `%LOCALAPPDATA%`→ 本地目录
/// - `%USERPROFILE%` → 家目录
///
/// 分隔符正反斜杠都接受（配置文件里写 `/` 更好看），Windows 上统一成 `\`。
/// **认不出来的变量不会被替换**，而是原样返回 `None` —— 宁可明确失败，
/// 也不要静默拼出一个错误的路径去动用户数据。
pub fn expand(template: &str) -> Option<PathBuf> {
    let t = template.trim();
    if t.is_empty() {
        return None;
    }

    let (base, rest) = if let Some(r) = t.strip_prefix('~') {
        (home_dir()?, r)
    } else if let Some(r) = strip_var(t, "APPDATA") {
        (roaming_dir()?, r)
    } else if let Some(r) = strip_var(t, "LOCALAPPDATA") {
        (local_dir()?, r)
    } else if let Some(r) = strip_var(t, "USERPROFILE") {
        (home_dir()?, r)
    } else if t.contains('%') {
        // 未知变量：不猜
        return None;
    } else {
        (PathBuf::new(), t)
    };

    // 去掉紧跟在前缀后面的分隔符，再把剩余的按分隔符逐段拼上去。
    let rest = rest.trim_start_matches(['/', '\\']);
    let mut p = base;
    for seg in rest.split(['/', '\\']).filter(|s| !s.is_empty()) {
        p.push(seg);
    }
    Some(p)
}

/// `%NAME%xxx` → `Some("xxx")`；大小写不敏感。
///
/// 返回值只借用 `s`，与 `name` 无关 —— 所以生命周期要分开写。
fn strip_var<'a>(s: &'a str, name: &str) -> Option<&'a str> {
    let b = s.as_bytes();
    if b.first() != Some(&b'%') {
        return None;
    }
    let close = s[1..].find('%')? + 1;
    if !s[1..close].eq_ignore_ascii_case(name) {
        return None;
    }
    Some(&s[close + 1..])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expand_tilde_and_vars() {
        let home = home_dir().expect("测试环境应当有家目录");

        assert_eq!(expand("~/.codex"), Some(home.join(".codex")));
        // 正反斜杠等价
        assert_eq!(expand("~\\.codex"), Some(home.join(".codex")));
        // 多级
        assert_eq!(
            expand("~/.workbuddy/bash/../x") .map(|p| p.components().count() > 0),
            Some(true),
            "展开不该崩"
        );

        if let (Some(a), Some(b)) = (roaming_dir(), expand("%APPDATA%/Cursor")) {
            assert_eq!(b, a.join("Cursor"));
        }
        if let (Some(a), Some(b)) = (local_dir(), expand("%LOCALAPPDATA%/npm-cache")) {
            assert_eq!(b, a.join("npm-cache"));
        }
        // 大小写不敏感
        if let (Some(a), Some(b)) = (roaming_dir(), expand("%appdata%/Cursor")) {
            assert_eq!(a.join("Cursor"), b);
        }
    }

    #[test]
    fn unknown_var_is_refused_not_guessed() {
        // 认不出来就直接失败 —— 绝不拼一个可能错误的路径去动用户数据
        assert_eq!(expand("%SOMETHING_ELSE%/x"), None);
        assert_eq!(expand(""), None);
        assert_eq!(expand("   "), None);
    }

    #[test]
    fn strip_var_matches_exactly() {
        assert_eq!(strip_var("%APPDATA%/x", "APPDATA"), Some("/x"));
        assert_eq!(strip_var("%APPDATA%", "APPDATA"), Some(""));
        // 前缀相同但变量名不同 → 不能误判
        assert_eq!(strip_var("%APPDATAX%/x", "APPDATA"), None);
        assert_eq!(strip_var("APPDATA/x", "APPDATA"), None);
    }
}
