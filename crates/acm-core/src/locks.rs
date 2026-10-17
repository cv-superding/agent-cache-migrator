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

use crate::fsutil;
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
    holders_ex(paths).0
}

/// 同 [`holders`]，但额外回答「检测本身可不可用」。
///
/// 🔴 `false`（会话创建失败）时返回的空列表**不等于**「没有占用」——
/// 调用方应当提示用户自行确认，而不是静默放行。这是「明确失败优于静默」的落点。
#[cfg(windows)]
pub fn holders_ex(paths: &[PathBuf]) -> (Vec<Holder>, bool) {
    imp::holders_ex(paths)
}

/// 见 Windows 版说明。
#[cfg(not(windows))]
pub fn holders(_paths: &[PathBuf]) -> Vec<Holder> {
    Vec::new()
}

/// 见 Windows 版说明。非 Windows 没有 Restart Manager：**恒不可用**。
#[cfg(not(windows))]
pub fn holders_ex(_paths: &[PathBuf]) -> (Vec<Holder>, bool) {
    (Vec::new(), false)
}

/// 便利：探测目录（目录本身 + 一级子项）并回答检测可用性。
pub fn holders_ex_dir(dir: &Path, max_entries: usize) -> (Vec<Holder>, bool) {
    holders_ex(&probe_paths(dir, max_entries))
}

/// 构造探测用的路径集合：**目录本身 + 递归子项**，广度优先，最多 `max_entries` 个。
///
/// 🔴 为什么要递归：Restart Manager 注册**目录**时只查该目录本身的句柄，
/// **不会递归查子文件** —— 而应用锁的几乎总是深层文件
/// （如 `User\globalStorage\state.vscdb`、`Cache\Cache_Data\*`）。
/// 只探一级会漏报，探整棵树又太慢 —— 所以 BFS + 上限，
/// 并**跳过联接子目录**（不穿过联接去探测目标盘；迁移中途的 src 尤其不能穿）。
/// 做法对齐 zennos0609-dotcom/AppDataMover 的 `ExpandToFiles`（cap 5000、跳联接）。
pub fn probe_paths(dir: &Path, max_entries: usize) -> Vec<PathBuf> {
    let mut v = vec![dir.to_path_buf()];
    let mut queue = std::collections::VecDeque::new();
    queue.push_back(dir.to_path_buf());
    while v.len() <= max_entries {
        let Some(cur) = queue.pop_front() else { break };
        let Ok(rd) = std::fs::read_dir(&cur) else {
            continue;
        };
        for e in rd.flatten() {
            if v.len() > max_entries {
                break;
            }
            let p = e.path();
            v.push(p.clone());
            // 真实子目录才继续下钻；联接/符号链接不递归
            if p.is_dir() && !fsutil::is_link(&p) {
                queue.push_back(p);
            }
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

    /// 见 [`super::holders_ex`]。只有**会话创建失败**才返回 `false`；
    /// 注册资源失败（比如路径全不存在）当作「没有持有者」处理，检测本身是可用的。
    pub fn holders_ex(paths: &[PathBuf]) -> (Vec<Holder>, bool) {
        if paths.is_empty() {
            return (Vec::new(), true);
        }
        let Some(session) = Session::start() else {
            return (Vec::new(), false); // 🔴 检测不可用 ≠ 没有占用
        };

        // 🔴 RM 的 RmRegisterResources **只接受文件路径** —— 混入目录会让整个
        // 注册调用失败，而我们此前把失败静默当成「没有占用」，导致占用检测
        // 一直在漏报（对齐 zennos0609-dotcom/AppDataMover：ExpandToFiles 只收文件）。
        // 目录句柄（比如正在被删除的目录）RM 本来也看不到，过滤掉没有损失。
        let files: Vec<PathBuf> = paths.iter().filter(|p| p.is_file()).cloned().collect();
        if files.is_empty() {
            return (Vec::new(), true); // 没有任何文件可注册 = 没有文件句柄可锁
        }

        // 宽字符串必须活到 RmRegisterResources 调用结束 —— 所以先收集成 Vec，再取指针。
        let wide: Vec<Vec<u16>> = files.iter().map(|p| wide(&p.to_string_lossy())).collect();
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
        // 注册失败 = 检测结果不可信 —— 如实报告「不可用」，让调用方提示用户
        // 自行确认，而不是静默放行（旧版这里返回 (空, true)，是在静默漏报）。
        if rc != ERROR_SUCCESS {
            return (Vec::new(), false);
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
            return (Vec::new(), true);
        }
        if needed == 0 {
            return (Vec::new(), true); // 没有进程持有 → 可以安全迁移
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
            return (Vec::new(), true);
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
        (out, true)
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

    /// 🔴 回归（参考 AppDataMover 的 ExpandToFiles 后加的）：
    /// 锁一个**二级子文件**， holders_of_dir 必须报出 —— RM 注册目录不递归查子文件，
    /// 探测集合必须自己展开到深层。旧版只探一级，这个场景会漏报。
    #[cfg(windows)]
    #[test]
    fn holders_detect_deep_file_lock() {
        use std::os::windows::fs::OpenOptionsExt;

        let tmp = std::env::temp_dir().join(format!("acm-locks-deep-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let deep = tmp.join("User").join("globalStorage");
        std::fs::create_dir_all(&deep).unwrap();
        let locked = deep.join("state.vscdb");
        std::fs::write(&locked, b"x").unwrap();

        // 自己独占锁住这个二级子文件（share_mode 0 = 不共享任何访问）
        let _guard = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(&locked)
            .expect("独占打开失败");

        let holders = holders_of_dir(&tmp, 2000);
        assert!(
            holders.iter().any(|h| h.pid == std::process::id() as u32),
            "深层文件被独占锁住时必须检测到（自己 pid={}），实际：{holders:?}",
            std::process::id()
        );

        drop(_guard);
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// probe_paths 递归 + 上限 + 跳联接（unix 版，CI 的 Linux job 会跑到）。
    #[cfg(not(windows))]
    #[test]
    fn probe_paths_recurses_with_cap() {
        let tmp = std::env::temp_dir().join(format!("acm-locks-probe2-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(tmp.join("a/b/c")).unwrap();
        std::fs::write(tmp.join("a/b/c/deep.txt"), b"x").unwrap();
        std::fs::write(tmp.join("top.txt"), b"x").unwrap();

        let v = probe_paths(&tmp, 100);
        assert!(
            v.iter().any(|p| p.ends_with("deep.txt")),
            "应当递归到深层：{v:?}"
        );

        // 上限生效
        let capped = probe_paths(&tmp, 2);
        assert_eq!(capped.len(), 3, "目录本身 + 2 个条目");

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn holder_label_is_readable() {
        assert_eq!(
            Holder {
                pid: 42,
                app: "WorkBuddy.exe".into()
            }
            .label(),
            "WorkBuddy.exe (pid 42)"
        );
        assert_eq!(
            Holder {
                pid: 7,
                app: String::new()
            }
            .label(),
            "pid 7"
        );
    }
}
