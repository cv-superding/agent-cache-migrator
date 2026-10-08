//! 文件系统小工具：重解析点判定、联接目标、体积统计。
//!
//! 🔴 这里所有跟平台有关的函数都**显式 cfg**，并且**两个分支签名完全一致**。
//! 这不是洁癖 —— 只被一个平台用到的私有函数在另一个平台上就是死代码，
//! 而 CI 把警告当错误（`-D warnings`），会让 mac/linux 的构建直接挂掉，
//! 且只在非 Windows 上暴露。踩过。

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Windows 的 `FILE_ATTRIBUTE_REPARSE_POINT`。
#[cfg(windows)]
const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;

/// 这个元数据是不是「重解析点」（Windows 的目录联接/符号链接，类 Unix 的符号链接）。
#[cfg(windows)]
pub fn is_reparse(md: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    md.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

/// 见 Windows 版说明。
#[cfg(not(windows))]
pub fn is_reparse(md: &std::fs::Metadata) -> bool {
    md.file_type().is_symlink()
}

/// 路径本身（不跟随）是不是重解析点。
pub fn is_link(p: &Path) -> bool {
    std::fs::symlink_metadata(p)
        .map(|md| is_reparse(&md))
        .unwrap_or(false)
}

/// 读联接/符号链接的目标。
///
/// Windows 的**目录联接**和符号链接都能读出来（`read_link` 会处理
/// mount point 重解析标签）；读不到就返回 `None`，不当作错误。
pub fn link_target(p: &Path) -> Option<PathBuf> {
    std::fs::read_link(p).ok().map(normalize)
}

/// 去掉 Windows `\\?\` 长路径前缀 —— 联接的目标经常带这个前缀，
/// 直接拿去 `exists()` 会失败，也会让界面显示得很丑。
fn normalize(p: PathBuf) -> PathBuf {
    let s = p.to_string_lossy();
    match s.strip_prefix(r"\\?\") {
        Some(rest) if rest.len() > 2 => PathBuf::from(rest),
        _ => p,
    }
}

/// 一个路径「是什么」。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EntryKind {
    Missing,
    File,
    Dir,
    /// 重解析点。`broken` = 目标已经不存在（**这是重点排查对象**）。
    Link {
        target: Option<PathBuf>,
        broken: bool,
    },
}

/// 判定一个路径，**不跟随**重解析点。
pub fn classify(p: &Path) -> EntryKind {
    let Ok(md) = std::fs::symlink_metadata(p) else {
        return EntryKind::Missing;
    };
    if is_reparse(&md) {
        let target = link_target(p);
        // 目标在不在，决定了这个链接是「可用」还是「断链」
        let broken = match &target {
            Some(t) => !t.exists(),
            None => true,
        };
        return EntryKind::Link { target, broken };
    }
    if md.is_dir() {
        EntryKind::Dir
    } else {
        EntryKind::File
    }
}

/// 统计结果。
///
/// `Serialize` 是必需的：它直接进检测结果的 JSON，界面/CLI 都靠它。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
pub struct Measure {
    pub files: u64,
    pub bytes: u64,
    /// 是否因为达到上限而提前停下 —— 界面要如实说明「这是下限，不是精确值」
    pub capped: bool,
}

/// 统计体积与文件数。
///
/// - **不跟随重解析点**：否则会把目标盘的数据算两遍（例如运行时目录本身就是联接）。
/// - 有上限（文件数 / 耗时）：几十万文件的目录能让界面卡几十秒，
///   而用户当时只需要一个数量级。超了就标 `capped`。
pub fn measure(root: &Path, cap_files: u64, cap: Duration) -> Measure {
    let started = Instant::now();
    let mut m = Measure::default();
    let mut stack = vec![root.to_path_buf()];

    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in rd.flatten() {
            let Ok(md) = e.metadata() else { continue };
            if is_reparse(&md) {
                continue; // 不跟进去
            }
            if md.is_dir() {
                stack.push(e.path());
            } else {
                m.files += 1;
                m.bytes += md.len();
            }
            if m.files > cap_files || started.elapsed() > cap {
                m.capped = true;
                return m;
            }
        }
    }
    m
}

/// 人类可读的体积。
pub fn human(bytes: u64) -> String {
    const U: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = bytes as f64;
    let mut i = 0;
    while v >= 1024.0 && i < U.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{bytes} B")
    } else {
        format!("{v:.2} {}", U[i])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_missing_and_file_and_dir() {
        let tmp = std::env::temp_dir().join("acm-fsutil-test");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        std::fs::write(tmp.join("a.txt"), b"hello").unwrap();

        assert_eq!(classify(&tmp), EntryKind::Dir);
        assert_eq!(classify(&tmp.join("a.txt")), EntryKind::File);
        assert_eq!(classify(&tmp.join("nope")), EntryKind::Missing);
        assert!(!is_link(&tmp));

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn measure_counts_files_and_bytes_without_following_links() {
        let tmp = std::env::temp_dir().join("acm-fsutil-measure");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(tmp.join("sub")).unwrap();
        std::fs::write(tmp.join("a.bin"), vec![0u8; 1000]).unwrap();
        std::fs::write(tmp.join("sub/b.bin"), vec![0u8; 24]).unwrap();

        let m = measure(&tmp, 10_000, Duration::from_secs(5));
        assert_eq!(m.files, 2);
        assert_eq!(m.bytes, 1024);
        assert!(!m.capped);

        // 上限要能生效
        let c = measure(&tmp, 1, Duration::from_secs(5));
        assert!(c.capped, "超过文件数上限应当标 capped");

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn human_formats() {
        assert_eq!(human(0), "0 B");
        assert_eq!(human(1024), "1.00 KB");
        assert_eq!(human(1024 * 1024 * 1024), "1.00 GB");
    }

    #[test]
    fn normalize_strips_long_path_prefix() {
        assert_eq!(
            normalize(PathBuf::from(r"\\?\C:\x\y")),
            PathBuf::from(r"C:\x\y")
        );
        assert_eq!(normalize(PathBuf::from(r"C:\x")), PathBuf::from(r"C:\x"));
    }
}
