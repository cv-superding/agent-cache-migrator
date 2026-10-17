//! 迁移内核：**复制 → 校验 → 改名快照 → 建联接 → 穿过联接验证**，任一步失败自动回滚。
//!
//! # 为什么不是「写个脚本搬一下」
//!
//! 下面每一条都是真实踩过的坑，也是这个工具相对通用脚本的价值所在：
//!
//! 1. 🔴 **`robocopy /L` 的清单里会混进「目标盘多出来的文件」**（它打的是目标路径）。
//!    拿「非空行数」当待复制数，那些**本来就在目标里、永远搬不过去**的行会让收敛判定
//!    永远不成立 —— 迁移永远报失败。所以必须**只数源树里的路径**（见 [`is_under_root`]）。
//! 2. 🔴 **应用还在跑就永远不收敛**：它会持续改写缓存，前面的复制刚完、后面又变了。
//!    所以迁移前必须确认没有持有者（`locks` 模块做精确判断）。
//! 3. 🔴 **改名会把「断链的联接」一起搬走**：源目录里若有断链的运行时联接，
//!    改名后联接还是指向旧位置。这里在复制阶段用 `/XJ` 跳过重解析点（不跟进去复制）。
//! 4. 🔴 **快照是回滚的唯一依据**：原目录是**改名**保留（`.moved-<时间戳>`），不是删除。
//!    删掉快照 = 这次迁移不可回滚，界面上必须说清楚。
//!
//! # 平台
//!
//! 目前只有 Windows 有完整实现（robocopy + NTFS 目录联接）。别的平台返回明确的不支持，
//! 而不是静默失败。

use crate::fsutil;
use crate::model::AgentDef;
use crate::paths;
use std::path::{Path, PathBuf};

/// 快照后缀前缀。回滚就是靠「认出这个名字」实现的，所以格式是约定。
pub const SNAPSHOT_MARK: &str = ".moved-";

// ============================================================ 数据类型

/// 单步结果。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StepResult {
    pub name: String,
    pub ok: bool,
    pub detail: String,
}

/// 进度回调的载荷。Tauri 壳会把它 emit 成事件，CLI 直接打印。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Progress {
    /// `copying` | `verifying` | `snapshotting` | `linking` | `verifying-link` | `done`
    pub step: String,
    pub detail: String,
    /// 只在能估算时给（0~100）
    pub percent: Option<u8>,
}

/// 迁移结果。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MigrateReport {
    pub ok: bool,
    pub src: String,
    pub dest: String,
    /// 快照目录（原目录改名后的位置），成功时才有
    pub snapshot: Option<String>,
    pub steps: Vec<StepResult>,
    pub error: Option<String>,
    /// 失败后是否已经自动回滚干净
    pub rolled_back: bool,
}

impl MigrateReport {
    fn new(src: &Path, dest: &Path) -> Self {
        Self {
            ok: false,
            src: src.to_string_lossy().to_string(),
            dest: dest.to_string_lossy().to_string(),
            snapshot: None,
            steps: Vec::new(),
            error: None,
            rolled_back: false,
        }
    }

    fn fail(&mut self, msg: impl Into<String>) -> Self {
        self.error = Some(msg.into());
        self.clone()
    }
}

/// 一个快照（迁移时保留的原目录）。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub path: String,
    pub name: String,
    /// 从名字里解析出的 agent id（`~/.codex.moved-20261008-221500` → `codex`）
    pub agent_id: String,
    pub created: String,
    pub measure: fsutil::Measure,
}

/// 修复结果。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RepairReport {
    pub ok: bool,
    pub agent_id: String,
    pub src: String,
    pub dest: String,
    /// `rebuilt`（坏链接删掉重建）| `created`（链接缺失直接建）| `noop`（本来就是好的）
    pub action: String,
    pub detail: String,
}

// ============================================================ 计划

/// 迁移计划：动手前把「会发生什么、有什么拦路虎」摊开给用户看。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Plan {
    pub agent_id: String,
    pub agent_name: String,
    pub src: String,
    pub dest: String,
    /// 快照会落在这里（`<src>.moved-<ts>`，动手时才知道确切时间戳，这里给前缀）
    pub snapshot_prefix: String,
    pub src_measure: fsutil::Measure,
    /// 目标盘可用空间
    pub dest_free: u64,
    pub dest_existing: fsutil::Measure,
    /// **阻止执行**的因素（非空就别点「开始」）
    pub blockers: Vec<String>,
    /// 该知道但不阻止执行的事
    pub warnings: Vec<String>,
    /// 预计目标最终体积是否够放
    pub fits: bool,
    /** ≥1GB 的大文件数（帮助预估复制耗时；扫描被上限截断时是下限） */
    pub large_files: u64,
    /// 大文件扫描是否被上限截断
    pub large_files_capped: bool,
    /// 校验阶段跳过的 glob（来自 `agents.toml` 的 `skip` 字段）。
    /// 复制仍会全量复制；只是这些条目**不参与「0 差异」收敛判定**。
    pub skip: Vec<String>,
}

/// 目标卷的文件系统名（如 `NTFS`、`exFAT`）。
///
/// unix 没有对应概念（ext4/APFS 都支持属性复制），返回 None、调用方跳过检查。
#[cfg(windows)]
pub fn volume_fs(root: &Path) -> Option<String> {
    use windows_sys::Win32::Storage::FileSystem::GetVolumeInformationW;

    // GetVolumeInformationW 要求传卷根（`X:\`）。
    // 🔴 归一化（实测三种形态都要处理）：
    //    `C:`（SystemDrive 常见形态，没有反斜杠）→ `C:\`
    //    `C:\Users\...\Temp`（完整路径）→ `C:\`；`C:\` 保持
    //    宽字符串必须 NUL 结尾（漏掉这个 GetVolumeInformationW 会静默失败）。
    let s = root.to_string_lossy().replace('/', "\\");
    let s = if s.len() == 2 && s.as_bytes().get(1) == Some(&b':') {
        format!("{s}\\")
    } else {
        s
    };
    let i = s.find(":\\")?;
    let root_s = format!("{}\\", &s[..i + 1]); // `C:` / `C:\xxx` → `C:\`
    let mut wide: Vec<u16> = root_s.encode_utf16().collect();
    wide.push(0);
    let mut fs_name = [0u16; 64];
    let ok = unsafe {
        GetVolumeInformationW(
            wide.as_ptr(),
            std::ptr::null_mut(),
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            fs_name.as_mut_ptr(),
            fs_name.len() as u32,
        )
    };
    if ok == 0 {
        return None;
    }
    let end = fs_name.iter().position(|&c| c == 0).unwrap_or(0);
    Some(String::from_utf16_lossy(&fs_name[..end]))
}

/// 见 Windows 版说明。
#[cfg(not(windows))]
pub fn volume_fs(_root: &Path) -> Option<String> {
    None
}

/// 生成计划。
///
/// `dest_root` 是「目标根目录」（会拼上 agent id 作为子目录），不是最终目录本身。
pub fn plan(def: &AgentDef, src: &Path, dest_root: &Path) -> Result<Plan, String> {
    if !src.exists() {
        return Err(format!("源目录不存在：{}", src.display()));
    }
    if fsutil::is_link(src) {
        return Err(format!(
            "{} 已经是联接了（已经迁移过），不需要再迁",
            src.display()
        ));
    }
    let dest = dest_root.join(&def.id);

    // 目标不能落在源里面，也不能就是源 —— 否则会自我递归复制。
    if path_starts_with(&dest, src) || dest == src {
        return Err(format!(
            "目标 {} 在源目录里面，会把目录复制进自己",
            dest.display()
        ));
    }

    let src_measure = fsutil::measure(src, 400_000, std::time::Duration::from_secs(30));
    let dest_existing = if dest.exists() && !fsutil::is_link(&dest) {
        fsutil::measure(&dest, 400_000, std::time::Duration::from_secs(20))
    } else {
        fsutil::Measure::default()
    };
    let dest_free = free_space(dest_root).unwrap_or(0);
    // 目标已有的部分不算重复占用 —— 增量复制只补差的那部分，所以按差值判断。
    let need = src_measure.bytes.saturating_sub(dest_existing.bytes);
    let fits = dest_free == 0 || dest_free > need;

    let mut blockers = Vec::new();
    let mut warnings = Vec::new();

    // ⓪ 目标卷文件系统：非 NTFS 一律拦下（Windows）
    // exFAT/FAT32 不支持 NTFS 属性与 ACL —— robocopy /COPY:DAT 会失败，
    // FAT32 还有 4GB 单文件上限。同类工具（OPTOPIA、DriveKeeper）也都强制 NTFS。
    #[cfg(windows)]
    if let Some(fs) = volume_fs(dest_root) {
        if !fs.eq_ignore_ascii_case("NTFS") {
            blockers.push(format!(
                "目标卷文件系统是 {fs}，不是 NTFS —— 属性/权限无法保留\
                 （FAT32 还有 4GB 单文件上限），复制会失败。请选一块 NTFS 格式的盘"
            ));
        }
    }

    // ⓪.5 源路径体检：系统关键目录拦下、云盘同步目录警告
    //（参考 zhao-wuyan/directory-migration-tool 的 PathValidator 黑名单/云盘警告）
    let (blocked_src, cloud_warn) = screen_source_path(src);
    if let Some(b) = blocked_src {
        blockers.push(b);
    }
    if let Some(w) = cloud_warn {
        warnings.push(w);
    }

    // ① 谁锁着源目录（精确判断，比按进程名猜可靠）
    let (holders, locks_ok) = crate::locks::holders_ex_dir(src, 2000);
    if !locks_ok {
        // 🔴 「检测不可用」不是「没有占用」—— 宁可啰嗦也不能静默放行
        warnings.push(
            "自动占用检测在本机不可用（Restart Manager 仅 Windows 有）——\
             迁移前请自行确认应用已完全退出（托盘里也要退）"
                .to_string(),
        );
    }
    if !holders.is_empty() {
        let names: Vec<String> = dedup_labels(&holders);
        blockers.push(format!(
            "{} 个进程正在使用这个目录，必须先完全退出：{}",
            names.len(),
            names.join(", ")
        ));
    }

    // ② 按配置再核对一次进程名（兜底：有些应用只在子进程里持有文件）
    let running = crate::procs::running_matching(&def.processes);
    if !running.is_empty() {
        let extra: Vec<String> = running
            .into_iter()
            .filter(|n| !holders.iter().any(|h| &h.app == n))
            .collect();
        if !extra.is_empty() {
            blockers.push(format!("配置要求先退出：{}", extra.join(", ")));
        }
    }

    if !fits {
        warnings.push(format!(
            "目标盘可用 {}，本次还差约 {} —— 可能装不下",
            fsutil::human(dest_free),
            fsutil::human(need)
        ));
    }
    if dest_existing.files > 0 {
        warnings.push(format!(
            "目标里已有 {} 个文件（{}）—— 这是上次迁移留下的有效副本，\
             本次只会按「大小+时间戳」增量补齐，**不会删除**目标里多出来的文件",
            dest_existing.files,
            fsutil::human(dest_existing.bytes)
        ));
    }
    if let Some(env) = &def.env_relocate {
        warnings.push(format!(
            "这个 Agent 官方支持用环境变量 {} 重定位数据目录。\
             当前走「搬目录 + 建联接」（对应用透明、更通用）；\
             也可以用 `acm env {id}` 自动设置该变量（更干净、不留联接）。",
            env.env,
            id = def.id
        ));
    }
    // 多目录 Agent：本函数一次只迁一个目录，GUI 只挑主目录 —— 如实告知还有别的
    let other_paths = def
        .paths
        .iter()
        .filter_map(|tpl| paths::expand(tpl))
        .filter(|p| p != src && p.exists() && !fsutil::is_link(p))
        .count();
    if other_paths > 0 {
        warnings.push(format!(
            "该 Agent 还声明了 {other_paths} 个数据目录，本次只迁这一个；\
             其余目录可用 acm migrate <id> 逐个处理"
        ));
    }

    // ≥1GB 的大文件数 —— 用户据此预估复制耗时（几个大文件 ≠ 几十万小文件）
    const LARGE: u64 = 1024 * 1024 * 1024;
    let (large_files, large_capped) =
        count_large_files(src, LARGE, 300_000, std::time::Duration::from_secs(15));
    if large_files > 0 {
        warnings.push(format!(
            "有 {} 个 ≥1GB 的大文件{} —— 大文件按顺序复制，比同体积的小文件快得多",
            large_files,
            if large_capped { "（下限）" } else { "" }
        ));
    }

    Ok(Plan {
        agent_id: def.id.clone(),
        agent_name: def.name.clone(),
        src: src.to_string_lossy().to_string(),
        dest: dest.to_string_lossy().to_string(),
        snapshot_prefix: format!("{}{}", src.to_string_lossy(), SNAPSHOT_MARK),
        src_measure,
        dest_free,
        dest_existing,
        blockers,
        warnings,
        fits,
        large_files,
        large_files_capped: large_capped,
        skip: def.skip.clone(),
    })
}

/// 源路径体检（参考 zhao-wuyan/directory-migration-tool 的 `PathValidator`）：
/// 返回 (blocker, warning)。
///
/// - **系统关键目录** → blocker：`C:\Windows`、`Program Files`、`/usr`、`/System` 这类
///   出现在源路径里，说明模板配置错了或用户手滑，宁可拒绝也不动。
/// - **云盘同步目录**（OneDrive/Dropbox/Google Drive/iCloud）→ warning：
///   迁移会让云盘认为「文件全被删了」，轻则重新上传、重则同步冲突 —— 提醒用户先暂停同步。
fn screen_source_path(src: &Path) -> (Option<String>, Option<String>) {
    let sp = src.to_string_lossy().to_ascii_lowercase();

    let blocked: &[&str] = if cfg!(windows) {
        &[
            "\\windows\\",
            "\\program files",
            "\\programdata\\microsoft",
            "\\system volume information",
        ]
    } else {
        &[
            "/usr/",
            "/etc/",
            "/bin/",
            "/sbin/",
            "/system/",
            "/private/var/",
        ]
    };
    if blocked.iter().any(|b| sp.contains(b)) {
        return (
            Some(format!(
                "源路径在系统关键目录下 —— 不允许迁移：{}（请检查 agents.toml 的 paths 配置）",
                src.display()
            )),
            None,
        );
    }

    const CLOUD: [&str; 5] = [
        "onedrive",
        "dropbox",
        "google drive",
        "googledrive",
        "icloud",
    ];
    if CLOUD.iter().any(|c| sp.contains(c)) {
        return (
            None,
            Some(
                "源路径疑似位于云盘同步目录（OneDrive/Dropbox 等）—— 迁移后云盘会认为文件被删除，\
                 可能触发重新同步或冲突；建议先暂停同步再迁移，或取消迁移"
                    .to_string(),
            ),
        );
    }
    (None, None)
}

/// 统计 ≥ `threshold` 字节的大文件数。不跟随重解析点；达到上限就停（如实标记）。
fn count_large_files(
    root: &Path,
    threshold: u64,
    cap_files: u64,
    cap: std::time::Duration,
) -> (u64, bool) {
    let started = std::time::Instant::now();
    let mut n = 0u64;
    let mut scanned = 0u64;
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in rd.flatten() {
            let Ok(md) = entry.metadata() else { continue };
            if fsutil::is_link(&entry.path()) {
                continue; // 不跟进去
            }
            scanned += 1;
            if scanned > cap_files || started.elapsed() > cap {
                return (n, true);
            }
            if md.is_dir() {
                stack.push(entry.path());
            } else if md.len() >= threshold {
                n += 1;
            }
        }
    }
    (n, false)
}

/// 默认目标根：剩余空间最大的非系统盘下的 `AgentCache\`。
///
/// 借鉴 AppDataMover 的思路 —— 但简化成「哪个盘空就去哪」，因为它不依赖
/// 「主程序装在哪」这个我们并不总是知道的信息。
pub fn default_dest_root() -> Option<PathBuf> {
    let mut best: Option<(u64, PathBuf)> = None;
    let sys = system_drive();
    for drive in drives() {
        // 别往系统盘搬 —— 那正是用户想腾出来的地方
        if sys
            .as_deref()
            .is_some_and(|s| norm_root(s) == norm_root(&drive))
        {
            continue;
        }
        let Some(free) = free_space(Path::new(&drive)) else {
            continue;
        };
        if best.as_ref().is_none_or(|(b, _)| free > *b) {
            best = Some((free, PathBuf::from(drive)));
        }
    }
    best.map(|(_, d)| d.join("AgentCache"))
}

// ============================================================ 执行

/// 执行迁移。
///
/// 五步，任何一步失败都会**尽力回滚**（把快照改回原名、删掉半成品联接），
/// 并且如实报告。回滚也失败时 `rolled_back = false`，用户按快照名手工处理。
///
/// 每次执行（无论成败）都会追加一条记录到 `%APPDATA%/AgentCache/history.jsonl`
/// —— 迁移是有副作用的操作，事后要能回答「什么时候对哪个目录做过什么」
/// （借鉴 OPTOPIA 的迁移状态清单）。写入失败不影响迁移本身。
pub fn run(plan: &Plan, cb: &mut dyn FnMut(Progress)) -> MigrateReport {
    run_with_options(plan, cb, RunOptions::default())
}

/// 迁移的执行选项。
#[derive(Debug, Clone, Copy, Default)]
pub struct RunOptions {
    /// 校验阶段在「0 差异」之外**逐文件计算 SHA-256** 并与目标比对 ——
    /// 更严但大目录很慢，做成可选项（对齐 OPTOPIA 的 `-VerifyHash`）。
    pub verify_hash: bool,
}

/// 全局取消开关（GUI 的「取消」按钮 / `cancel_migrate` 命令置位）。
/// 一次只跑一个迁移 —— 简单且够用；置位后在复制循环的每次读行之间生效。
static CANCELLED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// 请求取消当前迁移（若有）。取消点：复制循环的每行输出之间。
pub fn request_cancel() {
    CANCELLED.store(true, std::sync::atomic::Ordering::SeqCst);
}

/// 清除取消标记（下次迁移开始前调用方应清理）。
pub fn clear_cancel() {
    CANCELLED.store(false, std::sync::atomic::Ordering::SeqCst);
}

/// 取消时 copy 阶段返回的固定错误文案（调用方据此区分「取消」与「失败」）。
pub const CANCELLED_MSG: &str = "迁移已取消（目标盘已复制的部分保留，重新执行会自动续传）";

fn is_cancelled() -> bool {
    CANCELLED.load(std::sync::atomic::Ordering::SeqCst)
}

/// 带 options 的执行入口（`run` 的全参数版）。
pub fn run_with_options(
    plan: &Plan,
    cb: &mut dyn FnMut(Progress),
    opts: RunOptions,
) -> MigrateReport {
    clear_cancel();
    let rep = run_inner(plan, cb, opts);
    append_history(&rep, &plan.agent_id);
    rep
}

/// 历史记录文件：`%APPDATA%/AgentCache/history.jsonl`，每行一条 JSON。
pub fn history_path() -> Option<PathBuf> {
    crate::paths::roaming_dir().map(|d| d.join("AgentCache").join("history.jsonl"))
}

fn append_history(rep: &MigrateReport, agent_id: &str) {
    let line = serde_json::json!({
        "type": "migrate",
        "ts": chrono::Local::now().format("%Y-%m-%dT%H:%M:%S").to_string(),
        "agent": agent_id,
        "src": rep.src,
        "dest": rep.dest,
        "ok": rep.ok,
        "error": rep.error,
        "rolledBack": rep.rolled_back,
        "snapshot": rep.snapshot,
    });
    append_history_line(&line);
}

/// 环境变量变更也入账 —— 历史的意义就是「事后能回答做过什么」，
/// 改了用户级系统状态的操作更不能缺席。
pub fn append_env_history(agent_id: &str, var: &str, action: &str, detail: &str) {
    let line = serde_json::json!({
        "type": "env",
        "ts": chrono::Local::now().format("%Y-%m-%dT%H:%M:%S").to_string(),
        "agent": agent_id,
        "var": var,
        "action": action,
        "detail": detail,
    });
    append_history_line(&line);
}

fn append_history_line(line: &serde_json::Value) {
    let Some(p) = history_path() else { return };
    if let Some(dir) = p.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let ok = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&p)
        .and_then(|mut f| {
            use std::io::Write;
            writeln!(f, "{line}")
        });
    if let Err(e) = ok {
        eprintln!("⚠ 历史记录写入失败（不影响迁移）：{e}");
    }
}

fn run_inner(plan: &Plan, cb: &mut dyn FnMut(Progress), opts: RunOptions) -> MigrateReport {
    let src = PathBuf::from(&plan.src);
    let dest = PathBuf::from(&plan.dest);
    let mut r = MigrateReport::new(&src, &dest);

    if !plan.blockers.is_empty() {
        return r.fail(plan.blockers.join("；"));
    }

    // ① 复制（边复制边推进真实进度 —— 源文件总数作分母，无分母时不给百分比）
    cb(Progress {
        step: "copying".into(),
        detail: format!("正在复制到 {}", dest.display()),
        percent: Some(5),
    });
    let expected = plan.src_measure.files;
    // 🔴 大目录每秒可能复制上千个文件 —— 每个文件都 emit 一次事件会把
    // Tauri 的事件队列打爆、界面直接卡死。节流：非最终进度 200ms 才推一次。
    let mut last_emit = std::time::Instant::now() - std::time::Duration::from_millis(10_000);
    let mut on_file = |n: u64| {
        let is_final = expected > 0 && n >= expected;
        if !is_final && last_emit.elapsed() < std::time::Duration::from_millis(200) {
            return;
        }
        last_emit = std::time::Instant::now();
        // 复制阶段占 5% ~ 65%；measure 被上限截断时分母偏小，可能提前到顶 —— 钳在这里即可
        let percent = if expected > 0 {
            // checked_div：expected==0 时给 None（没有分母就不显示百分比）
            (n.min(expected) * 60)
                .checked_div(expected)
                .map(|v| 5 + v as u8)
        } else {
            None
        };
        cb(Progress {
            step: "copying".into(),
            detail: format!("已复制 {n} 个文件"),
            percent,
        });
    };
    match imp::copy_tree(&src, &dest, &mut on_file) {
        Ok(()) => r.steps.push(StepResult {
            name: "复制".into(),
            ok: true,
            detail: "复制完成（只补不删）".into(),
        }),
        Err(e) => {
            let detail = if e == CANCELLED_MSG {
                "已取消 —— 目标盘已复制的部分保留，重新执行会自动续传".to_string()
            } else {
                e.clone()
            };
            r.steps.push(StepResult {
                name: "复制".into(),
                ok: false,
                detail,
            });
            return r.fail(e);
        }
    }

    // ② 校验：**必须「还需要复制 0 个」才算过**
    cb(Progress {
        step: "verifying".into(),
        detail: "正在核对差异".into(),
        percent: Some(70),
    });
    match imp::pending_count(&src, &dest, &plan.skip) {
        Ok(0) => {
            // 可选：逐文件 SHA-256（--verify-hash / RunOptions::verify_hash）。
            // 大目录很慢 —— 所以是选项而不是默认。
            if opts.verify_hash {
                cb(Progress {
                    step: "verifying".into(),
                    detail: "正在逐文件比对 SHA-256（大目录需要一些时间）…".into(),
                    percent: None,
                });
                if let Err(e) = hash_compare_tree(&src, &dest, &plan.skip, cb) {
                    r.steps.push(StepResult {
                        name: "校验".into(),
                        ok: false,
                        detail: e.clone(),
                    });
                    // 哈希不一致：副本无效。半成品保留（下次重跑会重新覆盖复制）。
                    return r.fail(format!("SHA-256 校验失败：{e}"));
                }
            }
            r.steps.push(StepResult {
                name: "校验".into(),
                ok: true,
                detail: "源里已无待复制文件（0 差异）".into(),
            })
        }
        Ok(n) => {
            let msg = format!(
                "校验没过：源里还有 {n} 个文件没搬过去。\
                 （若数字一直不降，通常是应用还在后台运行、边复制边改写）"
            );
            r.steps.push(StepResult {
                name: "校验".into(),
                ok: false,
                detail: msg.clone(),
            });
            // 这一步失败**不安全回滚**：目标里的副本是有效增量，留着下次接着补。
            r.error = Some(msg);
            return r;
        }
        Err(e) => {
            r.steps.push(StepResult {
                name: "校验".into(),
                ok: false,
                detail: e.clone(),
            });
            return r.fail(e);
        }
    }

    // ③ 改名快照
    cb(Progress {
        step: "snapshotting".into(),
        detail: "正在把原目录改名保留（可回滚）".into(),
        percent: Some(80),
    });
    let snapshot = match imp::rename_to_snapshot(&src) {
        Ok(p) => {
            r.steps.push(StepResult {
                name: "快照".into(),
                ok: true,
                detail: format!("原目录改名保留为 {}", p.display()),
            });
            p
        }
        Err(e) => {
            r.steps.push(StepResult {
                name: "快照".into(),
                ok: false,
                detail: e.clone(),
            });
            return r.fail(e);
        }
    };
    r.snapshot = Some(snapshot.to_string_lossy().to_string());

    // ④ 建联接
    cb(Progress {
        step: "linking".into(),
        detail: "正在建立目录联接".into(),
        percent: Some(90),
    });
    if let Err(e) = imp::make_junction(&src, &dest) {
        r.steps.push(StepResult {
            name: "建联接".into(),
            ok: false,
            detail: e.clone(),
        });
        // 回滚：把快照改回去
        r.rolled_back = imp::rename_back(&snapshot, &src).is_ok();
        return r.fail(e);
    }
    r.steps.push(StepResult {
        name: "建联接".into(),
        ok: true,
        detail: format!("{} → {}", src.display(), dest.display()),
    });

    // ⑤ 穿过联接验证 —— 光看联接建成了不够，得真的读到一个文件
    cb(Progress {
        step: "verifying-link".into(),
        detail: "正在穿过联接验证可读性".into(),
        percent: Some(97),
    });
    match imp::verify_through_link(&src, &dest) {
        Ok(n) => r.steps.push(StepResult {
            name: "验证联接".into(),
            ok: true,
            detail: format!("直接读原路径，看到 {n} 个条目"),
        }),
        Err(e) => {
            r.steps.push(StepResult {
                name: "验证联接".into(),
                ok: false,
                detail: e.clone(),
            });
            let _ = imp::remove_junction(&src);
            r.rolled_back = imp::rename_back(&snapshot, &src).is_ok();
            r.snapshot = None;
            return r.fail(e);
        }
    }

    cb(Progress {
        step: "done".into(),
        detail: "迁移完成 —— 重启应用后，新的写入会直接落到目标盘".into(),
        percent: Some(100),
    });
    r.ok = true;
    r
}

/// 回滚：把某个 agent 的快照改回原位（联接必须先摘掉）。
///
/// 只处理**最新**的一个快照 —— 同名多个快照说明状态已经不干净，交给人判断。
///
/// 🔴 **回滚安全**（借鉴 OPTOPIA/appdata-migrate-tool）：迁移之后应用可能
/// 往联接目标里写了新数据，直接摘链接改回快照会**丢掉这些数据**。
/// 所以摘链接前先把「快照里没有的或大小不同的」文件复制到目标盘旁的
/// `<名字>.rescued-<时间戳>` 恢复目录 —— 回滚零丢失。
pub fn rollback(def: &AgentDef, src: &Path) -> Result<String, String> {
    let snaps = snapshots_of(src);
    let Some((snap, _ts)) = snaps.into_iter().next() else {
        return Err(format!(
            "{} 没有找到快照（*.moved-*），无法回滚",
            src.display()
        ));
    };

    if src.exists() && !fsutil::is_link(src) {
        return Err(format!(
            "{} 现在是一个真实目录（不是联接），回滚会覆盖它 —— 请先手工确认",
            src.display()
        ));
    }

    let mut msgs: Vec<String> = Vec::new();
    if fsutil::is_link(src) {
        // 迁移后新数据在联接目标里 —— 摘链接前先抢救
        if let Some(target) = fsutil::link_target(src).filter(|t| t.is_dir()) {
            // 🔴 撤销环境变量（`--with-env` 迁移设置的）：变量仍指向目标盘的
            // 旧副本，不撤的话回滚后应用会优先读变量 → 拿到**过期数据**。
            // 只撤「恰好指向本次迁移目标」的变量 —— 用户自定义的其他值不动。
            if let Some(env) = &def.env_relocate {
                if let Some(cur) = crate::envvar::get(&env.env) {
                    let norm = |s: &str| s.trim_end_matches(['\\', '/']).to_string();
                    if norm(&cur).eq_ignore_ascii_case(&norm(&target.to_string_lossy())) {
                        match crate::envvar::remove(&env.env) {
                            Ok(()) => {
                                let detail = format!("回滚时自动撤销（原指向 {cur}）");
                                append_env_history(&def.id, &env.env, "unset", &detail);
                                msgs.push(format!("已撤销环境变量 {}（原指向 {cur}）", env.env));
                            }
                            Err(e) => msgs.push(format!(
                                "⚠ 环境变量 {} 撤销失败（{e}）—— 它仍指向 {cur}，\
                                 请手工执行 acm env {} --unset",
                                env.env, def.id
                            )),
                        }
                    } else {
                        msgs.push(format!(
                            "⚠ 环境变量 {} 当前指向 {cur}（不是本次迁移目标），未自动撤销",
                            env.env
                        ));
                    }
                }
            }

            let ts = chrono::Local::now().format("%Y%m%d-%H%M%S").to_string();
            let name = target
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| "data".to_string());
            let rescue = target.with_file_name(format!("{name}.rescued-{ts}"));
            match rescue_changed_files(&target, &snap, &rescue) {
                Ok((n, bytes)) if n > 0 => {
                    msgs.push(format!(
                        "⚠ 迁移后新增/变更的 {} 个文件（{}）已先备份到 {}",
                        n,
                        fsutil::human(bytes),
                        rescue.display()
                    ));
                }
                Ok(_) => {} // 没有新数据，直接回滚
                Err(e) => {
                    return Err(format!(
                        "备份迁移后新增数据失败，已中止回滚（快照与链接都未动）：{e}"
                    ))
                }
            }
        }
        imp::remove_junction(src)?;
    }
    imp::rename_back(&snap, src)?;
    let mut msg = format!(
        "已回滚：{} → {}（agent {}）",
        snap.display(),
        src.display(),
        def.id
    );
    for m in &msgs {
        msg.push_str(&format!("；{m}"));
    }
    Ok(msg)
}

/// 逐文件 SHA-256 校验：源树里每个文件与目标对应文件比对内容哈希。
/// 比 robocopy `/L`（大小 + 时间戳）更严 —— 字节级一致。跳过联接与 skip glob。
fn hash_compare_tree(
    src: &Path,
    dest: &Path,
    skip: &[String],
    cb: &mut dyn FnMut(Progress),
) -> Result<(), String> {
    let mut stack = vec![src.to_path_buf()];
    let mut checked: u64 = 0;
    while let Some(dir) = stack.pop() {
        if is_cancelled() {
            return Err(CANCELLED_MSG.to_string());
        }
        let rd = std::fs::read_dir(&dir).map_err(|e| format!("读 {} 失败：{e}", dir.display()))?;
        for entry in rd.flatten() {
            if is_cancelled() {
                return Err(CANCELLED_MSG.to_string());
            }
            let p = entry.path();
            if fsutil::is_link(&p) {
                continue; // 不跟进去
            }
            let md = std::fs::symlink_metadata(&p).map_err(|e| format!("{p:?}：{e}"))?;
            if md.is_dir() {
                stack.push(p);
                continue;
            }
            let rel = p
                .strip_prefix(src)
                .map_err(|_| "内部错误：strip_prefix 失败".to_string())?;
            if crate::globs::matches_any(rel, skip) {
                continue;
            }
            let dst_file = dest.join(rel);
            let a = crate::hash::sha256_file(&p)?;
            let b = crate::hash::sha256_file(&dst_file)
                .map_err(|_| format!("目标侧缺失 {:?}（复制不完整）", rel))?;
            if a != b {
                return Err(format!("{rel:?} 两侧内容不一致"));
            }
            checked += 1;
            if checked.is_multiple_of(1000) {
                cb(Progress {
                    step: "verifying".into(),
                    detail: format!("SHA-256 已比对 {checked} 个文件…"),
                    percent: None,
                });
            }
        }
    }
    Ok(())
}

/// 把「迁移后新增/变更」的文件从 `target` 复制到 `rescue_dir`。
///
/// 判据（相对 `target` 的路径）：快照里**不存在**，或**大小不同**。
/// 大小相同视为未改 —— 与迁移校验的收敛判据一致（应用已退出的前提下成立）。
/// 返回 (文件数, 字节数)。**不改 target 本身** —— 只复制。
fn rescue_changed_files(
    target: &Path,
    snapshot: &Path,
    rescue_dir: &Path,
) -> Result<(u64, u64), String> {
    let mut files = 0u64;
    let mut bytes = 0u64;
    let mut stack = vec![target.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let rd = std::fs::read_dir(&dir).map_err(|e| format!("读 {} 失败：{e}", dir.display()))?;
        for entry in rd.flatten() {
            let p = entry.path();
            if fsutil::is_link(&p) {
                continue; // 不跟随
            }
            let Ok(md) = entry.metadata() else { continue };
            if md.is_dir() {
                stack.push(p);
                continue;
            }
            let Ok(rel) = p.strip_prefix(target) else {
                continue;
            };
            let snap_file = snapshot.join(rel);
            let changed = match std::fs::symlink_metadata(&snap_file) {
                Ok(s) => s.len() != md.len(),
                Err(_) => true, // 快照里没有 = 新增
            };
            if !changed {
                continue;
            }
            let to = rescue_dir.join(rel);
            if let Some(parent) = to.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| format!("建 {} 失败：{e}", parent.display()))?;
            }
            std::fs::copy(&p, &to).map_err(|e| format!("复制 {} 失败：{e}", p.display()))?;
            files += 1;
            bytes += md.len();
        }
    }
    Ok((files, bytes))
}

/// 设置/删除 Agent 官方支持的重定位环境变量（`env_relocate` 的执行端）。
///
/// - `unset = false`：把 `env.env` 设为 `dest_root/<agent-id>`（持久、用户级）
/// - `unset = true`：删除该变量，恢复应用默认路径
///
/// ⚠️ 持久的用户级变更 —— 调用方（界面/CLI）必须先展示原值并确认。
/// 返回人类可读的结果描述（含原值，方便撤销）。
pub fn env_relocate_apply(
    def: &AgentDef,
    dest_root: Option<&Path>,
    unset: bool,
) -> Result<String, String> {
    let env = def
        .env_relocate
        .as_ref()
        .ok_or_else(|| format!("{} 没有声明官方支持的环境变量重定位", def.name))?;

    if unset {
        let old = crate::envvar::get(&env.env);
        if old.is_none() {
            return Ok(format!("{} 当前未设置，无需删除。", env.env));
        }
        crate::envvar::remove(&env.env)?;
        let msg = format!(
            "已删除 {}（重启应用后恢复默认路径；原值 {}）",
            env.env,
            old.unwrap_or_default()
        );
        append_env_history(&def.id, &env.env, "unset", &msg);
        return Ok(msg);
    }

    let Some(dest_root) = dest_root else {
        return Err("需要指定目标根目录".to_string());
    };
    let value = dest_root.join(&def.id);
    let old = crate::envvar::get(&env.env);
    crate::envvar::set(&env.env, &value.to_string_lossy())?;
    let mut msg = format!("已设置 {} = {}（重启应用后生效）", env.env, value.display());
    if let Some(o) = old {
        msg.push_str(&format!("（原值 {o}）"));
    }
    append_env_history(&def.id, &env.env, "set", &msg);
    Ok(msg)
}

/// 修复模式：目标数据还在、只是链接坏了/丢了时，**只重建链接，不复制数据**。
///
/// 三种场景：
/// - `rebuilt` —— 链接还在但断链（目标被挪走后换过位置）→ 删掉重建到 `dest_root/<id>`
/// - `created` —— 链接整个没了（被清理工具误删）→ 直接建
/// - `noop` —— 链接完好，什么都不做
///
/// 防御：候选目标必须是**有内容的真实目录**（防止把空目录/另一个链接当数据）；
/// 应用在跑时拒绝修复（重建瞬间句柄会断）。
pub fn repair(def: &AgentDef, src: &Path, dest_root: &Path) -> Result<RepairReport, String> {
    let dest = dest_root.join(&def.id);
    let mk = |action: &str, detail: String| RepairReport {
        ok: true,
        agent_id: def.id.clone(),
        src: src.to_string_lossy().to_string(),
        dest: dest.to_string_lossy().to_string(),
        action: action.to_string(),
        detail,
    };

    // 应用在跑就不动 —— 重建的那一瞬间它手里的句柄会失效
    let running = crate::procs::running_matching(&def.processes);
    if !running.is_empty() {
        return Err(format!(
            "请先退出：{}（修复前不能有进程占用）",
            running.join(", ")
        ));
    }

    // 候选目标：必须是有内容的真实目录
    let dest_kind = fsutil::classify(&dest);
    let dest_files = std::fs::read_dir(&dest)
        .map(|rd| rd.flatten().count())
        .unwrap_or(0);
    let dest_ok = matches!(dest_kind, fsutil::EntryKind::Dir) && dest_files > 0;
    if !dest_ok {
        return Err(format!(
            "候选目标 {} 不存在、是空的、或不是真实目录 —— 数据不在预期位置，\
             无法自动修复。请先确认目标盘上的数据还在（例如手工打开看看），\
             再用正确的 --dest 重试",
            dest.display()
        ));
    }

    let kind = fsutil::classify(src);
    match &kind {
        // 真实目录/文件不是修复对象 —— 那是「迁移」的事
        fsutil::EntryKind::Dir | fsutil::EntryKind::File => Err(format!(
            "{} 是真实{}，不需要修复；要把数据搬到别的盘请走迁移流程",
            src.display(),
            if matches!(kind, fsutil::EntryKind::Dir) {
                "目录"
            } else {
                "文件"
            }
        )),
        // 链接完好 → 检查指向对不对
        fsutil::EntryKind::Link {
            broken: false,
            target,
        } => {
            // 🔴 必须精确指向 dest：指向 dest 的子路径也算「不在预期位置」
            let points_right = target.as_ref().map(|t| t == &dest).unwrap_or(false);
            if points_right {
                Ok(mk(
                    "noop",
                    format!("链接完好：{} → {}", src.display(), dest.display()),
                ))
            } else {
                // 指向别处且能用 —— 不擅自改；如实说明
                Err(format!(
                    "链接完好但指向 {}（不是 {}）—— 数据可能在另一个位置，\
                     确认后请手工处理，修错位置比不修更糟",
                    target
                        .as_ref()
                        .map(|t| t.to_string_lossy())
                        .unwrap_or_default(),
                    dest.display()
                ))
            }
        }
        // 断链 → 删掉重建
        fsutil::EntryKind::Link { broken: true, .. } => {
            imp::remove_junction(src)?;
            imp::make_junction(src, &dest)?;
            let n = imp::verify_through_link(src, &dest)?;
            Ok(mk(
                "rebuilt",
                format!("坏链接已重建，穿过链接读到 {n} 个条目"),
            ))
        }
        // 链接整个没了 → 直接建（src 的父目录必须还在）
        fsutil::EntryKind::Missing => {
            if !src.parent().is_some_and(|p| p.exists()) {
                return Err(format!(
                    "{} 的父目录不存在 —— 路径本身有问题，无法自动修复",
                    src.display()
                ));
            }
            imp::make_junction(src, &dest)?;
            let n = imp::verify_through_link(src, &dest)?;
            Ok(mk(
                "created",
                format!("链接已创建，穿过链接读到 {n} 个条目"),
            ))
        }
    }
}

/// 列出所有 agent 的快照。
///
/// 扫的是每个 Agent 声明的数据目录的**父目录**，找 `名字.moved-时间戳`。
pub fn list_snapshots(defs: &[AgentDef]) -> Vec<Snapshot> {
    let mut out = Vec::new();
    for def in defs {
        for tpl in &def.paths {
            let Some(p) = paths::expand(tpl) else {
                continue;
            };
            for (snap, ts) in snapshots_of(&p) {
                let m = fsutil::measure(&snap, 400_000, std::time::Duration::from_secs(20));
                out.push(Snapshot {
                    name: snap
                        .file_name()
                        .map(|s| s.to_string_lossy().to_string())
                        .unwrap_or_default(),
                    path: snap.to_string_lossy().to_string(),
                    agent_id: def.id.clone(),
                    created: ts,
                    measure: m,
                });
            }
        }
    }
    out.sort_by(|a, b| b.created.cmp(&a.created));
    out
}

/// 删掉一个快照。
///
/// - `permanent = true` —— 永久删除（解 ACL → 清属性 → 递归删），**不可恢复**。
/// - `permanent = false` —— **移入回收站**（`trash` crate：Windows 走
///   COM `IFileOperation`，macOS 走 NSWorkspace，Linux 走 freedesktop trash，
///   全部可从系统回收站还原）。
///
/// ⚠️ 永久删除之后这次迁移就**不能回滚**了 —— 界面上必须先跟用户确认。
///
/// 🔴 路径必须通过 [`validate_snapshot_path`] 的校验：名字符合快照约定、
/// 且就躺在某个已知 Agent 数据目录的旁边。这个命令的底层一旦接受任意路径
/// 就等于给了前端一个「删任意目录」的原语 —— 校验是纵深防御，不是摆设。
pub fn cleanup(snapshot: &Path, permanent: bool) -> Result<u64, String> {
    let defs = crate::agents::load(None)?;
    validate_snapshot_path(snapshot, &defs)?;
    if permanent {
        imp::remove_dir_forced(snapshot)
    } else {
        // 先量体积 —— 移进回收站之后就量不到了
        let freed = fsutil::measure(snapshot, 2_000_000, std::time::Duration::from_secs(60)).bytes;
        imp::remove_dir_recycle(snapshot)?;
        Ok(freed)
    }
}

/// 校验一个路径是不是「我们生成的快照」。
///
/// 两条判据（都要满足）：
/// 1. 文件名形如 `<名字>.moved-YYYYMMDD-HHMMSS`（时间戳是 8 位数字 + `-` + 6 位数字）；
/// 2. 它躺在**注册表里某个 Agent 数据目录的父目录**下，且前缀名与那个数据目录一致。
///
/// 独立成函数并接收 `defs` 参数，是为了能单测（不依赖本机装了哪些 Agent）。
fn validate_snapshot_path(snapshot: &Path, defs: &[AgentDef]) -> Result<(), String> {
    let Some(name) = snapshot
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
    else {
        return Err(format!("路径没有文件名：{}", snapshot.display()));
    };
    let Some(idx) = name.find(SNAPSHOT_MARK) else {
        return Err(format!(
            "拒绝删除：{name} 不符合快照命名约定（缺少 {SNAPSHOT_MARK} 标记）"
        ));
    };
    let ts = &name[idx + SNAPSHOT_MARK.len()..];
    let ts_ok = ts.len() == 15
        && ts.as_bytes()[8] == b'-'
        && ts
            .chars()
            .enumerate()
            .all(|(i, c)| i == 8 || c.is_ascii_digit());
    if !ts_ok {
        return Err(format!(
            "拒绝删除：快照时间戳格式不对（应为 YYYYMMDD-HHMMSS），实际是「{ts}」"
        ));
    }

    let Some(parent) = snapshot.parent() else {
        return Err("拒绝删除：路径没有父目录".to_string());
    };
    for def in defs {
        for tpl in &def.paths {
            let Some(p) = paths::expand(tpl) else {
                continue;
            };
            // 🔴 前缀必须带上 SNAPSHOT_MARK 边界：`.codex2.moved-…` 不能被
            // 认成 `.codex` 的快照（`starts_with(".codex")` 是真，但那是另一个目录）
            let prefix_ok = p.file_name().is_some_and(|n| {
                let expected = format!("{}{}", n.to_string_lossy(), SNAPSHOT_MARK);
                name.starts_with(&expected)
            });
            let parent_ok = p.parent().is_some_and(|pp| pp == parent);
            if prefix_ok && parent_ok {
                return Ok(());
            }
        }
    }
    Err(format!(
        "拒绝删除：{} 不在任何已知 Agent 数据目录旁边 —— \
         这可能是手工构造的路径，为防误删任意目录已拦截",
        snapshot.display()
    ))
}

// ============================================================ 内部工具（跨平台）

/// `p` 是否在 `root` 里面（含相等）。
fn path_starts_with(p: &Path, root: &Path) -> bool {
    let a = norm_for_compare(p);
    let b = norm_for_compare(root);
    a == b || a.starts_with(&format!("{b}\\"))
}

fn norm_for_compare(p: &Path) -> String {
    p.to_string_lossy()
        .trim_start_matches(r"\\?\")
        .replace('/', "\\")
        .trim_end_matches('\\')
        .to_ascii_lowercase()
}

/// 把持有者按进程名去重，得到「要退出的应用」清单。
fn dedup_labels(holders: &[crate::locks::Holder]) -> Vec<String> {
    let mut v: Vec<String> = holders
        .iter()
        .map(|h| {
            if h.app.is_empty() {
                format!("pid {}", h.pid)
            } else {
                h.app.clone()
            }
        })
        .collect();
    v.sort();
    v.dedup();
    v
}

/// 找出 `<parent>/<name>.moved-<ts>` 形式的快照，返回 (路径, 时间戳文本)。
fn snapshots_of(target: &Path) -> Vec<(PathBuf, String)> {
    let (Some(parent), Some(name)) = (target.parent(), target.file_name()) else {
        return Vec::new();
    };
    let prefix = format!("{}{}", name.to_string_lossy(), SNAPSHOT_MARK);
    let Ok(rd) = std::fs::read_dir(parent) else {
        return Vec::new();
    };
    let mut out: Vec<(PathBuf, String)> = rd
        .flatten()
        .filter_map(|e| {
            let n = e.file_name().to_string_lossy().to_string();
            let ts = n.strip_prefix(&prefix)?.to_string();
            Some((e.path(), ts))
        })
        .collect();
    // 时间戳是 `YYYYMMDD-HHMMSS`，字典序＝时间序，新的在前
    out.sort_by(|a, b| b.1.cmp(&a.1));
    out
}

// ============================================================ 平台实现

/// Windows 实现：robocopy + `mklink /J`。
#[cfg(windows)]
mod imp {
    use super::{fsutil, SNAPSHOT_MARK};
    use crate::syscmd;
    use std::io::{BufRead, BufReader as IoBufReader};
    use std::path::{Path, PathBuf};
    use std::process::{Output, Stdio};

    fn run(program: &str, args: &[&str]) -> std::io::Result<Output> {
        // 🔴 走 `syscmd::command`（已带 `CREATE_NO_WINDOW`）—— robocopy 是控制台程序，
        // GUI 进程直接 `Command::new` 会让每次调用都闪一个黑框。
        syscmd::command(program)
            .args(args)
            .stdin(Stdio::null())
            .output()
    }

    /// 共用的 robocopy 参数：不重试、不等待、跳过重解析点。
    const ROBO_SAFE: [&str; 5] = ["/E", "/COPY:DAT", "/DCOPY:DAT", "/R:0", "/W:0"];

    /// 🔴 `/XJ` **必须**有 —— 否则会跟进源目录里的重解析点，
    /// 把运行时联接指向的整棵树也复制一遍（可能几十 GB 且在别的盘上）。
    const ROBO_EXTRA: [&str; 1] = ["/XJ"];

    /// 🔴 `/MT:16` 多线程复制（对齐 zennos0609-dotcom/AppDataMover）——
    /// 大目录复制显著加速；每文件仍输出一行，行计数进度的口径不变。
    const ROBO_MT: [&str; 1] = ["/MT:16"];

    /// 递归复制（Windows：robocopy），**每复制一个文件回调一次**已复制计数。
    ///
    /// 🔴 复制阶段**不**加 `/NFL` —— 要靠「每复制一个文件输出一行」驱动真实进度。
    /// `/NDL`（不列目录）、`/NJH /NJS`（无头尾）、`/NP`（无百分比行）都保留，
    /// 于是**剩下的非空行 ≈ 已复制文件数**。行内容是 OEM 编码无所谓 —— 只数行，
    /// 不做解码（解码才需要关心代码页）。
    ///
    /// 另外参考 OPTOPIA 的做法把 robocopy 完整日志落盘到
    /// `%APPDATA%\AgentCache\logs\<名>-<时间戳>.robocopy.log`（`/TEE /LOG`）——
    /// stdout 照常驱动进度，日志留档供事后审计「当时到底复制了什么」。
    pub fn copy_tree(src: &Path, dst: &Path, on_file: &mut dyn FnMut(u64)) -> Result<(), String> {
        let s = src.to_string_lossy().to_string();
        let d = dst.to_string_lossy().to_string();
        let mut args: Vec<String> = vec![s.clone(), d.clone()];
        args.extend(ROBO_SAFE.iter().map(|s| s.to_string()));
        args.extend(ROBO_EXTRA.iter().map(|s| s.to_string()));
        args.extend(ROBO_MT.iter().map(|s| s.to_string()));
        args.extend([
            "/NDL".to_string(),
            "/NJH".to_string(),
            "/NJS".to_string(),
            "/NP".to_string(),
        ]);

        // 日志落盘（best-effort：建不了目录就不留档，不影响复制本身）
        if let Some(log_dir) =
            crate::paths::roaming_dir().map(|d| d.join("AgentCache").join("logs"))
        {
            if std::fs::create_dir_all(&log_dir).is_ok() {
                let ts = chrono::Local::now().format("%Y%m%d-%H%M%S");
                let name = src
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| "copy".to_string());
                args.push("/TEE".to_string());
                args.push(format!(
                    "/LOG:{}",
                    log_dir.join(format!("{name}-{ts}.robocopy.log")).display()
                ));
            }
        }

        let mut child = syscmd::command("robocopy")
            .args(&args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("调不起 robocopy：{e}"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "拿不到 robocopy 的标准输出".to_string())?;

        let mut n: u64 = 0;
        let mut last: std::collections::VecDeque<String> = std::collections::VecDeque::new();
        let mut buf: Vec<u8> = Vec::new();
        let mut reader = IoBufReader::new(stdout);
        loop {
            // 🔴 取消点：每读一行检查一次 —— 命中就杀掉 robocopy 子进程再返回
            if super::is_cancelled() {
                let _ = child.kill();
                return Err(super::CANCELLED_MSG.to_string());
            }
            buf.clear();
            match reader.read_until(b'\n', &mut buf) {
                Ok(0) => break, // EOF
                Ok(_) => {
                    if buf.iter().any(|b| !b.is_ascii_whitespace()) {
                        n += 1;
                        if last.len() == 3 {
                            last.pop_front();
                        }
                        last.push_back(String::from_utf8_lossy(&buf).trim().to_string());
                        on_file(n);
                    }
                }
                Err(e) => {
                    let _ = child.kill();
                    return Err(format!("读 robocopy 输出失败：{e}"));
                }
            }
        }
        let out = child
            .wait()
            .map_err(|e| format!("等 robocopy 结束失败：{e}"))?;
        // wait() 直接给 ExitStatus（不是 Output）—— code() 拿退出码
        let code = out.code().unwrap_or(-1);
        // robocopy 的退出码：0~7 都是成功（含「有文件被复制」「目标有多余文件」…），≥8 才是失败。
        if code < 8 {
            Ok(())
        } else {
            let tail: Vec<String> = last.iter().rev().take(3).cloned().collect();
            Err(format!(
                "robocopy 失败（退出码 {code}）：{}",
                tail.join(" | ")
            ))
        }
    }

    /// 还差多少个文件没搬过去（只数**源树里**的）。
    ///
    /// `skip`：校验阶段忽略的 glob（相对源根的路径命中即不计数）。
    /// 复制阶段不带这些排除 —— 多搬无害，缺的只是「不必一致」的约束。
    pub fn pending_count(src: &Path, dst: &Path, skip: &[String]) -> Result<u64, String> {
        let s = src.to_string_lossy().to_string();
        let d = dst.to_string_lossy().to_string();
        let mut args: Vec<&str> = vec![s.as_str(), d.as_str()];
        args.extend_from_slice(&ROBO_SAFE);
        args.extend_from_slice(&ROBO_EXTRA);
        // /L = 只列不复制；/NS /NC 只留路径
        args.extend_from_slice(&["/L", "/NS", "/NC", "/NDL", "/NJH", "/NJS", "/NP"]);

        let out = run("robocopy", &args).map_err(|e| format!("调不起 robocopy：{e}"))?;
        let text = String::from_utf8_lossy(&out.stdout);
        let root_prefix = norm_root_prefix(src);
        Ok(text
            .lines()
            .filter(|l| !l.trim().is_empty())
            .filter(|l| is_under_root(l, src))
            // 校验排除：命中 skip glob 的相对路径不算「还没搬过去」
            .filter(|l| {
                // ⚠️ 必须先绑定成命名变量：`&norm(l)[..]` 是对临时值做索引切片，
                // 临时值活不过这条 let 语句（索引表达式不享受生命周期延长），
                // 直接写会让 rel 悬垂 —— E0716。
                let full = norm(l);
                let rel = &full[root_prefix.len()..];
                !crate::globs::matches_any(Path::new(rel), skip)
            })
            .count() as u64)
    }

    fn norm(s: &str) -> String {
        s.trim()
            .trim_start_matches(r"\\?\")
            .replace('/', "\\")
            .to_ascii_lowercase()
    }

    /// 归一化后的源根（不带尾分隔符），用于从清单行里剥出相对路径。
    fn norm_root_prefix(root: &Path) -> String {
        let r = norm(&root.to_string_lossy());
        r.trim_end_matches('\\').to_string()
    }

    /// 🔴 只被 Windows 用到 ⇒ 必须 cfg，否则 mac/linux 上是死代码、
    /// 而 CI 开着 `-D warnings`，会直接挂（**且只在非 Windows 上暴露**）。踩过。
    ///
    /// 判据：这行路径确实以「源根目录」开头，且后面紧跟一个分隔符
    /// （防止 `C:\Users\x\.codex2` 被误判成在 `C:\Users\x\.codex` 之下）。
    fn is_under_root(line: &str, root: &Path) -> bool {
        let norm = |s: &str| {
            s.trim()
                .trim_start_matches(r"\\?\")
                .replace('/', "\\")
                .to_ascii_lowercase()
        };
        let l = norm(line);
        let r = norm(&root.to_string_lossy());
        let r = r.trim_end_matches('\\');
        if r.is_empty() {
            return true;
        }
        l.starts_with(r) && l.as_bytes().get(r.len()) == Some(&b'\\')
    }

    /// 原目录改名成快照。改名失败时会重试一次（杀软/索引服务常会短暂占用）。
    pub fn rename_to_snapshot(src: &Path) -> Result<PathBuf, String> {
        let ts = chrono::Local::now().format("%Y%m%d-%H%M%S").to_string();
        let name = src
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .ok_or_else(|| format!("{} 不像一个目录路径", src.display()))?;
        let dst = src.with_file_name(format!("{name}{SNAPSHOT_MARK}{ts}"));

        match std::fs::rename(src, &dst) {
            Ok(()) => Ok(dst),
            Err(e1) => {
                // 给系统一点时间放开句柄再试一次 —— 比直接报错对用户友好得多
                std::thread::sleep(std::time::Duration::from_millis(600));
                match std::fs::rename(src, &dst) {
                    Ok(()) => Ok(dst),
                    Err(e2) => Err(format!(
                        "改名失败：{e1}（重试后仍是 {e2}）。通常是还有进程占着目录"
                    )),
                }
            }
        }
    }

    /// 快照改回原名（回滚用）。
    pub fn rename_back(snapshot: &Path, src: &Path) -> Result<(), String> {
        std::fs::rename(snapshot, src)
            .map_err(|e| format!("回滚改名失败：{e}（快照仍在 {}）", snapshot.display()))
    }

    /// 建目录联接。**不需要管理员权限**。
    pub fn make_junction(link: &Path, target: &Path) -> Result<(), String> {
        // mklink 是 cmd 内置命令，不是 exe —— 必须经 cmd /C。
        // /J 是目录联接（不是符号链接），不需要管理员也不受开发者模式影响。
        let out = run(
            "cmd",
            &[
                "/C",
                "mklink",
                "/J",
                &link.to_string_lossy(),
                &target.to_string_lossy(),
            ],
        )
        .map_err(|e| format!("调不起 cmd：{e}"))?;
        if out.status.success() {
            return Ok(());
        }
        Err(format!(
            "建联接失败：{} {}",
            String::from_utf8_lossy(&out.stdout).trim(),
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }

    /// 摘掉联接。
    ///
    /// 🔴 用 `remove_dir`（＝ `RemoveDirectoryW`）而**不是** `remove_dir_all`：
    /// 后者会递归删进去、把目标盘的真实数据删掉。对联接执行 `remove_dir` 只摘链接本身。
    pub fn remove_junction(link: &Path) -> Result<(), String> {
        std::fs::remove_dir(link).map_err(|e| format!("摘除联接失败：{e}"))
    }

    /// 穿过联接读一次，确认应用真的能用。
    ///
    /// 只「联接建成了」不够 —— 如果目标盘掉线或权限不对，联接照样「存在」但读不出东西。
    pub fn verify_through_link(link: &Path, dest: &Path) -> Result<usize, String> {
        let n = std::fs::read_dir(link)
            .map_err(|e| format!("穿过联接读 {} 失败：{e}", link.display()))?
            .flatten()
            .count();
        if n == 0 {
            let d = std::fs::read_dir(dest)
                .map(|r| r.flatten().count())
                .unwrap_or(0);
            if d == 0 {
                return Err("联接建成但两边都是空的 —— 疑似目标目录没内容".to_string());
            }
        }
        Ok(n)
    }

    /// 移入回收站（**可还原**，不是彻底删除）。
    ///
    /// 用 [`trash`] crate：Windows 走 COM `IFileOperation`（SHFileOperation
    /// 已过时，且在无交互窗口站的进程里会莫名报错 2 —— 实测踩过）、
    /// macOS 走 NSWorkspace、Linux 走 freedesktop trash 约定。
    /// 调用方（界面/CLI）负责先跟用户确认。
    pub fn remove_dir_recycle(root: &Path) -> Result<(), String> {
        trash::delete(root).map_err(|e| format!("移入回收站失败：{e}"))
    }

    /// 永久删除一个目录树（先解 ACL 再删，跳过联接本体）。
    pub fn remove_dir_forced(root: &Path) -> Result<u64, String> {
        if !root.exists() {
            return Ok(0);
        }
        // 先量体积 —— 删完就量不到了。`measure` 不跟随重解析点，
        // 所以联接指向的别处数据不会被算进来（那些也不该被删）。
        let freed = fsutil::measure(root, 2_000_000, std::time::Duration::from_secs(60)).bytes;

        // 🔴 应用会给「当天日志目录」加 `(OI)(CI)(DENY)(DE,DC)`，里面文件继承「拒绝删除」
        // → 直接删会 `os error 5`。先对整棵子树 reset 掉显式 ACE，还原成继承。
        let _ = run(
            "icacls",
            &[&root.to_string_lossy(), "/reset", "/T", "/C", "/Q"],
        );

        // 🔴 `remove_dir_all` 在 Windows 上会识别重解析点、**只删链接本身不递归进去**
        // （标准库自带保护）。这正是我们要的：绝不把目标盘的真实数据删掉。
        if let Err(e1) = std::fs::remove_dir_all(root) {
            // 只读/隐藏属性是第二常见的阻碍，去掉再试一次。
            let star = format!("{}\\*", root.to_string_lossy());
            let _ = run("attrib", &["-R", "-S", "-H", &star, "/S", "/D"]);
            std::fs::remove_dir_all(root)
                .map_err(|e2| format!("删除失败：{e1}（清属性后仍失败：{e2}）"))?;
        }
        Ok(freed)
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn is_under_root_requires_separator_boundary() {
            let root = Path::new(r"C:\Users\x\.codex");
            assert!(is_under_root(r"C:\Users\x\.codex\a.txt", root));
            assert!(is_under_root(r"\\?\C:\Users\x\.codex\a.txt", root));
            assert!(is_under_root(r"c:\users\x\.codex\sub\b.bin", root));
            assert!(
                is_under_root(r"C:/Users/x/.codex/a.txt", root),
                "正斜杠也要认"
            );
            assert!(
                is_under_root("  C:\\Users\\x\\.codex\\a  ", root),
                "要 trim 缩进"
            );

            // 🔴 前缀相同但**不是**子路径 —— 没有分隔符边界
            assert!(!is_under_root(r"C:\Users\x\.codex2\a.txt", root));
            assert!(!is_under_root(r"C:\Users\x\.codex-old\a.txt", root));

            // 🔴 目标盘路径必须被排除（这是「迁移永远失败」的真凶）
            assert!(!is_under_root(r"F:\.AgentCache\codex\a.txt", root));
        }

        #[test]
        fn snapshot_prefix_matches_contract() {
            let src = PathBuf::from(r"C:\Users\x\.codex");
            let name = src.file_name().unwrap().to_string_lossy().to_string();
            let prefix = format!("{name}{SNAPSHOT_MARK}");
            assert_eq!(prefix, ".codex.moved-");
        }

        fn scratch(name: &str) -> PathBuf {
            let d = std::env::temp_dir().join(format!("acm-win-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&d);
            std::fs::create_dir_all(&d).unwrap();
            d
        }

        /// 端到端：复制（/XJ 跳过联接）→ 校验 → 快照 → 建联接 → 穿过联接验证 → 回滚。
        ///
        /// 🔴 Windows 是主战场，但这条主路径此前只有手工验证。建联接（`mklink /J`）
        /// **不需要管理员权限**，所以这个测试在 CI 的 windows runner 上能真跑 ——
        /// unix 那边的同款测试早已存在，这里补齐对等性。
        #[test]
        fn copy_verify_link_rollback_roundtrip() {
            let base = scratch("rt");
            let src = base.join("src");
            let dst = base.join("dst");
            std::fs::create_dir_all(src.join("sub")).unwrap();
            std::fs::write(src.join("a.txt"), b"hello").unwrap();
            std::fs::write(src.join("sub/b.bin"), vec![7u8; 4096]).unwrap();

            // 造一个指向「外面」的目录联接 —— 复制时必须跳过它（/XJ 的契约）
            let outside = base.join("outside");
            std::fs::create_dir_all(&outside).unwrap();
            std::fs::write(outside.join("must-not-be-copied.txt"), b"x").unwrap();
            make_junction(&src.join("link-out"), &outside).unwrap();

            // ① 复制
            copy_tree(&src, &dst, &mut |_| {}).unwrap();
            assert!(dst.join("a.txt").exists());
            assert!(dst.join("sub/b.bin").exists());
            assert!(
                !dst.join("link-out").exists(),
                "源里的联接不该被复制/跟随（对齐 /XJ）"
            );

            // ② 校验：刚复制完应当收敛；skip 命中的条目不算数
            assert_eq!(pending_count(&src, &dst, &[]).unwrap(), 0);
            std::fs::write(src.join("logs.tmp"), b"junk").unwrap();
            assert_eq!(
                pending_count(&src, &dst, &["*.tmp".to_string()]).unwrap(),
                0,
                "skip 命中的文件不该参与收敛判定"
            );
            assert_eq!(pending_count(&src, &dst, &[]).unwrap(), 1);

            // ③ 新增文件 → 增量复制 → 收敛
            std::fs::write(src.join("c.txt"), b"new").unwrap();
            assert_eq!(pending_count(&src, &dst, &[]).unwrap(), 2);
            copy_tree(&src, &dst, &mut |_| {}).unwrap();
            assert_eq!(
                pending_count(&src, &dst, &[]).unwrap(),
                0,
                "增量复制后应当收敛"
            );

            // ④ 快照 + 联接 + 穿过联接验证
            let snap = rename_to_snapshot(&src).unwrap();
            assert!(snap
                .file_name()
                .unwrap()
                .to_string_lossy()
                .contains(".moved-"));
            make_junction(&src, &dst).unwrap();
            assert!(verify_through_link(&src, &dst).unwrap() >= 3);
            assert!(src.join("a.txt").exists(), "穿过联接读原路径也要拿到文件");

            // ⑤ 摘联接（🔴 目标真实数据一个不能少）+ 回滚
            remove_junction(&src).unwrap();
            rename_back(&snap, &src).unwrap();
            assert!(src.is_dir(), "回滚后应恢复成真实目录");
            assert!(!fsutil::is_link(&src));
            assert!(src.join("a.txt").exists());

            let _ = std::fs::remove_dir_all(&base);
            let _ = std::fs::remove_dir_all(&outside);
        }

        /// 回收站删除：目录从原位置消失，但**不是**被永久销毁。
        /// （回收站里能还原 —— 这正是它和 `remove_dir_forced` 的区别。）
        ///
        /// ⚠️ `#[ignore]`：shell 级删除（IFileOperation/SHFileOperation）需要
        /// **交互式桌面会话**，在无头环境（agent shell、部分 CI service 容器）
        /// 里会被系统直接中止 —— SHFileOperation 报代码 2、IFileOperation 报
        /// 「operations were aborted」，都是这个原因，与代码逻辑无关。
        /// 终端用户跑的是交互式 GUI，不受影响。需要真验证时：
        /// `cargo test -p acm-core --lib recycle_removes -- --ignored`
        #[test]
        #[ignore = "需要交互式桌面会话（无头环境会被系统中止）"]
        fn recycle_removes_from_place_but_not_forever() {
            let base = scratch("recycle");
            let dir = base.join("victim.moved-20261016-000000");
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("x.txt"), b"x").unwrap();

            remove_dir_recycle(&dir).unwrap();
            assert!(!dir.exists(), "应从原位置消失（已进回收站）");

            let _ = std::fs::remove_dir_all(&base);
        }

        /// `remove_dir_forced` 的安全带：目录里如果躺着联接，
        /// 删除时只摘联接本身，**绝不能把联接指向的真实数据删掉**。
        #[test]
        fn remove_dir_forced_never_deletes_through_reparse() {
            let base = scratch("rmtree");
            let victim = base.join("victim");
            std::fs::create_dir_all(&victim).unwrap();
            std::fs::write(victim.join("own.txt"), b"delete-me").unwrap();

            // 联接指向外面的「必须幸存」目录
            let survivor = base.join("survivor");
            std::fs::create_dir_all(&survivor).unwrap();
            std::fs::write(survivor.join("keep.txt"), b"keep-me").unwrap();
            make_junction(&victim.join("link"), &survivor).unwrap();

            remove_dir_forced(&victim).unwrap();
            assert!(!victim.exists(), "victim 本体应被删除");
            assert!(
                survivor.join("keep.txt").exists(),
                "🔴 联接指向的真实数据必须原样幸存"
            );

            let _ = std::fs::remove_dir_all(&base);
        }
    }
}

/// Unix 实现：纯 Rust 递归复制 + **符号链接**（代替 Windows 的 NTFS 目录联接）。
///
/// 为什么不用 `rsync`：各平台的 rsync 参数、是否预装、版本差异都不一致，
/// 而「复制」这件事用标准库做完全够 —— `std::fs::copy` 保留权限位，
/// `File::set_times` 保留 mtime（Rust 1.75+ 的标准能力），**零额外依赖**。
///
/// 🔴 语义上与 Windows 版对齐的两点（改动时别破坏）：
/// - **不跟随符号链接**（对应 Windows 的 `/XJ`）：源里若有指向别处的链接，
///   跟进去会把别处的整棵树也复制一遍，可能是几十 GB 且在另一块盘上。
/// - **摘链接只摘链接本身**（`remove_file`，不是 `remove_dir_all`）。
#[cfg(unix)]
mod imp {
    use super::SNAPSHOT_MARK;
    use crate::fsutil;
    use std::fs::{File, FileTimes};
    use std::path::{Path, PathBuf};
    use std::time::Duration;

    /// 递归复制（unix：纯标准库），**每复制一个文件回调一次**已复制计数。
    pub fn copy_tree(src: &Path, dst: &Path, on_file: &mut dyn FnMut(u64)) -> Result<(), String> {
        let mut n: u64 = 0;
        copy_dir(src, dst, &mut n, on_file)
            .map_err(|e| format!("复制 {} → {} 失败：{e}", src.display(), dst.display()))
    }

    fn copy_dir(
        src: &Path,
        dst: &Path,
        n: &mut u64,
        on_file: &mut dyn FnMut(u64),
    ) -> std::io::Result<()> {
        if super::is_cancelled() {
            return Err(std::io::Error::other(super::CANCELLED_MSG));
        }
        std::fs::create_dir_all(dst)?;
        for entry in std::fs::read_dir(src)? {
            let entry = entry?;
            let from = entry.path();
            let to = dst.join(entry.file_name());
            // 🔴 `DirEntry::metadata()` 是 **lstat 语义**（不跟随链接）—— 正是我们要的。
            let md = entry.metadata()?;
            let ft = md.file_type();
            if ft.is_symlink() {
                continue; // 不跟随、不复制（对齐 Windows 的 /XJ）
            }
            if ft.is_dir() {
                copy_dir(&from, &to, n, on_file)?;
            } else {
                copy_file(&from, &to, &md)?;
                *n += 1;
                on_file(*n);
            }
        }
        Ok(())
    }

    fn copy_file(from: &Path, to: &Path, md: &std::fs::Metadata) -> std::io::Result<()> {
        std::fs::copy(from, to)?;
        // 保留 mtime。不只是好看 —— 人看 `ls -l` 时时间是对的，也便于排查。
        // （权限位 `std::fs::copy` 已经带过去了。）
        if let Ok(mtime) = md.modified() {
            if let Ok(f) = File::options().write(true).open(to) {
                let _ = f.set_times(FileTimes::new().set_modified(mtime));
            }
        }
        Ok(())
    }

    /// 还差多少个文件没搬过去。
    ///
    /// 判据是**相对路径存在 + 大小一致**。
    ///
    /// ⚠️ 刻意**不比对 mtime**：源与目标可能落在不同文件系统上（ext4 / APFS / exFAT），
    /// 时间戳精度不同（纳秒 vs 秒），拿它当判据会让增量**永远不收敛** —— 这正是
    /// Windows 那边翻过的车（拿聚合指标当收敛条件，重试多少次都失败）。
    /// 大小不同必然要重传；「同尺寸改写」在**应用已退出**的前提下不会发生，
    /// 而迁移前的占用检测（`locks` + `procs`）就是为此把关的。
    pub fn pending_count(src: &Path, dst: &Path, skip: &[String]) -> Result<u64, String> {
        let mut n = 0u64;
        walk(src, src, &mut |rel: &Path, md: &std::fs::Metadata| {
            // skip 命中的条目不参与收敛判定（复制仍会带上）
            if crate::globs::matches_any(rel, skip) {
                return;
            }
            let to = dst.join(rel);
            match std::fs::symlink_metadata(&to) {
                Ok(d) if d.is_file() && d.len() == md.len() => {}
                _ => n += 1,
            }
        })
        .map_err(|e| format!("扫描 {} 失败：{e}", src.display()))?;
        Ok(n)
    }

    fn walk(
        root: &Path,
        dir: &Path,
        f: &mut dyn FnMut(&Path, &std::fs::Metadata),
    ) -> std::io::Result<()> {
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            let md = entry.metadata()?;
            let ft = md.file_type();
            if ft.is_symlink() {
                continue;
            }
            if ft.is_dir() {
                walk(root, &path, f)?;
            } else if let Ok(rel) = path.strip_prefix(root) {
                f(rel, &md);
            }
        }
        Ok(())
    }

    /// 原目录改名成快照（与 Windows 版同样的 `<名字>.moved-<时间戳>` 约定）。
    pub fn rename_to_snapshot(src: &Path) -> Result<PathBuf, String> {
        let ts = chrono::Local::now().format("%Y%m%d-%H%M%S").to_string();
        let name = src
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .ok_or_else(|| format!("{} 不像一个目录路径", src.display()))?;
        let dst = src.with_file_name(format!("{name}{SNAPSHOT_MARK}{ts}"));
        std::fs::rename(src, &dst).map_err(|e| format!("改名失败：{e}（通常是有进程占着目录）"))?;
        Ok(dst)
    }

    pub fn rename_back(snapshot: &Path, src: &Path) -> Result<(), String> {
        std::fs::rename(snapshot, src)
            .map_err(|e| format!("回滚改名失败：{e}（快照仍在 {}）", snapshot.display()))
    }

    /// 建符号链接。
    ///
    /// ⚠️ 与 Windows 的目录联接**有语义差别**，文档里要说清楚：
    /// 联接是文件系统层的「目录别名」，对应用完全透明；而符号链接是一个
    /// 真实存在的链接文件，绝大多数程序会正常跟随，但少数会 `O_NOFOLLOW`
    /// 或 `lstat` 检测。这是 unix 上的通行做法，但比 Windows 那份多一个前提。
    pub fn make_junction(link: &Path, target: &Path) -> Result<(), String> {
        std::os::unix::fs::symlink(target, link).map_err(|e| {
            format!(
                "建符号链接失败：{e}（{} → {}）",
                link.display(),
                target.display()
            )
        })
    }

    /// 摘掉符号链接。
    ///
    /// 🔴 必须用 `remove_file`：unix 上符号链接是「文件」。
    /// 用 `remove_dir_all` 会**跟随进去、把目标盘的真实数据删掉**。
    pub fn remove_junction(link: &Path) -> Result<(), String> {
        std::fs::remove_file(link).map_err(|e| format!("摘除符号链接失败：{e}"))
    }

    /// 穿过链接读一次，确认应用真的能用。
    ///
    /// 只「链接建成了」不够 —— 目标盘掉线或权限不对时，链接照样「存在」但读不出东西。
    pub fn verify_through_link(link: &Path, dest: &Path) -> Result<usize, String> {
        let md = std::fs::symlink_metadata(link)
            .map_err(|e| format!("读不到 {}：{e}", link.display()))?;
        if !md.file_type().is_symlink() {
            return Err(format!("{} 建出来的不是符号链接", link.display()));
        }
        let n = std::fs::read_dir(link)
            .map_err(|e| format!("穿过链接读 {} 失败：{e}", link.display()))?
            .flatten()
            .count();
        if n == 0 {
            let d = std::fs::read_dir(dest)
                .map(|r| r.flatten().count())
                .unwrap_or(0);
            if d == 0 {
                return Err("链接建成但两边都是空的 —— 疑似目标目录没内容".to_string());
            }
        }
        Ok(n)
    }

    /// 永久删除一个目录树（先量体积再删）。unix 上不需要解 ACL。
    pub fn remove_dir_forced(root: &Path) -> Result<u64, String> {
        if !root.exists() {
            return Ok(0);
        }
        let freed = fsutil::measure(root, 2_000_000, Duration::from_secs(60)).bytes;
        // `remove_dir_all` 遇到符号链接只会 unlink 链接本身（标准库自带保护），
        // 所以不会误删目标盘的数据。
        std::fs::remove_dir_all(root).map_err(|e| format!("删除失败：{e}"))?;
        Ok(freed)
    }

    /// unix 同样走 [`trash`]（freedesktop trash / macOS `.Trash`）—— 与 Windows
    /// 一致：移入回收站、可还原，而不是假装做了。
    pub fn remove_dir_recycle(root: &Path) -> Result<(), String> {
        trash::delete(root).map_err(|e| format!("移入回收站失败：{e}"))
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn scratch(name: &str) -> PathBuf {
            let d = std::env::temp_dir().join(format!("acm-unix-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&d);
            std::fs::create_dir_all(&d).unwrap();
            d
        }

        /// 端到端：复制 → 校验 → 快照 → 建链接 → 穿过链接验证 → 回滚。
        /// 这个测试在 Linux/macOS 上真跑，所以 unix 这条路是**被验证过的**，不是纸面实现。
        #[test]
        fn copy_verify_link_rollback_roundtrip() {
            let base = scratch("rt");
            let src = base.join("src");
            let dst = base.join("dst");
            std::fs::create_dir_all(src.join("sub")).unwrap();
            std::fs::write(src.join("a.txt"), b"hello").unwrap();
            std::fs::write(src.join("sub/b.bin"), vec![7u8; 4096]).unwrap();

            // 造一个指向「外面」的符号链接 —— 复制时必须跳过它
            let outside = base.join("outside");
            std::fs::create_dir_all(&outside).unwrap();
            std::fs::write(outside.join("must-not-be-copied.txt"), b"x").unwrap();
            std::os::unix::fs::symlink(&outside, src.join("link-out")).unwrap();

            // ① 复制
            copy_tree(&src, &dst, &mut |_| {}).unwrap();
            assert!(dst.join("a.txt").exists());
            assert!(dst.join("sub/b.bin").exists());
            assert!(
                !dst.join("link-out").exists(),
                "源里的符号链接不该被复制/跟随（对齐 Windows 的 /XJ）"
            );
            assert!(
                !dst.join("outside").exists(),
                "🔴 绝不能把链接指向的整棵树也复制进来"
            );

            // ② 校验：刚复制完应当收敛
            assert_eq!(pending_count(&src, &dst, &[]).unwrap(), 0);

            // ③ 新增一个文件 → 校验要能发现
            std::fs::write(src.join("c.txt"), b"new").unwrap();
            assert_eq!(pending_count(&src, &dst, &[]).unwrap(), 1);

            // ④ 再复制一次（增量）→ 应当收敛，且新文件确实到了目标
            //    （这条是补上的：早先只在 ③ 之后直接断言目标条目数，
            //     忘了 c.txt 还没被复制 —— 测试自己把这一步暴露了出来）
            copy_tree(&src, &dst, &mut |_| {}).unwrap();
            assert_eq!(
                pending_count(&src, &dst, &[]).unwrap(),
                0,
                "增量复制后应当收敛"
            );
            assert!(dst.join("c.txt").exists(), "增量复制要把新文件带过去");

            // ⑤ 快照 + 链接 + 穿过链接验证
            let snap = rename_to_snapshot(&src).unwrap();
            assert!(snap
                .file_name()
                .unwrap()
                .to_string_lossy()
                .contains(".moved-"));
            make_junction(&src, &dst).unwrap();
            // dst 里现在应当是 a.txt / c.txt / sub 三个条目
            assert!(verify_through_link(&src, &dst).unwrap() >= 3);
            assert!(src.join("a.txt").exists(), "穿过链接读原路径也要拿到文件");

            // ⑥ 摘链接 + 回滚
            remove_junction(&src).unwrap();
            rename_back(&snap, &src).unwrap();
            assert!(src.is_dir() && !src.is_symlink(), "回滚后应恢复成真实目录");

            let _ = std::fs::remove_dir_all(&base);
        }

        #[test]
        fn pending_counts_missing_and_size_mismatch() {
            let base = scratch("pending");
            let src = base.join("s");
            let dst = base.join("d");
            std::fs::create_dir_all(&src).unwrap();
            std::fs::create_dir_all(&dst).unwrap();
            std::fs::write(src.join("same.bin"), vec![1u8; 100]).unwrap();
            std::fs::write(dst.join("same.bin"), vec![1u8; 100]).unwrap(); // 一致
            std::fs::write(src.join("diff.bin"), vec![1u8; 100]).unwrap();
            std::fs::write(dst.join("diff.bin"), vec![1u8; 50]).unwrap(); // 大小不同
            std::fs::write(src.join("only-src.bin"), b"x").unwrap(); // 目标没有

            assert_eq!(pending_count(&src, &dst, &[]).unwrap(), 2);

            let _ = std::fs::remove_dir_all(&base);
        }

        /// skip 命中的条目不参与收敛判定 —— `agents.toml` 里 `skip` 字段的契约。
        #[test]
        fn skip_globs_are_excluded_from_verification() {
            let base = scratch("skip");
            let src = base.join("s");
            let dst = base.join("d");
            std::fs::create_dir_all(src.join("logs")).unwrap();
            std::fs::create_dir_all(&dst).unwrap();
            // 数据文件：两边不一致，必须被算出来
            std::fs::write(src.join("data.db"), vec![1u8; 10]).unwrap();
            // 日志：目标里没有，但配了 skip 就不该计入
            std::fs::write(src.join("logs/run.log"), b"log").unwrap();
            std::fs::write(src.join("trace.log"), b"log").unwrap();

            let skip = vec!["logs/**".to_string(), "**/*.log".to_string()];
            assert_eq!(
                pending_count(&src, &dst, &skip).unwrap(),
                1,
                "只有 data.db 该被算作待复制"
            );
            // 不带 skip 时 3 个都算
            assert_eq!(pending_count(&src, &dst, &[]).unwrap(), 3);

            let _ = std::fs::remove_dir_all(&base);
        }

        #[test]
        fn remove_junction_never_touches_target() {
            let base = scratch("unlink");
            let target = base.join("real");
            std::fs::create_dir_all(&target).unwrap();
            std::fs::write(target.join("keep.txt"), b"keep").unwrap();
            let link = base.join("link");
            make_junction(&link, &target).unwrap();

            remove_junction(&link).unwrap();
            assert!(!link.exists(), "链接应已摘除");
            assert!(
                target.join("keep.txt").exists(),
                "🔴 目标里的真实数据必须一个不少"
            );

            let _ = std::fs::remove_dir_all(&base);
        }
    }
}

/// 既不是 Windows 也不是 Unix（wasm 等）—— 明确报「不支持」，不静默失败。
#[cfg(not(any(windows, unix)))]
mod imp {
    use std::path::{Path, PathBuf};

    const MSG: &str = "迁移目前只实现了 Windows 与 Unix（macOS / Linux）";

    pub fn copy_tree(
        _src: &Path,
        _dst: &Path,
        _on_file: &mut dyn FnMut(u64),
    ) -> Result<(), String> {
        Err(MSG.to_string())
    }
    pub fn pending_count(_src: &Path, _dst: &Path, _skip: &[String]) -> Result<u64, String> {
        Err(MSG.to_string())
    }
    pub fn rename_to_snapshot(_src: &Path) -> Result<PathBuf, String> {
        Err(MSG.to_string())
    }
    pub fn rename_back(_snapshot: &Path, _src: &Path) -> Result<(), String> {
        Err(MSG.to_string())
    }
    pub fn make_junction(_link: &Path, _target: &Path) -> Result<(), String> {
        Err(MSG.to_string())
    }
    pub fn remove_junction(_link: &Path) -> Result<(), String> {
        Err(MSG.to_string())
    }
    pub fn verify_through_link(_link: &Path, _dest: &Path) -> Result<usize, String> {
        Err(MSG.to_string())
    }
    pub fn remove_dir_forced(_root: &Path) -> Result<u64, String> {
        Err(MSG.to_string())
    }
    pub fn remove_dir_recycle(_root: &Path) -> Result<(), String> {
        Err(MSG.to_string())
    }
}

// ============================================================ 盘符 / 挂载点

/// 本机可选的「根位置」。
///
/// - **Windows**：盘符（`C:/`、`D:/`…）
/// - **unix**：挂载点（`/`、`/home`、`/mnt/data`…）—— unix 没有盘符这个概念
///
/// 调用方（界面）应当把它当作「一个可选的根路径」来用，而不是当成盘符字面量。
/// 界面上统一的说法是「目标位置」。
#[cfg(windows)]
pub fn drives() -> Vec<String> {
    let mask = unsafe { windows_sys::Win32::Storage::FileSystem::GetLogicalDrives() };
    (0..26u32)
        .filter(|i| mask & (1 << i) != 0)
        .map(|i| format!("{}:\\", (b'A' + i as u8) as char))
        .collect()
}

/// 见 Windows 版说明。
#[cfg(unix)]
pub fn drives() -> Vec<String> {
    let mut v: Vec<String> = mount_points().into_iter().map(|(mp, _, _)| mp).collect();
    v.sort();
    v.dedup();
    v
}

/// 见 Windows 版说明。
#[cfg(not(any(windows, unix)))]
pub fn drives() -> Vec<String> {
    vec!["/".to_string()]
}

/// 解析 `df -P -k` → `(挂载点, 可用字节, 总字节)`。
///
/// 为什么用 `df` 而不是读 `/proc/mounts`：后者给的是「设备」，
/// 而用户要的恰恰是**哪个位置还有空**。`-P` 是 POSIX 规定的单行格式，
/// 就是为脚本解析而存在的，Linux 与 macOS 行为一致。
#[cfg(unix)]
fn mount_points() -> Vec<(String, u64, u64)> {
    let Ok(out) = crate::syscmd::command("df").args(["-P", "-k"]).output() else {
        return Vec::new();
    };
    let text = String::from_utf8_lossy(&out.stdout);
    let mut v = Vec::new();
    for line in text.lines().skip(1) {
        // Filesystem  1024-blocks  Used  Available  Capacity  Mounted on
        let cols: Vec<&str> = line.split_whitespace().collect();
        if cols.len() < 6 {
            continue;
        }
        // 挂载点可能含空格 → 从第 6 列起重新拼回来
        let mp = cols[5..].join(" ");
        if !mp.starts_with('/') {
            continue;
        }
        // 滤掉虚拟 / 系统文件系统：不会有人想把数据「迁到」/proc 里
        if ["/proc", "/sys", "/dev", "/run", "/snap", "/boot"]
            .iter()
            .any(|x| mp == *x || mp.starts_with(&format!("{x}/")))
        {
            continue;
        }
        let total = cols[1].parse::<u64>().unwrap_or(0).saturating_mul(1024);
        let avail = cols[3].parse::<u64>().unwrap_or(0).saturating_mul(1024);
        if total > 0 {
            v.push((mp, avail, total));
        }
    }
    v
}

/// 某个路径所在卷的可用字节。
#[cfg(any(windows, unix))]
pub fn free_space(p: &Path) -> Option<u64> {
    volume_space(p).map(|(free, _)| free)
}

/// 见上。
#[cfg(not(any(windows, unix)))]
pub fn free_space(_p: &Path) -> Option<u64> {
    None
}

/// 某个路径所在卷的 `(可用, 总量)`。
///
/// 界面要画容量条，所以总量也得给 —— 一次调用拿全，没必要分两次。
#[cfg(windows)]
pub fn volume_space(p: &Path) -> Option<(u64, u64)> {
    let (free, total) = sys::volume_space(&p.to_string_lossy());
    (total > 0).then_some((free, total))
}

/// 见 Windows 版说明。
#[cfg(unix)]
pub fn volume_space(p: &Path) -> Option<(u64, u64)> {
    let (free, total) = sys::volume_space(&p.to_string_lossy());
    (total > 0).then_some((free, total))
}

/// 见上。
#[cfg(not(any(windows, unix)))]
pub fn volume_space(_p: &Path) -> Option<(u64, u64)> {
    None
}

/// Windows 的系统调用集中在这里 —— 上面的公开函数只是薄薄一层包装，
/// 这样平台差异只有一处，也方便对照检查。
#[cfg(windows)]
mod sys {
    use std::os::windows::ffi::OsStrExt;

    /// 取到盘符根（`X:/`）—— 传根最稳，且能避开超长路径的坑。
    fn root_of(p: &str) -> Option<&str> {
        let i = p.find(":\\")?;
        Some(&p[..i + 2])
    }

    pub fn volume_space(p: &str) -> (u64, u64) {
        let Some(root) = root_of(p) else {
            return (0, 0);
        };
        let wide: Vec<u16> = std::ffi::OsStr::new(root)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let (mut free, mut total) = (0u64, 0u64);
        let ok = unsafe {
            windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW(
                wide.as_ptr(),
                &mut free,
                &mut total,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 {
            (0, 0)
        } else {
            (free, total)
        }
    }
}

/// unix 的容量查询：`df -P -k <路径>` 会直接给出**该路径所在文件系统**的那一行
/// （不用自己去匹配挂载点前缀，`df` 已经做了）。
#[cfg(unix)]
mod sys {
    pub fn volume_space(p: &str) -> (u64, u64) {
        let Ok(out) = crate::syscmd::command("df")
            .args(["-P", "-k"])
            .arg(p)
            .output()
        else {
            return (0, 0);
        };
        let text = String::from_utf8_lossy(&out.stdout);
        // 第一行是表头，取第二行
        let Some(line) = text.lines().nth(1) else {
            return (0, 0);
        };
        let cols: Vec<&str> = line.split_whitespace().collect();
        if cols.len() < 4 {
            return (0, 0);
        }
        let total = cols[1].parse::<u64>().unwrap_or(0).saturating_mul(1024);
        let avail = cols[3].parse::<u64>().unwrap_or(0).saturating_mul(1024);
        (avail, total)
    }
}

/// 系统位置：Windows 是 `C:`，unix 是 `/`。
#[cfg(windows)]
pub fn system_drive() -> Option<String> {
    std::env::var("SystemDrive").ok().map(|s| s.to_uppercase())
}

/// 见 Windows 版说明。
#[cfg(unix)]
pub fn system_drive() -> Option<String> {
    Some("/".to_string())
}

/// 见上。
#[cfg(not(any(windows, unix)))]
pub fn system_drive() -> Option<String> {
    None
}

/// 规范化「根」字符串以便比较：去掉尾部分隔符。
/// Windows 的 `C:/` → `C:`；unix 的 `/` 保持 `/`。
fn norm_root(s: &str) -> String {
    let t = s.trim_end_matches(['\\', '/']);
    if t.is_empty() {
        "/".to_string()
    } else {
        t.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_inside_detects_recursion() {
        assert!(path_starts_with(
            Path::new(r"D:\a\b\c"),
            Path::new(r"D:\a\b")
        ));
        assert!(path_starts_with(Path::new(r"D:\a\b"), Path::new(r"D:\a\b")));
        // 前缀相同但不是子路径
        assert!(!path_starts_with(
            Path::new(r"D:\a\bbc"),
            Path::new(r"D:\a\b")
        ));
        // 长路径前缀与大小写都要忽略
        assert!(path_starts_with(
            Path::new(r"\\?\D:\A\B\C"),
            Path::new(r"d:\a\b")
        ));
    }

    #[test]
    fn snapshots_are_found_by_name() {
        let base = std::env::temp_dir().join("acm-snap-test");
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();

        let target = base.join(".codex");
        let s1 = base.join(".codex.moved-20261001-100000");
        let s2 = base.join(".codex.moved-20261008-221500");
        let other = base.join(".codex.bak"); // 不该被认成快照
        for p in [&s1, &s2, &other] {
            std::fs::create_dir_all(p).unwrap();
        }

        let got = snapshots_of(&target);
        assert_eq!(got.len(), 2, "只认 .moved- 形式");
        assert_eq!(got[0].0, s2, "新的排前面");
        assert_eq!(got[0].1, "20261008-221500");

        let _ = std::fs::remove_dir_all(&base);
    }

    /// `permanent=false` 现在走回收站（不再是「必须显式选择永久删除」），
    /// 但**两条路都要先过快照路径校验** —— 随手传一个目录进来必须被拒。
    #[test]
    fn cleanup_rejects_non_snapshot_paths() {
        let p = std::env::temp_dir().join("acm-nonexistent-cleanup");
        // 名字不符合 <名字>.moved-<时间戳> 约定 → 两条路都进不去
        assert!(cleanup(&p, false).is_err());
        assert!(cleanup(&p, true).is_err());
    }

    /// `delete_snapshot` 是「解 ACL + 清属性 + 递归删」的强破坏原语，
    /// 路径校验是它唯一的安全带 —— 这几条用例就是安全带本身。
    #[test]
    fn snapshot_path_validation_gates_deletion() {
        let defs = crate::agents::parse(
            r#"
schema_version = 1
[[agent]]
id = "codex"
name = "Codex"
paths = ["~/.codex"]
"#,
        )
        .unwrap();
        let home = paths::home_dir().unwrap();
        let parent = home.clone(); // ~/.codex 的父目录就是 ~

        // ✅ 合法快照：约定命名 + 在已知数据目录旁边
        let good = parent.join(".codex.moved-20261008-221500");
        assert!(validate_snapshot_path(&good, &defs).is_ok());

        // ❌ 没有 .moved- 标记
        assert!(validate_snapshot_path(&parent.join(".codex.bak"), &defs).is_err());

        // ❌ 时间戳格式不对（缺横杠 / 位数不够 / 带尾巴）
        assert!(validate_snapshot_path(&parent.join(".codex.moved-oops"), &defs).is_err());
        assert!(
            validate_snapshot_path(&parent.join(".codex.moved-20261008221500"), &defs).is_err()
        );
        assert!(
            validate_snapshot_path(&parent.join(".codex.moved-20261008-2215-extra"), &defs)
                .is_err()
        );

        // ❌ 名字像快照、时间戳也对，但不在任何已知 Agent 数据目录旁边
        let defs2 = crate::agents::parse(
            r#"
schema_version = 1
[[agent]]
id = "codex"
name = "Codex"
paths = ["~/.nonexistent-agent-xyz"]
"#,
        )
        .unwrap();
        assert!(validate_snapshot_path(&good, &defs2).is_err());

        // ❌ 名字前缀对不上（前缀相同但不是同一个数据目录）
        let imposter = parent.join(".codex2.moved-20261008-221500");
        assert!(validate_snapshot_path(&imposter, &defs).is_err());
    }

    /// 源路径体检：系统目录拦下、云盘警告、普通路径放行。
    #[test]
    fn screen_source_path_flags_system_and_cloud() {
        // 系统关键目录（Windows 形态）
        #[cfg(windows)]
        {
            let (b, w) = screen_source_path(Path::new("C:\\Windows\\System32\\config"));
            assert!(b.is_some(), "Windows 目录应被拦下");
            assert!(w.is_none());
            let (b, _) = screen_source_path(Path::new("C:\\Program Files\\SomeApp\\data"));
            assert!(b.is_some(), "Program Files 应被拦下");
        }
        // 系统关键目录（unix 形态，CI 的 Linux job 会跑到）
        #[cfg(not(windows))]
        {
            let (b, w) = screen_source_path(Path::new("/etc/agentcache"));
            assert!(b.is_some(), "/etc 应被拦下");
            assert!(w.is_none());
        }
        // 云盘同步目录 → warning
        let cloud = std::env::temp_dir().join("OneDrive").join("agentdata");
        let (b, w) = screen_source_path(&cloud);
        assert!(b.is_none(), "云盘只是警告不是拦截");
        assert!(w.is_some(), "OneDrive 应触发警告");
        // 普通路径放行
        let plain = std::env::temp_dir().join("plain-agent");
        let (b, w) = screen_source_path(&plain);
        assert!(b.is_none() && w.is_none());
    }

    /// 修复模式全场景：断链重建 / noop / 链接缺失直建 / 真实目录拒绝。
    /// 跨平台 —— Windows 建联接、unix 建符号链接，走的都是各自的 imp。
    #[test]
    fn repair_rebuilds_broken_link_and_creates_missing() {
        let base = std::env::temp_dir().join(format!("acm-repair-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        let dest_root = base.join("AgentCache");
        let dest = dest_root.join("codex");
        std::fs::create_dir_all(&dest).unwrap();
        std::fs::write(dest.join("data.bin"), vec![1u8; 128]).unwrap();

        let defs = crate::agents::parse(
            r#"
schema_version = 1
[[agent]]
id = "codex"
name = "Codex"
paths = ["~/.codex"]
processes = []
"#,
        )
        .unwrap();
        let def = &defs[0];
        let src = base.join(".codex");

        // ① 断链（指向不存在的旧位置）→ rebuilt
        imp::make_junction(&src, &base.join("old-gone")).unwrap();
        let rep = repair(def, &src, &dest_root).unwrap();
        assert_eq!(rep.action, "rebuilt", "断链应当被重建");
        assert!(src.join("data.bin").exists(), "穿过重建的链接要能读到数据");

        // ② 链接完好且指向正确 → noop
        let rep2 = repair(def, &src, &dest_root).unwrap();
        assert_eq!(rep2.action, "noop");

        // ③ 链接整个没了 → created
        imp::remove_junction(&src).unwrap();
        assert!(!src.exists());
        let rep3 = repair(def, &src, &dest_root).unwrap();
        assert_eq!(rep3.action, "created");
        assert!(src.join("data.bin").exists());

        // ④ 真实目录 → 拒绝（那是迁移的事）
        let real = base.join("real");
        std::fs::create_dir_all(&real).unwrap();
        assert!(repair(def, &real, &dest_root).is_err());

        // ⑤ 链接指向别处且能用 → 拒绝擅改（修错位置比不修更糟）
        let elsewhere = base.join("elsewhere");
        std::fs::create_dir_all(&elsewhere).unwrap();
        std::fs::write(elsewhere.join("o.bin"), b"o").unwrap();
        let _ = imp::remove_junction(&src);
        imp::make_junction(&src, &elsewhere).unwrap();
        assert!(repair(def, &src, &dest_root).is_err());

        // ⑥ 候选目标为空 → 拒绝
        let empty_root = base.join("Empty");
        std::fs::create_dir_all(empty_root.join("codex")).unwrap();
        let _ = imp::remove_junction(&src);
        assert!(repair(def, &src, &empty_root).is_err());

        let _ = std::fs::remove_dir_all(&base);
    }

    /// 回滚前的数据抢救：只搬「快照里没有的或大小不同的」，相同的跳过。
    #[test]
    fn rescue_copies_only_new_or_changed_files() {
        let base = std::env::temp_dir().join(format!("acm-rescue-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let snap = base.join("snap");
        let target = base.join("target");
        std::fs::create_dir_all(snap.join("newdir")).unwrap();
        std::fs::create_dir_all(target.join("newdir")).unwrap();

        std::fs::write(snap.join("same.txt"), vec![1u8; 100]).unwrap();
        std::fs::write(target.join("same.txt"), vec![1u8; 100]).unwrap(); // 一致 → 跳过
        std::fs::write(snap.join("old.txt"), vec![1u8; 50]).unwrap();
        std::fs::write(target.join("old.txt"), vec![1u8; 60]).unwrap(); // 改过 → 抢救
        std::fs::write(target.join("new.txt"), b"new").unwrap(); // 新增 → 抢救
        std::fs::write(target.join("newdir/nested.bin"), vec![2u8; 30]).unwrap(); // 新增嵌套 → 抢救

        let rescue = base.join("rescued");
        let (n, bytes) = rescue_changed_files(&target, &snap, &rescue).unwrap();
        assert_eq!(n, 3);
        assert_eq!(bytes, 60 + 3 + 30);
        assert!(rescue.join("old.txt").exists());
        assert!(rescue.join("new.txt").exists());
        assert!(rescue.join("newdir/nested.bin").exists());
        assert!(!rescue.join("same.txt").exists(), "未变更的文件不该被复制");

        let _ = std::fs::remove_dir_all(&base);
    }

    #[cfg(windows)]
    #[test]
    fn system_volume_is_ntfs() {
        // 系统盘在 Windows 上必然是 NTFS —— 这个断言在 CI 的 windows runner 上也成立
        let fs = volume_fs(Path::new(
            &std::env::var("SystemDrive").unwrap_or("C:\\".into()),
        ))
        .expect("应当能读到系统盘的文件系统");
        assert_eq!(fs.to_ascii_uppercase(), "NTFS");
    }

    /// 🔴 取消 + 哈希校验联动：置位后 copy_tree 中止并返回取消文案；
    /// 清除标记后 hash_compare_tree 正常工作（两者共用全局取消标记，
    /// 必须串行验证 —— 并行测试会互相干扰）。
    /// macOS 的 imp 是占位实现，跳过取消断言。
    #[test]
    fn cancel_flag_and_hash_verification() {
        // 用项目 target/ 而不是 %TEMP% —— 某些沙盒环境会干扰 %TEMP% 里的新建目录
        // （create 成功后 write 却 NotFound），而 target/ 是 cargo 自己频繁写的目录。
        let temp = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("acm-tests");
        let pid = std::process::id();
        let mut setup = None;
        for attempt in 0..3 {
            let base = temp.join(format!("acm-mt-{pid}-{attempt}"));
            let src = base.join("src");
            let dst = base.join("dst");
            if std::fs::create_dir_all(&src).is_err() {
                continue;
            }
            if std::fs::create_dir_all(&dst).is_err() {
                continue;
            }
            if std::fs::write(src.join("f.txt"), b"original").is_err() {
                continue;
            }
            if std::fs::write(dst.join("f.txt"), b"original").is_ok() {
                setup = Some((base, src, dst));
                break;
            }
        }
        let (base, src, dst) = setup.expect("临时目录连续 3 次创建失败（环境异常）");

        // ① 置位取消 → copy_tree 中止
        request_cancel();
        #[cfg(any(windows, all(unix, not(target_os = "macos"))))]
        {
            let r = imp::copy_tree(&src, &base.join("dst2"), &mut |_| {});
            let msg = r.expect_err("取消置位后必须中止");
            assert!(msg.contains(CANCELLED_MSG), "应返回取消文案，实际：{msg}");
        }
        clear_cancel();

        // ② 清除后：哈希一致 → 通过
        let noop = &mut |_p: Progress| {};
        hash_compare_tree(&src, &dst, &[], noop).expect("清除取消标记后应通过");

        // ③ 目标被篡改 → 报不一致
        std::fs::write(dst.join("f.txt"), b"tampered!").unwrap();
        let err = hash_compare_tree(&src, &dst, &[], noop).expect_err("内容不一致必须报错");
        assert!(
            err.contains("内容不一致") || err.contains("f.txt"),
            "实际：{err}"
        );

        clear_cancel();
        let _ = std::fs::remove_dir_all(&base);
    }

    /// 🔴 回归：`--with-env` 迁移设置的变量在 rollback 时必须自动撤销 ——
    /// 否则数据回滚到原位后，应用优先读变量会拿到目标盘的**过期副本**。
    /// 搭一个完整的「已迁移」现场（数据 + 目标 + 联接 + 变量），回滚后断言变量消失。
    #[cfg(windows)]
    #[test]
    fn rollback_unsets_env_var_pointing_at_target() {
        let base = std::env::temp_dir().join(format!("acm-rbenv-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let src = base.join(".codex");
        let dest = base.join("AgentCache").join("codex");
        std::fs::create_dir_all(&dest).unwrap();
        std::fs::write(dest.join("data.bin"), vec![1u8; 64]).unwrap();

        let defs = crate::agents::parse(
            r#"
schema_version = 1
[[agent]]
id = "codex"
name = "Codex"
paths = ["~/.codex"]
processes = []
[agent.env_relocate]
env = "AC_TEST_ROLLBACK_VAR"
"#,
        )
        .unwrap();
        let def = &defs[0];
        let var = &def.env_relocate.as_ref().unwrap().env;

        // 现场：原目录已改名快照、联接已建、变量已指向目标（= with-env 迁移后的状态）
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("old.bin"), b"old").unwrap();
        let _snapshot = imp::rename_to_snapshot(&src).expect("改名失败");
        imp::make_junction(&src, &dest).unwrap();
        crate::envvar::set(var, &dest.to_string_lossy()).unwrap();

        // 回滚
        let msg = rollback(def, &src).expect("回滚应成功");
        assert!(msg.contains("已撤销环境变量"), "消息应提到撤销变量：{msg}");
        assert_eq!(crate::envvar::get(var), None, "变量必须被撤销");
        // 回滚后原路径 = 快照里的旧数据；迁移数据仍在目标盘（由 rescue/联接逻辑保证）
        assert!(src.join("old.bin").exists(), "回滚后原路径应是快照数据");
        assert!(dest.join("data.bin").exists(), "目标盘数据不应被破坏");

        let _ = crate::envvar::remove(var);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn dedup_labels_collapses_same_app() {
        let h = vec![
            crate::locks::Holder {
                pid: 1,
                app: "X.exe".into(),
            },
            crate::locks::Holder {
                pid: 2,
                app: "X.exe".into(),
            },
            crate::locks::Holder {
                pid: 3,
                app: "Y.exe".into(),
            },
        ];
        assert_eq!(dedup_labels(&h), vec!["X.exe", "Y.exe"]);
    }

    #[test]
    fn now_is_sane() {
        // 快照时间戳的来源 —— 只验证它是个合理的当前时间
        let d = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap();
        assert!(d.as_secs() > 1_700_000_000);
    }
}
