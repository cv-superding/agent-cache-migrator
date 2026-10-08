//! Windows **Restart Manager** —— 精确回答「哪些进程锁着这些路径」。
//!
//! 为什么需要它：`procs.rs` 是**按进程名匹配**的兜底手段，只能回答「这个 exe 在不在跑」。
//! 实际迁移时遇到的经常是反过来的问题 —— *「我明明关掉窗口了，为什么还说被占用」*，
//! 或者应用换了 exe 名（同一份安装里两个 `*buddy.exe`）导致名字匹配漏判。
//!
//! Restart Manager（Windows 自带，装软件时那个「以下程序正在使用文件」就是它）能直接
//! 给出**持有这些路径的进程**，不需要猜名字。
//!
//! 🔴 平台分支**签名一致**（见 `fsutil` 顶部说明）：非 Windows 返回空，调用方自然退化成
//! 「不做占用检查」，不需要在每个调用点写 cfg。

use std::path::{Path, PathBuf};

/// 一个持有者进程。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Holder {
    pub pid: u32,
    /// 应用名（形如 `WorkBuddy.exe`）。拿不到就是空串。
    pub app: String,
}

impl Holder {
    /// 给界面/日志用的一行描述。
    pub fn label(&self) -> String {
        if self.app.is_empty() {
            format!("pid {}", self.pid)
        } else {
            format!("{} (pid {})", self.app, self.pid)
        }
    }
}

/// 这些路径（文件或目录）当前被哪些进程持有。
///
/// 路径不存在的会被系统忽略，不报错 —— 调用方不必先过滤。
#[cfg(windows)]
pub fn holders(paths: &[PathBuf]) -> Vec<Holder> {
    imp::holders(paths)
}

/// 见 Windows 版说明。
#[cfg(not(windows))]
pub fn holders(_paths: &[PathBuf]) -> Vec<Holder> {
    Vec::new()
}

/// 构造探测用的路径集合：**目录本身 + 直接子项**，最多 `max_entries` 个。
///
/// 为什么不递归整棵树：几十万文件逐个注册给 Restart Manager 要几十秒，
/// 而「应用是否在跑」用顶层条目就够准 —— 应用锁的通常是 `SingletonLock` /
/// `Cookies` / `*.db-shm` 这类位于顶层或一级子目录的东西。
pub fn probe_paths(dir: &Path, max_entries: usize) -> Vec<PathBuf> {
    let mut v = vec![dir.to_path_buf()];
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten().take(max_entries) {
            v.push(e.path());
        }
    }
    v
}

/// 便利：直接探测一个目录。
pub fn holders_of_dir(dir: &Path, max_entries: usize) -> Vec<Holder> {
    holders(&probe_paths(dir, max_entries))
}

#[cfg(windows)]
mod imp {
    use super::Holder;
    use std::path::PathBuf;
    use windows_sys::Win32::Foundation::{ERROR_MORE_DATA, ERROR_SUCCESS};
    use windows_sys::Win32::System::RestartManager::{
        RmEndSession, RmGetList, RmRegisterResources, RmStartSession, RM_PROCESS_INFO,
    };

    /// `CCH_RM_SESSION_KEY`；缓冲区要 +1 放下结尾的 NUL。
    const SESSION_KEY_LEN: usize = 32;

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn from_wide(buf: &[u16]) -> String {
        let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        String::from_utf16_lossy(&buf[..end])
    }

    /// 会话句柄的 RAII 包装 —— 中间任何一步提前返回都要关会话，否则泄漏。
    struct Session(u32);

    impl Session {
        fn start() -> Option<Self> {
            let mut key = vec![0u16; SESSION_KEY_LEN + 1];
            let mut handle: u32 = 0;
            // 第三个参数是 `PWSTR`（*mut u16），key 必须可写。
            let rc = unsafe { RmStartSession(&mut handle, 0, key.as_mut_ptr()) };
            (rc == ERROR_SUCCESS).then_some(Self(handle))
        }
    }

    impl Drop for Session {
        fn drop(&mut self) {
            unsafe { RmEndSession(self.0) };
        }
    }

    pub fn holders(paths: &[PathBuf]) -> Vec<Holder> {
        if paths.is_empty() {
            return Vec::new();
        }
        let Some(session) = Session::start() else {
            return Vec::new();
        };

        // 宽字符串必须活到 RmRegisterResources 调用结束 —— 所以先收集成 Vec，再取指针。
        let wide: Vec<Vec<u16>> = paths
            .iter()
            .map(|p| wide(&p.to_string_lossy()))
            .collect();
        let ptrs: Vec<*const u16> = wide.iter().map(|w| w.as_ptr()).collect();

        let rc = unsafe {
            RmRegisterResources(
                session.0,
                ptrs.len() as u32,
                ptrs.as_ptr(),
                0,
                std::ptr::null(),
                0,
                std::ptr::null(),
            )
        };
        // 注册失败（路径全不存在等）＝ 没有持有者，静默返回空。
        if rc != ERROR_SUCCESS {
            return Vec::new();
        }

        let mut reasons: u32 = 0;
        let mut needed: u32 = 0;
        let mut count: u32 = 0;

        // 第一次问「需要多少个」——返回 ERROR_MORE_DATA 是**正常**的。
        let rc = unsafe {
            RmGetList(
                session.0,
                &mut needed,
                &mut count,
                std::ptr::null_mut(),
                &mut reasons,
            )
        };
        if rc != ERROR_SUCCESS && rc != ERROR_MORE_DATA {
            return Vec::new();
        }
        if needed == 0 {
            return Vec::new(); // 没有进程持有 → 可以安全迁移
        }

        let mut infos: Vec<RM_PROCESS_INFO> = vec![unsafe { std::mem::zeroed() }; needed as usize];
        count = needed;
        let rc = unsafe {
            RmGetList(
                session.0,
                &mut needed,
                &mut count,
                infos.as_mut_ptr(),
                &mut reasons,
            )
        };
        if rc != ERROR_SUCCESS {
            return Vec::new();
        }

        let mut out: Vec<Holder> = infos
            .iter()
            .take(count as usize)
            .map(|i| Holder {
                pid: i.Process.dwProcessId,
                app: from_wide(&i.strAppName),
            })
            .collect();
        out.sort_by(|a, b| (a.pid, &a.app).cmp(&(b.pid, &b.app)));
        out.dedup();
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_paths_includes_dir_and_children() {
        let tmp = std::env::temp_dir().join("acm-locks-probe");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        std::fs::write(tmp.join("a.txt"), b"x").unwrap();
        std::fs::write(tmp.join("b.txt"), b"x").unwrap();

        let v = probe_paths(&tmp, 100);
        assert_eq!(v.len(), 3, "应当是目录 + 两个子项");
        assert_eq!(v[0], tmp);

        // 上限要生效（不含目录本身）
        let capped = probe_paths(&tmp, 1);
        assert_eq!(capped.len(), 2);

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn holders_on_empty_input_is_empty() {
        assert!(holders(&[]).is_empty());
    }

    #[test]
    fn holder_label_is_readable() {
        assert_eq!(
            Holder { pid: 42, app: "WorkBuddy.exe".into() }.label(),
            "WorkBuddy.exe (pid 42)"
        );
        assert_eq!(Holder { pid: 7, app: String::new() }.label(), "pid 7");
    }
}
