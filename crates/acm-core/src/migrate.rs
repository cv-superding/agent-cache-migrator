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

    // ① 谁锁着源目录（精确判断，比按进程名猜可靠）
    let holders = crate::locks::holders_of_dir(src, 400);
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
    if def.env_relocate.is_some() {
        warnings.push(
            "这个 Agent 官方支持用环境变量重定位数据目录；当前走的是「搬目录 + 建联接」\
             （对应用透明、更通用）。"
                .to_string(),
        );
    }
    if !cfg!(windows) {
        blockers.push("目前只支持 Windows（robocopy + NTFS 目录联接）".to_string());
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
    })
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
        if Some(drive.trim_end_matches('\\').to_string()) == sys {
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
pub fn run(plan: &Plan, cb: &mut dyn FnMut(Progress)) -> MigrateReport {
    let src = PathBuf::from(&plan.src);
    let dest = PathBuf::from(&plan.dest);
    let mut r = MigrateReport::new(&src, &dest);

    if !plan.blockers.is_empty() {
        return r.fail(plan.blockers.join("；"));
    }

    // ① 复制
    cb(Progress {
        step: "copying".into(),
        detail: format!("正在复制到 {}", dest.display()),
        percent: Some(5),
    });
    match win::copy_tree(&src, &dest) {
        Ok(()) => r.steps.push(StepResult {
            name: "复制".into(),
            ok: true,
            detail: "robocopy 完成（只补不删）".into(),
        }),
        Err(e) => {
            r.steps.push(StepResult {
                name: "复制".into(),
                ok: false,
                detail: e.clone(),
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
    match win::pending_count(&src, &dest) {
        Ok(0) => r.steps.push(StepResult {
            name: "校验".into(),
            ok: true,
            detail: "源里已无待复制文件（0 差异）".into(),
        }),
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
    let snapshot = match win::rename_to_snapshot(&src) {
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
    if let Err(e) = win::make_junction(&src, &dest) {
        r.steps.push(StepResult {
            name: "建联接".into(),
            ok: false,
            detail: e.clone(),
        });
        // 回滚：把快照改回去
        r.rolled_back = win::rename_back(&snapshot, &src).is_ok();
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
    match win::verify_through_link(&src, &dest) {
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
            let _ = win::remove_junction(&src);
            r.rolled_back = win::rename_back(&snapshot, &src).is_ok();
            r.snapshot = None;
            return r.fail(e);
        }
    }

    cb(Progress {
        step: "done".into(),
        detail: "迁移完成".into(),
        percent: Some(100),
    });
    r.ok = true;
    r
}

/// 回滚：把某个 agent 的快照改回原位（联接必须先摘掉）。
///
/// 只处理**最新**的一个快照 —— 同名多个快照说明状态已经不干净，交给人判断。
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
    if fsutil::is_link(src) {
        win::remove_junction(src)?;
    }
    win::rename_back(&snap, src)?;
    Ok(format!(
        "已回滚：{} → {}（agent {}）",
        snap.display(),
        src.display(),
        def.id
    ))
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

/// 删掉一个快照。`permanent = false` 时**暂不支持回收站**，会直接报错让调用方明确选择。
///
/// ⚠️ 删掉之后这次迁移就**不能回滚**了 —— 界面上必须先跟用户确认。
pub fn cleanup(snapshot: &Path, permanent: bool) -> Result<u64, String> {
    if !permanent {
        return Err(
            "回收站删除尚未实现（SHFileOperation 不支持超长路径，需要逐条兜底）；\
                    请显式选择永久删除"
                .to_string(),
        );
    }
    win::remove_dir_forced(snapshot)
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
mod win {
    use super::{fsutil, SNAPSHOT_MARK};
    use std::path::{Path, PathBuf};
    use std::process::{Command, Output, Stdio};

    /// 关掉子进程的控制台窗口（否则 GUI 里会闪黑框）。
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    fn run(program: &str, args: &[&str]) -> std::io::Result<Output> {
        use std::os::windows::process::CommandExt;
        Command::new(program)
            .args(args)
            .creation_flags(CREATE_NO_WINDOW)
            .stdin(Stdio::null())
            .output()
    }

    /// 共用的 robocopy 参数：不重试、不等待、跳过重解析点。
    const ROBO_SAFE: [&str; 5] = ["/E", "/COPY:DAT", "/DCOPY:DAT", "/R:0", "/W:0"];

    /// 🔴 `/XJ` **必须**有 —— 否则会跟进源目录里的重解析点，
    /// 把运行时联接指向的整棵树也复制一遍（可能几十 GB 且在别的盘上）。
    const ROBO_EXTRA: [&str; 1] = ["/XJ"];

    pub fn copy_tree(src: &Path, dst: &Path) -> Result<(), String> {
        let s = src.to_string_lossy().to_string();
        let d = dst.to_string_lossy().to_string();
        let mut args: Vec<&str> = vec![s.as_str(), d.as_str()];
        args.extend_from_slice(&ROBO_SAFE);
        args.extend_from_slice(&ROBO_EXTRA);
        // /NFL /NDL 不列文件、不列目录；/NJH /NJS 不打印头尾；/NP 不显示百分比
        // —— 我们不需要进度行，robocopy 自己的百分比是「按文件数」的，对大目录没意义。
        args.extend_from_slice(&["/NFL", "/NDL", "/NJH", "/NJS", "/NP"]);

        let out = run("robocopy", &args).map_err(|e| format!("调不起 robocopy：{e}"))?;
        let code = out.status.code().unwrap_or(-1);
        // robocopy 的退出码：0~7 都是成功（含「有文件被复制」「目标有多余文件」…），≥8 才是失败。
        if code < 8 {
            Ok(())
        } else {
            Err(format!(
                "robocopy 失败（退出码 {code}）：{}",
                tail_lines(&out, 3)
            ))
        }
    }

    /// 还差多少个文件没搬过去（只数**源树里**的）。
    pub fn pending_count(src: &Path, dst: &Path) -> Result<u64, String> {
        let s = src.to_string_lossy().to_string();
        let d = dst.to_string_lossy().to_string();
        let mut args: Vec<&str> = vec![s.as_str(), d.as_str()];
        args.extend_from_slice(&ROBO_SAFE);
        args.extend_from_slice(&ROBO_EXTRA);
        // /L = 只列不复制；/NS /NC 只留路径
        args.extend_from_slice(&["/L", "/NS", "/NC", "/NDL", "/NJH", "/NJS", "/NP"]);

        let out = run("robocopy", &args).map_err(|e| format!("调不起 robocopy：{e}"))?;
        let text = String::from_utf8_lossy(&out.stdout);
        Ok(text
            .lines()
            .filter(|l| !l.trim().is_empty())
            .filter(|l| is_under_root(l, src))
            .count() as u64)
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

    /// 取输出末尾几行（报错时给人看）。
    fn tail_lines(out: &Output, n: usize) -> String {
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
        lines[lines.len().saturating_sub(n)..].join(" | ")
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
    }
}

/// 非 Windows 占位：签名一致，明确报「不支持」而不是静默失败。
#[cfg(not(windows))]
mod win {
    use std::path::{Path, PathBuf};

    const MSG: &str = "迁移目前只实现了 Windows（robocopy + NTFS 目录联接）";

    pub fn copy_tree(_src: &Path, _dst: &Path) -> Result<(), String> {
        Err(MSG.to_string())
    }
    pub fn pending_count(_src: &Path, _dst: &Path) -> Result<u64, String> {
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
}

// ============================================================ 盘符

/// 本机盘符（形如 `C:\`）。
#[cfg(windows)]
pub fn drives() -> Vec<String> {
    let mask = unsafe { windows_sys::Win32::Storage::FileSystem::GetLogicalDrives() };
    (0..26u32)
        .filter(|i| mask & (1 << i) != 0)
        .map(|i| format!("{}:\\", (b'A' + i as u8) as char))
        .collect()
}

/// 见 Windows 版。
#[cfg(not(windows))]
pub fn drives() -> Vec<String> {
    vec!["/".to_string()]
}

/// 某个路径所在卷的可用字节。
#[cfg(windows)]
pub fn free_space(p: &Path) -> Option<u64> {
    volume_space(p).map(|(free, _)| free)
}

/// 见 Windows 版。
#[cfg(not(windows))]
pub fn free_space(_p: &Path) -> Option<u64> {
    None
}

/// 某个路径所在卷的 `(可用, 总量)`。
///
/// 界面要画容量条，所以总量也得给 —— 调用系统 API 一次就能拿全，没必要分两次。
#[cfg(windows)]
pub fn volume_space(p: &Path) -> Option<(u64, u64)> {
    let (free, total) = sys::volume_space(&p.to_string_lossy());
    (total > 0).then_some((free, total))
}

/// 见 Windows 版。
#[cfg(not(windows))]
pub fn volume_space(_p: &Path) -> Option<(u64, u64)> {
    None
}

/// Windows 系统调用集中在这里 —— 上面几个公开函数只是薄薄一层包装，
/// 这样平台差异只有一处，也方便对照检查。
#[cfg(windows)]
mod sys {
    use std::os::windows::ffi::OsStrExt;

    /// 取到盘符根（`X:\`）—— 传根最稳，且能避开超长路径的坑。
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

/// 系统盘（形如 `C:`）。
pub fn system_drive() -> Option<String> {
    std::env::var("SystemDrive").ok().map(|s| s.to_uppercase())
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

    #[test]
    fn cleanup_requires_explicit_permanent() {
        let p = std::env::temp_dir().join("acm-nonexistent-cleanup");
        // 不传永久删除 → 必须报错，而不是悄悄做了别的事
        assert!(cleanup(&p, false).is_err());
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
