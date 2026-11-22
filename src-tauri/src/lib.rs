//! AgentCache 桌面端：Tauri 命令层。
//!
//! 设计原则：**这里只做「翻译」** —— 把内核的类型转成前端能用的形状、
//! 把长任务丢到后台线程、把进度 emit 出去。真正的逻辑一律在 `acm-core`，
//! 这样 CLI 和 GUI 走的是同一套代码，出问题能用 `acm` 复现。

use acm_core::{agents, fsutil, migrate, paths, DetectOpts};
use serde::Serialize;
use serde_json::Value;
use std::path::{Path, PathBuf};
use tauri::Emitter;

/// 进度事件名 —— 前端 `listen("migrate-progress")`。
const PROGRESS_EVENT: &str = "migrate-progress";

/// 一个可选的目标位置。
///
/// ⚠️ 字段名叫 `letter` 是历史原因 —— **unix 上它是挂载点**（`/home`、`/mnt/data`），
/// 不是盘符。前端已经按「一个根路径」来用，不当作盘符字面量。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DriveInfo {
    /// Windows：`C:\`；unix：挂载点。
    pub letter: String,
    pub free: u64,
    pub total: u64,
    pub free_text: String,
    pub used_percent: u8,
    pub is_system: bool,
    /// 是不是当前自动挑中的目标根所在的位置
    pub is_default_target: bool,
}

/// 检测全部 Agent。
///
/// `measure = false` 时只判存在性，几乎瞬时 —— 界面首屏先用它渲染，
/// 再后台跑一次带体积的。
#[tauri::command]
async fn detect(
    measure: bool,
    only: Option<String>,
) -> Result<Vec<acm_core::DetectedAgent>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let mut defs = agents::load(None)?;
        if let Some(id) = only {
            defs.retain(|d| d.id == id);
        }
        let opts = DetectOpts {
            measure,
            ..Default::default()
        };
        Ok(agents::detect_all(&defs, &opts))
    })
    .await
    .map_err(|e| format!("检测任务异常：{e}"))?
}

/// 列出注册表（不检测）。
#[tauri::command]
async fn list_agents() -> Result<Vec<acm_core::AgentDef>, String> {
    tauri::async_runtime::spawn_blocking(|| agents::load(None))
        .await
        .map_err(|e| format!("读注册表异常：{e}"))?
}

/// 盘符列表 + 自动挑中的目标根。
#[tauri::command]
async fn drives() -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(|| {
        let sys = migrate::system_drive();
        let default_root = migrate::default_dest_root();
        let default_drive = default_root
            .as_ref()
            .map(|p| root_of(p))
            .unwrap_or_default();

        let list: Vec<DriveInfo> = migrate::drives()
            .into_iter()
            .map(|d| {
                let (free, total) = migrate::volume_space(Path::new(&d)).unwrap_or((0, 0));
                let used_percent = if total == 0 {
                    0
                } else {
                    (((total - free) as f64 / total as f64) * 100.0)
                        .round()
                        .clamp(0.0, 100.0) as u8
                };
                let letter_trim = d.trim_end_matches('\\').to_string();
                DriveInfo {
                    is_system: Some(letter_trim.clone()) == sys,
                    is_default_target: !default_drive.is_empty() && letter_trim == default_drive,
                    free_text: fsutil::human(free),
                    used_percent,
                    free,
                    total,
                    letter: d,
                }
            })
            .collect();

        Ok(serde_json::json!({
            "drives": list,
            "defaultDestRoot": default_root.map(|p| p.to_string_lossy().to_string()),
            "systemDrive": sys,
        }))
    })
    .await
    .map_err(|e| format!("读盘符异常：{e}"))?
}

/// 为某个 Agent 生成迁移计划（**不动任何文件**）。
#[tauri::command]
async fn plan(agent_id: String, dest_root: String) -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let (def, src) = resolve(&agent_id)?;
        let p = migrate::plan(&def, &src, Path::new(&dest_root))?;
        serde_json::to_value(p).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("生成计划异常：{e}"))?
}

/// 执行迁移。进度通过 `migrate-progress` 事件推送。
///
/// `with_env`：迁移成功后顺带设置官方重定位环境变量（联接 + 变量双保险）；
/// 结果作为 `set-env` 步骤附加到报告里，失败不推翻迁移本身。
#[tauri::command]
async fn migrate_agent(
    app: tauri::AppHandle,
    agent_id: String,
    dest_root: String,
    with_env: Option<bool>,
) -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let (def, src) = resolve(&agent_id)?;
        let p = migrate::plan(&def, &src, Path::new(&dest_root))?;
        if !p.blockers.is_empty() {
            return Err(p.blockers.join("；"));
        }
        let emit_app = app.clone();
        let mut cb = move |pr: migrate::Progress| {
            // 事件发失败（窗口关了）不该中断迁移
            let _ = emit_app.emit(PROGRESS_EVENT, &pr);
        };
        let mut rep = migrate::run(&p, &mut cb);
        if with_env.unwrap_or(false) && rep.ok {
            let detail = match migrate::env_relocate_apply(&def, Some(Path::new(&dest_root)), false)
            {
                Ok(msg) => migrate::StepResult {
                    name: "set-env".into(),
                    ok: true,
                    detail: msg,
                },
                Err(e) => migrate::StepResult {
                    name: "set-env".into(),
                    ok: false,
                    detail: format!("{e}（迁移本身已成功，不影响使用）"),
                },
            };
            rep.steps.push(detail);
        }
        serde_json::to_value(rep).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("迁移任务异常：{e}"))?
}

/// 请求取消当前正在进行的迁移（复制循环在行间响应；已复制的部分保留可续传）。
#[tauri::command]
async fn cancel_migrate() -> Result<(), String> {
    migrate::request_cancel();
    Ok(())
}

/// 回滚某个 Agent（把快照改回原位）。
#[tauri::command]
async fn rollback(agent_id: String) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let (def, src) = resolve(&agent_id)?;
        migrate::rollback(&def, &src)
    })
    .await
    .map_err(|e| format!("回滚任务异常：{e}"))?
}

/// 修复模式：目标数据还在但链接坏了/丢了，只重建链接不复制数据。
///
/// ⚠️ 与 `resolve` 不同：src 可能是一个**断链**（`exists()` 为 false），
/// 也可能整个没了（父目录还在就行），所以解析规则放宽到 symlink/父目录。
#[tauri::command]
async fn repair(agent_id: String, dest_root: String) -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let defs = agents::load(None)?;
        let def = defs
            .iter()
            .find(|d| d.id == agent_id)
            .ok_or_else(|| format!("注册表里没有 id = {agent_id}"))?;
        let src = def
            .paths
            .iter()
            .find_map(|tpl| {
                paths::expand(tpl).filter(|p| {
                    p.symlink_metadata().is_ok() || p.parent().is_some_and(|pp| pp.exists())
                })
            })
            .ok_or_else(|| format!("{} 的数据路径连父目录都不存在，无法修复", def.name))?;
        let rep = migrate::repair(def, &src, Path::new(&dest_root))?;
        serde_json::to_value(rep).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("修复任务异常：{e}"))?
}

/// 列出所有迁移快照。
#[tauri::command]
async fn snapshots() -> Result<Vec<migrate::Snapshot>, String> {
    tauri::async_runtime::spawn_blocking(|| {
        let defs = agents::load(None)?;
        Ok(migrate::list_snapshots(&defs))
    })
    .await
    .map_err(|e| format!("列快照异常：{e}"))?
}

/// 设置/删除 Agent 官方支持的重定位环境变量（仅限配了 env_relocate 的）。
#[tauri::command]
async fn env_set(
    agent_id: String,
    dest_root: Option<String>,
    unset: Option<bool>,
) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let defs = agents::load(None)?;
        let def = defs
            .iter()
            .find(|d| d.id == agent_id)
            .ok_or_else(|| format!("注册表里没有 id = {agent_id}"))?;
        migrate::env_relocate_apply(
            def,
            dest_root.as_deref().map(Path::new),
            unset.unwrap_or(false),
        )
    })
    .await
    .map_err(|e| format!("设置环境变量异常：{e}"))?
}

/// 清理某个 Agent 的缓存目录（移入回收站）。
///
/// 用在「应用早卸载了但残留还占着空间」的场景 —— 清完再扫一次，它就落到未安装。
/// `preview = true` 时只列清单不动文件（确认框用）。
#[tauri::command]
async fn purge_agent(agent_id: String, preview: Option<bool>) -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || {
        if preview.unwrap_or(false) {
            let defs = agents::load(None)?;
            let def = defs
                .iter()
                .find(|d| d.id == agent_id)
                .ok_or_else(|| format!("注册表里没有 id = {agent_id}"))?;
            let items = acm_core::purge::preview(def);
            serde_json::to_value(serde_json::json!({
                "ok": true,
                "items": items,
                "freed": 0,
                "freedText": "",
                "problems": [],
            }))
            .map_err(|e| e.to_string())
        } else {
            let rep = acm_core::purge::purge_by_id(&agent_id)?;
            serde_json::to_value(rep).map_err(|e| e.to_string())
        }
    })
    .await
    .map_err(|e| format!("清理任务异常：{e}"))?
}

/// 结束某个 Agent 的进程 —— 详情页「运行状态」旁边的「结束进程」按钮。
///
/// 场景：`tasklist` 名字匹配当兜底，偶尔漏（进程刚起 / 权限 / 名字对不上），
/// 用户明明自己关了，界面还说「运行中」，迁移也被拦着。与其让他去任务管理器
/// 里翻，不如给一个明确目标：按注册表的 `processes` 结束，`ignore_processes`
/// 里的常驻服务**不碰**（杀了可能让应用再也起不来）。
#[tauri::command]
async fn close_agent(agent_id: String) -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let defs = agents::load(None)?;
        let def = defs
            .iter()
            .find(|d| d.id == agent_id)
            .ok_or_else(|| format!("注册表里没有 id = {agent_id}"))?;
        let rep = acm_core::procs::kill_agent_processes(&def.processes, &def.ignore_processes);
        serde_json::to_value(rep).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("结束进程任务异常：{e}"))?
}

/// 「改用启动参数（--user-data-dir）」：把桌面 / 开始菜单 / 任务栏里指向该应用的
/// 快捷方式改成带参数启动，并备份原值（`restore_launch_args` 可还原）。
#[tauri::command]
async fn apply_launch_args(agent_id: String, dest_root: String) -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let defs = agents::load(None)?;
        let def = defs
            .iter()
            .find(|d| d.id == agent_id)
            .ok_or_else(|| format!("注册表里没有 id = {agent_id}"))?;
        let rep =
            acm_core::shortcuts::apply_launch_args(def, std::path::Path::new(&dest_root))?;
        serde_json::to_value(rep).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("改快捷方式任务异常：{e}"))?
}

/// 还原启动参数：把最近一次备份的 `.lnk` 参数写回去。
#[tauri::command]
async fn restore_launch_args(agent_id: String) -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let backups = acm_core::shortcuts::backups(&agent_id);
        let Some(latest) = backups.first() else {
            return Err(format!("{agent_id} 没有可还原的备份"));
        };
        let n = acm_core::shortcuts::restore(std::path::Path::new(latest))?;
        Ok(serde_json::json!({ "restored": n, "backup": latest }))
    })
    .await
    .map_err(|e| format!("还原任务异常：{e}"))?
}

/// 清空迁移历史 —— 界面上「迁移历史」卡片里的「清空」按钮。
#[tauri::command]
async fn clear_history() -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let n = migrate::clear_history()?;
        Ok(serde_json::json!({ "removed": n }))
    })
    .await
    .map_err(|e| format!("清空历史异常：{e}"))?
}

/// 精确统计一个 Agent 的数据目录（不设上限）—— 列表里的数是「扫到上限为止」
/// 的下限（`≥`），用户想看准数时点一下，走这里慢慢算。
#[tauri::command]
async fn measure_agent(agent_id: String) -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let defs = agents::load(None)?;
        let def = defs
            .into_iter()
            .find(|d| d.id == agent_id)
            .ok_or_else(|| format!("注册表里没有 id = {agent_id}"))?;

        let mut total = acm_core::fsutil::Measure::default();
        let mut paths = Vec::new();
        for tpl in &def.paths {
            let Some(p) = paths::expand(tpl) else { continue };
            if !p.exists() {
                continue;
            }
            let m = acm_core::fsutil::measure_exact(&p);
            total.files += m.files;
            total.bytes += m.bytes;
            total.capped |= m.capped;
            paths.push(serde_json::json!({
                "resolved": p.to_string_lossy(),
                "bytes": m.bytes,
                "files": m.files,
            }));
        }

        Ok(serde_json::json!({ "total": total, "paths": paths }))
    })
    .await
    .map_err(|e| format!("精确统计异常：{e}"))?
}

/// 迁移历史（`history.jsonl`，新的在前；默认最近 15 条）。
#[tauri::command]
async fn history(all: Option<bool>) -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let p = migrate::history_path().ok_or("定位不了历史文件（缺少用户配置目录）")?;
        let mut items: Vec<Value> = if p.exists() {
            let text = std::fs::read_to_string(&p).map_err(|e| e.to_string())?;
            text.lines()
                .filter(|l| !l.trim().is_empty())
                .filter_map(|l| serde_json::from_str(l).ok())
                .collect()
        } else {
            Vec::new()
        };
        items.reverse(); // 新的在前
        if !all.unwrap_or(false) {
            items.truncate(15);
        }
        Ok(serde_json::json!({
            "items": items,
            "path": p.to_string_lossy(),
        }))
    })
    .await
    .map_err(|e| format!("读历史异常：{e}"))?
}

/// 删除一个快照。
///
/// `permanent` 必须显式为 true —— 删掉就不能回滚了，这个选择必须由用户做出。
#[tauri::command]
async fn delete_snapshot(path: String, permanent: bool) -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let freed = migrate::cleanup(Path::new(&path), permanent)?;
        Ok(serde_json::json!({
            "ok": true,
            "freed": freed,
            "freedText": fsutil::human(freed),
        }))
    })
    .await
    .map_err(|e| format!("删除任务异常：{e}"))?
}

/// 用系统默认程序打开路径（目录会在资源管理器里打开）。
#[tauri::command]
async fn open_path(app: tauri::AppHandle, path: String) -> Result<(), String> {
    use tauri_plugin_opener::OpenerExt;
    let p = PathBuf::from(&path);
    if !p.exists() {
        return Err(format!("路径不存在：{path}"));
    }
    app.opener()
        .open_path(path, None::<&str>)
        .map_err(|e| format!("打不开：{e}"))
}

/// 内核版本（界面右下角显示，排障时对得上）。
#[tauri::command]
fn version() -> Value {
    serde_json::json!({
        "app": env!("CARGO_PKG_VERSION"),
        "core": env!("CARGO_PKG_VERSION"),
    })
}

// ---------------------------------------------------------------- 内部

/// 把 agent id 解析成 (定义, 源目录)。
///
/// 取**第一个还没被迁移的真实目录**：一个 Agent 声明多个数据目录时（豆包就是
/// `%LOCALAPPDATA%\Doubao` 真数据 + `%APPDATA%\Doubao` 几个配置文件），
/// 主目录迁完之后，「迁移」按钮应该指向下一个还没迁的目录，
/// 而不是每次都撞上「已经是联接了」这句废话。
/// 全迁完了才退回第一个存在的路径 —— 让 `plan` 报「已经迁移过」。
fn resolve(agent_id: &str) -> Result<(acm_core::AgentDef, PathBuf), String> {
    let defs = agents::load(None)?;
    let def = defs
        .into_iter()
        .find(|d| d.id == agent_id)
        .ok_or_else(|| format!("注册表里没有 id = {agent_id}"))?;

    let mut migrated: Option<PathBuf> = None;
    for tpl in &def.paths {
        let Some(p) = paths::expand(tpl) else { continue };
        if acm_core::fsutil::is_link(&p) {
            migrated.get_or_insert(p);
            continue;
        }
        if p.exists() {
            return Ok((def, p));
        }
    }
    if let Some(p) = migrated {
        return Ok((def, p));
    }
    Err(format!("{} 的数据目录都不存在", def.name))
}

/// 取路径所在的「根」。
///
/// - **Windows**：盘符（`C:\`、`D:\`…）
/// - **unix**：挂载点（`/`、`/home`、`/mnt/data`…）—— 取**最长**的匹配前缀，
///   因为挂载点是可嵌套的（`/` 和 `/mnt/data` 会同时存在，后者更具体）
fn root_of(p: &Path) -> String {
    let s = p.to_string_lossy();
    if let Some(i) = s.find(":\\") {
        return s[..i + 2].to_string();
    }
    migrate::drives()
        .into_iter()
        .filter(|m| {
            let m = m.trim_end_matches('/');
            let m = if m.is_empty() { "/" } else { m };
            m == "/" || s == m || s.starts_with(&format!("{m}/"))
        })
        .max_by_key(|m| m.len())
        .unwrap_or_else(|| "/".to_string())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            detect,
            list_agents,
            drives,
            plan,
            migrate_agent,
            cancel_migrate,
            rollback,
            repair,
            snapshots,
            history,
            clear_history,
            measure_agent,
            env_set,
            purge_agent,
            close_agent,
            apply_launch_args,
            restore_launch_args,
            delete_snapshot,
            open_path,
            version,
        ])
        .run(tauri::generate_context!())
        .expect("AgentCache 启动失败");
}
