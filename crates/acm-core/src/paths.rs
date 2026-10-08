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

/// `%APPDATA%` —— Windows 的漫游目录，同时**兼任 Electron 应用 userData 的跨平台等价物**。
///
/// ⚠️ 这个名字来自 Windows，但 `agents.toml` 里大量路径是按 Windows 写的
/// （`%APPDATA%\Cursor`），而 Electron 的 `app.getPath('userData')` 各平台落点不同：
///
/// | 平台 | userData |
/// |---|---|
/// | Windows | `%APPDATA%\<app>`（= `AppData/Roaming`） |
/// | macOS | `~/Library/Application Support/<app>` |
/// | Linux | `~/.config/<app>`（跟随 `XDG_CONFIG_HOME`） |
///
/// 所以这里做的是**等价映射**，让同一份配置在三平台上都指向应用真正用的位置。
/// 不做这一步的话，`%APPDATA%` 在 Linux 上会落到 `~/AppData/Roaming` ——
/// **一个根本不存在的地方**，检测和迁移都会静默找错目录。
pub fn roaming_dir() -> Option<PathBuf> {
    // Windows：环境变量说了算
    if let Some(v) = non_empty(std::env::var("APPDATA").ok()) {
        return Some(PathBuf::from(v));
    }
    if cfg!(target_os = "macos") {
        return home_dir().map(|h| h.join("Library").join("Application Support"));
    }
    // Linux / 其它 unix：Electron 跟的是 XDG_CONFIG_HOME
    if let Some(v) = non_empty(std::env::var("XDG_CONFIG_HOME").ok()) {
        return Some(PathBuf::from(v));
    }
    home_dir().map(|h| h.join(".config"))
}

/// `%LOCALAPPDATA%` —— Windows 上存**不漫游**的本地数据。
///
/// unix 没有这个区分，映射到各自惯用的「本地数据」位置。
/// （`agents.toml` 里主要给 `npm-cache` 之类用，且通常还有一条 unix 原生路径兜底。）
pub fn local_dir() -> Option<PathBuf> {
    if let Some(v) = non_empty(std::env::var("LOCALAPPDATA").ok()) {
        return Some(PathBuf::from(v));
    }
    if cfg!(target_os = "macos") {
        // macOS 的 Electron 也把 userData 放在 Application Support 下
        return home_dir().map(|h| h.join("Library").join("Application Support"));
    }
    if let Some(v) = non_empty(std::env::var("XDG_DATA_HOME").ok()) {
        return Some(PathBuf::from(v));
    }
    home_dir().map(|h| h.join(".local").join("share"))
}

fn non_empty(v: Option<String>) -> Option<String> {
    v.map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

/// 展开一个路径模板。
///
/// 支持的前缀：
/// - `~`              → 家目录
/// - `%APPDATA%`      → 配置/漫游目录（**跨平台等价映射**，见 `roaming_dir`）
/// - `%LOCALAPPDATA%` → 本地数据目录（见 `local_dir`）
/// - `%USERPROFILE%`  → 家目录
///
/// 🔴 `%APPDATA%` / `%LOCALAPPDATA%` 是 **Windows 风格的名字，但含义是跨平台的** ——
/// 它们映射到各平台「应用放 userData 的那个位置」。这样 `agents.toml` 里
/// 一份 `%APPDATA%\Cursor` 在 Windows / macOS / Linux 上都能落到对的目录
/// （Electron 的 `app.getPath('userData')` 各平台落点不同）。
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
            expand("~/.workbuddy/bash/../x").map(|p| p.components().count() > 0),
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

    /// 🔴 这条在 Linux / macOS 的 CI 上会真跑。
    ///
    /// 回归背景：早先 `roaming_dir()` 在非 Windows 上直接拼 `~/AppData/Roaming` ——
    /// 那是**一个根本不存在的地方**，于是 `agents.toml` 里所有
    /// `%APPDATA%\Xxx` 条目在 mac/linux 上都会静默指到错目录。
    #[test]
    fn appdata_maps_to_platform_native_location() {
        if cfg!(windows) {
            return; // Windows 上由环境变量决定，不必断言
        }
        let r = roaming_dir().expect("应当能定位配置目录");
        let s = r.to_string_lossy().replace('\\', "/");
        assert!(
            !s.contains("AppData"),
            "非 Windows 上不该出现 Windows 风格的 AppData 路径，实际是 {s}"
        );
        if cfg!(target_os = "macos") {
            assert!(s.ends_with("Library/Application Support"), "实际是 {s}");
        } else {
            assert!(s.contains(".config"), "实际是 {s}");
        }

        // %APPDATA% 展开的结果必须带上子目录
        let c = expand("%APPDATA%/Cursor").expect("应当能展开");
        assert_eq!(c, r.join("Cursor"));
        assert!(
            !c.to_string_lossy().contains("AppData"),
            "展开结果不该含 AppData：{}",
            c.display()
        );
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
