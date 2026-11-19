//! 应用**是否还装在本机** —— 用来区分「已卸载，只剩缓存」和「装着但没迁移过」。
//!
//! 判定依据只有两样，都是注册表 / 文件系统里的硬事实：
//! 1. `install.uninstall` 声明的卸载清单条目名（Windows：三个 Uninstall 注册表位置）；
//! 2. `install.exe` 声明的可执行文件路径（展开后文件存在 = 装着）。
//!
//! 语义**刻意保守**：
//! - 没配 `install` 的 Agent → `None`（无法判断，界面保持旧行为，绝不瞎猜「已卸载」）；
//! - 配了就代表「能判断」，判否即 `Some(false)` → 界面标「已卸载 · 只剩缓存」并给出清理入口。
//!
//! 🔴 名字匹配规则（不然会误判）：
//! - 大小写不敏感的**全等**；
//! - 或「hint + 空格 + 数字开头」的前缀（`DeepSeek Harness` 能匹配
//!   `DeepSeek Harness 0.2.1-rc.1`，但 `Trae` 不会匹配 `TRAE CN (User)`）。

use std::collections::HashSet;

/// 卸载清单快照（一次扫描，所有 Agent 共用）。
pub struct Snapshot {
    /// 卸载清单里的 DisplayName（原样，匹配时再转小写）
    names: Vec<String>,
    /// `false` = 这台机器上拿不到卸载清单（非 Windows / reg 不可用）→ 一切「未知」
    known: bool,
}

impl Snapshot {
    /// 扫描本机卸载清单。
    pub fn take() -> Self {
        match uninstall_names() {
            Some(names) => Self { names, known: true },
            None => Self {
                names: Vec::new(),
                known: false,
            },
        }
    }

    /// 空快照（测试 / 明确不判断时用）。
    pub fn unknown() -> Self {
        Self {
            names: Vec::new(),
            known: false,
        }
    }

    /// 判断某个 Agent 还装没装。`None` = 判断不了。
    pub fn is_installed(&self, uninstall: &[String], exe: &[String]) -> Option<bool> {
        if uninstall.is_empty() && exe.is_empty() {
            return None;
        }

        // 可执行文件是更硬的证据：文件就在那儿 = 装着
        for tpl in exe {
            if let Some(p) = crate::paths::expand(tpl) {
                if p.exists() {
                    return Some(true);
                }
            }
        }

        if !self.known {
            return None;
        }

        if uninstall
            .iter()
            .any(|hint| self.names.iter().any(|n| matches_display_name(n, hint)))
        {
            return Some(true);
        }

        // 配了线索但两头都空 → 判定「已经不在本机」
        Some(false)
    }
}

/// 名字匹配：全等，或「hint + 空格 + 版本号」。
fn matches_display_name(display: &str, hint: &str) -> bool {
    let d = display.trim().to_lowercase();
    let h = hint.trim().to_lowercase();
    if h.is_empty() {
        return false;
    }
    if d == h {
        return true;
    }
    match d.strip_prefix(&h) {
        Some(rest) => match rest.chars().next() {
            // 必须是「空格 + 数字」——`Trae` 不该命中 `TRAE CN (User)`
            Some(' ') => matches!(rest[1..].chars().next(), Some(c) if c.is_ascii_digit()),
            _ => false,
        },
        None => false,
    }
}

/// 枚举三个 Uninstall 位置里的 DisplayName。
///
/// 用注册表**原生 API**（不是 `reg.exe`）：`reg query` 的输出走控制台代码页，
/// 中文 DisplayName（如「豆包」）读进来会变成乱码，进而把装着的应用误判成已卸载。
/// 原生 API 直接拿 UTF-16，且不用起子进程。
#[cfg(windows)]
fn uninstall_names() -> Option<Vec<String>> {
    use windows_sys::Win32::Foundation::{ERROR_NO_MORE_ITEMS, ERROR_SUCCESS, FILETIME};
    use windows_sys::Win32::System::Registry::{
        RegCloseKey, RegEnumKeyExW, RegGetValueW, RegOpenKeyExW, HKEY, HKEY_CURRENT_USER,
        HKEY_LOCAL_MACHINE, KEY_READ, KEY_WOW64_32KEY, RRF_RT_REG_SZ,
    };

    const SUFFIX: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall";

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// 打开一个根下的 Uninstall 键，逐个读子键的 DisplayName。
    /// 返回 `false` = 这个根打不开（比如 32 位视图不存在）。
    unsafe fn collect(root: HKEY, extra: u32, out: &mut Vec<String>) -> bool {
        let sub = wide(SUFFIX);
        let mut hk: HKEY = 0;
        if RegOpenKeyExW(root, sub.as_ptr(), 0, KEY_READ | extra, &mut hk) != ERROR_SUCCESS {
            return false;
        }

        let mut idx = 0u32;
        loop {
            let mut name = [0u16; 256];
            let mut len = name.len() as u32;
            let rc = RegEnumKeyExW(
                hk,
                idx,
                name.as_mut_ptr(),
                &mut len,
                std::ptr::null_mut::<u32>(),
                std::ptr::null_mut::<u16>(),
                std::ptr::null_mut::<u32>(),
                std::ptr::null_mut::<FILETIME>(),
            );
            if rc == ERROR_NO_MORE_ITEMS {
                break;
            }
            if rc != ERROR_SUCCESS {
                break;
            }
            idx += 1;

            let sub_key: Vec<u16> = wide(&String::from_utf16_lossy(&name[..len as usize]));
            let value = wide("DisplayName");
            let mut buf = [0u16; 512];
            let mut cb = (buf.len() * 2) as u32;
            let rc = RegGetValueW(
                hk,
                sub_key.as_ptr(),
                value.as_ptr(),
                RRF_RT_REG_SZ,
                std::ptr::null_mut::<u32>(),
                buf.as_mut_ptr().cast(),
                &mut cb,
            );
            if rc == ERROR_SUCCESS && cb >= 2 {
                // cb 是字节数（含结尾的 NUL）
                let n = (cb as usize / 2).saturating_sub(1);
                out.push(String::from_utf16_lossy(&buf[..n]));
            }
        }

        RegCloseKey(hk);
        true
    }

    let mut names = Vec::new();
    let mut any_ok = false;
    unsafe {
        any_ok |= collect(HKEY_CURRENT_USER, 0, &mut names);
        any_ok |= collect(HKEY_LOCAL_MACHINE, 0, &mut names);
        // 32 位安装的应用只出现在 WOW6432Node（也就是 32 位视图）
        any_ok |= collect(HKEY_LOCAL_MACHINE, KEY_WOW64_32KEY, &mut names);
    }

    if any_ok {
        // 去重：同一应用可能同时登记在 HKCU 和 HKLM
        let uniq: HashSet<String> = names.into_iter().collect();
        Some(uniq.into_iter().collect())
    } else {
        None
    }
}

#[cfg(not(windows))]
fn uninstall_names() -> Option<Vec<String>> {
    // macOS / Linux 没有统一的卸载清单 —— 老实返回「不知道」，不猜。
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_matching_is_precise() {
        assert!(matches_display_name("DeepSeek Harness 0.2.1-rc.1", "DeepSeek Harness"));
        assert!(matches_display_name("ZCode 3.18.2", "ZCode"));
        assert!(matches_display_name("uTools", "uTools"));
        // 关键反例：Trae 国际版不该被 Trae CN 的卸载项「救活」
        assert!(!matches_display_name("TRAE CN (User)", "Trae"));
        assert!(matches_display_name("TRAE CN (User)", "Trae CN (User)"));
        // 只认「空格 + 数字」，别的后缀都不算
        assert!(!matches_display_name("WorkBuddy AI 5.6.2", "WorkBuddy"));
        assert!(matches_display_name("Ollama version 0.33.2", "Ollama version"));
    }

    /// 没配 install 的 Agent 一定返回 None —— 不能因为「没线索」就判成已卸载
    #[test]
    fn no_hints_means_unknown() {
        let snap = Snapshot::unknown();
        assert_eq!(snap.is_installed(&[], &[]), None);
    }

    /// 配了线索但清单不可得（非 Windows）→ 也返回 None，不误判
    #[test]
    fn unknown_snapshot_never_says_uninstalled() {
        let snap = Snapshot::unknown();
        assert_eq!(snap.is_installed(&["uTools".into()], &[]), None);
    }

    #[test]
    fn exe_hint_wins_when_file_exists() {
        let tmp = std::env::temp_dir().join("acm-install-exe");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        let exe = tmp.join("AgentTest.exe");
        std::fs::write(&exe, b"x").unwrap();
        let snap = Snapshot::unknown();
        assert_eq!(
            snap.is_installed(&[], &[exe.to_string_lossy().to_string()]),
            Some(true)
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// Windows 上能不能真读到卸载清单（读不到就是 `None`，界面退回「判断不了」）
    #[cfg(windows)]
    #[test]
    fn registry_enumeration_reads_unicode_display_names() {
        let Some(names) = uninstall_names() else {
            return; // 三个根全打不开（极少见），不是本测试要覆盖的分支
        };
        assert!(!names.is_empty(), "能打开键就应该读到 DisplayName");
        // 中文 DisplayName 必须能原样读出来 —— 早先用 reg.exe 时这里读成乱码，
        // 结果把「豆包」这类应用误判成已卸载。
        if let Some(zh) = names.iter().find(|n| n.chars().any(|c| c as u32 > 0x2E80)) {
            assert!(
                !zh.contains('\u{FFFD}'),
                "DisplayName 出现替换字符，说明编码处理错了：{zh}"
            );
        }
    }
}
