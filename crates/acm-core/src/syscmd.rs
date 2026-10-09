//! 子进程的**统一入口**。
//!
//! 所有对外部命令的调用都必须从这里构造。理由只有一个：**Windows 上要关掉
//! 子进程的控制台窗口**。
//!
//! 🔴 本程序在 Windows 上是 GUI 子系统（`windows_subsystem = "windows"`，
//! 见 `src-tauri/src/main.rs`）。这种进程自己没有控制台 —— 于是它调用一个
//! **控制台程序**（`tasklist` / `robocopy` / `df` …）时，Windows 会**为子进程
//! 新建一个控制台窗口**。用户看到的就是一串黑框不停闪。
//!
//! 这跟「检测做了什么」无关，纯粹是子进程的控制台没关。修法只有一个：
//! 创建子进程时带上 `CREATE_NO_WINDOW`。
//!
//! ⚠️ **别用别名的理由**：`robocopy` 加了、`tasklist` 漏了，结果一启动就弹
//! 十几个窗口。为了不再漏，unix 侧的调用（`df`）**也从这里起手** —— 虽然
//! 那边本来就不弹窗，但统一入口之后，CI 能用一条 grep 拦死所有旁路
//! （见 `.github/workflows/ci.yml` 的「子进程入口检查」）。

use std::process::Command;

/// `CREATE_NO_WINDOW`：不为子进程新建控制台窗口。
///
/// 为什么不改用 `DETACHED_PROCESS`：后者让子进程彻底脱离控制台，
/// 一些依赖 stdout 的程序行为会变；`CREATE_NO_WINDOW` 只是「不给窗口」，
/// 管道照常可用 —— 正是我们需要的。
#[cfg(windows)]
pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// 建一个子进程命令（Windows 上**不会弹控制台窗口**）。
///
/// 🔴 两个平台**签名一致**（见 `fsutil` 顶部说明）：非 Windows 上就是普通的
/// `Command`，所以调用方不需要写 `cfg`。
#[cfg(windows)]
pub fn command(program: &str) -> Command {
    use std::os::windows::process::CommandExt;
    let mut cmd = Command::new(program);
    cmd.creation_flags(CREATE_NO_WINDOW);
    cmd
}

/// 见 Windows 版说明。
#[cfg(not(windows))]
pub fn command(program: &str) -> Command {
    Command::new(program)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 起一个真实子进程并拿到输出 —— 证明「关掉窗口」不影响管道读写。
    /// （CI 的 Linux job 上跑的是非 Windows 分支，同样能验证签名与行为一致。）
    #[test]
    fn command_captures_output_normally() {
        #[cfg(windows)]
        let out = command("cmd").args(["/C", "echo", "hi"]).output();
        #[cfg(not(windows))]
        let out = command("echo").arg("hi").output();

        let out = out.expect("应当能起子进程");
        assert!(
            String::from_utf8_lossy(&out.stdout).contains("hi"),
            "stdout 应能被正常捕获（CREATE_NO_WINDOW 不能影响管道）"
        );
    }
}
