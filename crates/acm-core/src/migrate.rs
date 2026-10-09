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
    match imp::copy_tree(&src, &dest) {
        Ok(()) => r.steps.push(StepResult {
            name: "复制".into(),
            ok: true,
            detail: "复制完成（只补不删）".into(),
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
    match imp::pending_count(&src, &dest) {
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
        imp::remove_junction(src)?;
    }
    imp::rename_back(&snap, src)?;
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
    imp::remove_dir_forced(snapshot)
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

    /// 递归复制。**只补不删** —— 目标里多出来的文件保留
    /// （那通常是上次迁移留下的有效增量，或应用自己新写的）。
    pub fn copy_tree(src: &Path, dst: &Path) -> Result<(), String> {
        copy_dir(src, dst)
            .map_err(|e| format!("复制 {} → {} 失败：{e}", src.display(), dst.display()))
    }

    fn copy_dir(src: &Path, dst: &Path) -> std::io::Result<()> {
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
                copy_dir(&from, &to)?;
            } else {
                copy_file(&from, &to, &md)?;
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
    pub fn pending_count(src: &Path, dst: &Path) -> Result<u64, String> {
        let mut n = 0u64;
        walk(src, src, &mut |rel: &Path, md: &std::fs::Metadata| {
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
            copy_tree(&src, &dst).unwrap();
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
            assert_eq!(pending_count(&src, &dst).unwrap(), 0);

            // ③ 新增一个文件 → 校验要能发现
            std::fs::write(src.join("c.txt"), b"new").unwrap();
            assert_eq!(pending_count(&src, &dst).unwrap(), 1);

            // ④ 再复制一次（增量）→ 应当收敛，且新文件确实到了目标
            //    （这条是补上的：早先只在 ③ 之后直接断言目标条目数，
            //     忘了 c.txt 还没被复制 —— 测试自己把这一步暴露了出来）
            copy_tree(&src, &dst).unwrap();
            assert_eq!(pending_count(&src, &dst).unwrap(), 0, "增量复制后应当收敛");
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

            assert_eq!(pending_count(&src, &dst).unwrap(), 2);

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
