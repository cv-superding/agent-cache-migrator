//! 用户级环境变量的读取/设置/删除 —— `env_relocate` 的执行端。
//!
//! Windows：写 `HKCU\Environment`（REG_SZ）并广播 `WM_SETTINGCHANGE`，
//! 让资源管理器/新进程立即感知（已运行的进程不受影响 —— 这是 Windows 的机制，
//! 不是本工具能改变的，界面上要说清楚「重启应用生效」）。
//!
//! unix：用户级环境变量靠 shell profile（各 shell 语法/加载时机不同），
//! **不自动改** —— 明确报错，不假装做了。
//!
//! 🔴 这些函数改的是**持久的用户配置**：set 前调用方必须先把原值展示给用户
//! （[`get`]），并告知可用 [`remove`] 撤销。

/// 当前用户级环境变量的值（只认 REG_SZ；不存在返回 None）。
#[cfg(windows)]
pub fn get(name: &str) -> Option<String> {
    imp::get(name)
}

/// 见 Windows 版说明。
#[cfg(not(windows))]
pub fn get(_name: &str) -> Option<String> {
    None
}

/// 设置用户级环境变量（持久；会广播 `WM_SETTINGCHANGE`）。
#[cfg(windows)]
pub fn set(name: &str, value: &str) -> Result<(), String> {
    imp::set(name, value)
}

/// 见 Windows 版说明。
#[cfg(not(windows))]
pub fn set(_name: &str, _value: &str) -> Result<(), String> {
    Err(
        "unix 上不支持自动设置用户级环境变量（各 shell 的 profile 语法不同）\
         —— 请手工加到 ~/.bashrc / ~/.zshrc"
            .to_string(),
    )
}

/// 删除用户级环境变量（恢复应用默认路径）。
#[cfg(windows)]
pub fn remove(name: &str) -> Result<(), String> {
    imp::remove(name)
}

/// 见 Windows 版说明。
#[cfg(not(windows))]
pub fn remove(_name: &str) -> Result<(), String> {
    Err("unix 上不支持自动删除用户级环境变量".to_string())
}

#[cfg(windows)]
mod imp {
    use windows_sys::Win32::Foundation::ERROR_SUCCESS;
    use windows_sys::Win32::System::Registry::{
        RegCloseKey, RegDeleteValueW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW, HKEY,
        HKEY_CURRENT_USER, KEY_QUERY_VALUE, KEY_SET_VALUE, REG_SZ,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        SendMessageTimeoutW, HWND_BROADCAST, SMTO_ABORTIFHUNG, WM_SETTINGCHANGE,
    };

    const ENV_SUBKEY: &str = "Environment";

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn from_wide(buf: &[u16]) -> String {
        let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        String::from_utf16_lossy(&buf[..end])
    }

    /// 打开 `HKCU\Environment`。🔴 这个键是 Windows 建用户的默认项，必然存在，
    /// 用 RegOpenKeyExW 就够（RegCreateKeyExW 还要拉 Win32_Security 依赖）。
    /// 🔴 `HKEY` 在 windows-sys 里是 `isize`，不是指针 —— 别用 null_mut()。
    fn open(sam: u32) -> Result<HKEY, String> {
        let sub = wide(ENV_SUBKEY);
        let mut hkey: HKEY = 0;
        let rc = unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, sub.as_ptr(), 0, sam, &mut hkey) };
        if rc != ERROR_SUCCESS {
            return Err(format!("打开注册表 Environment 失败（代码 {rc}）"));
        }
        Ok(hkey)
    }

    pub fn get(name: &str) -> Option<String> {
        let Ok(hkey) = open(KEY_QUERY_VALUE) else {
            return None;
        };
        let name_w = wide(name);
        // 先问大小再取值 —— 两段式，避免硬编码缓冲区截断长路径
        let mut size: u32 = 0;
        let mut kind: u32 = 0;
        let rc = unsafe {
            RegQueryValueExW(
                hkey,
                name_w.as_ptr(),
                std::ptr::null(),
                &mut kind,
                std::ptr::null_mut(),
                &mut size,
            )
        };
        if rc != ERROR_SUCCESS || kind != REG_SZ {
            unsafe { RegCloseKey(hkey) };
            return None;
        }
        let mut buf = vec![0u16; (size as usize).max(2) / 2 + 1];
        let rc = unsafe {
            RegQueryValueExW(
                hkey,
                name_w.as_ptr(),
                std::ptr::null(),
                std::ptr::null_mut(),
                buf.as_mut_ptr() as *mut u8,
                &mut size,
            )
        };
        unsafe { RegCloseKey(hkey) };
        if rc != ERROR_SUCCESS {
            return None;
        }
        Some(from_wide(&buf))
    }

    pub fn set(name: &str, value: &str) -> Result<(), String> {
        let hkey = open(KEY_SET_VALUE)?;
        let name_w = wide(name);
        let mut value_w = wide(value); // REG_SZ 需要自带 NUL 结尾
        let rc = unsafe {
            RegSetValueExW(
                hkey,
                name_w.as_ptr(),
                0,
                REG_SZ,
                value_w.as_mut_ptr() as *const u8,
                (value_w.len() * 2) as u32,
            )
        };
        unsafe { RegCloseKey(hkey) };
        if rc != ERROR_SUCCESS {
            return Err(format!("写入环境变量失败（代码 {rc}）"));
        }
        broadcast();
        Ok(())
    }

    pub fn remove(name: &str) -> Result<(), String> {
        let hkey = open(KEY_SET_VALUE)?;
        let name_w = wide(name);
        let rc = unsafe { RegDeleteValueW(hkey, name_w.as_ptr()) };
        unsafe { RegCloseKey(hkey) };
        if rc != ERROR_SUCCESS {
            return Err(format!("删除环境变量失败（代码 {rc}）"));
        }
        broadcast();
        Ok(())
    }

    /// 广播「环境变量变了」，让资源管理器与之后启动的进程拿到新值。
    fn broadcast() {
        let param = wide("Environment");
        unsafe {
            SendMessageTimeoutW(
                HWND_BROADCAST,
                WM_SETTINGCHANGE,
                0,
                param.as_ptr() as isize,
                SMTO_ABORTIFHUNG,
                1000,
                std::ptr::null_mut(),
            );
        }
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    /// 读写删 roundtrip —— 用测试专属变量名，测完必须清理干净。
    #[test]
    fn set_get_remove_roundtrip() {
        let name = "AC_TEST_ENV_VAR_DO_NOT_USE";
        // 先确保起点干净
        let _ = remove(name);

        assert_eq!(get(name), None, "起点应当不存在");
        set(name, "D:\\test-path").unwrap();
        assert_eq!(get(name).as_deref(), Some("D:\\test-path"));
        // 覆盖写
        set(name, "E:\\other").unwrap();
        assert_eq!(get(name).as_deref(), Some("E:\\other"));
        remove(name).unwrap();
        assert_eq!(get(name), None, "测完必须清理，不留垃圾");
    }
}
