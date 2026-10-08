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

/// 一个盘符的信息（界面上画容量条用）。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DriveInfo {
    /// `C:\`
    pub letter: String,
    pub free: u64,
    pub total: u64,
    pub free_text: String,
    pub used_percent: u8,
    pub is_system: bool,
    /// 是不是当前自动挑中的目标根所在的盘
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
            .map(|p| drive_letter(p))
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
#[tauri::command]
async fn migrate_agent(
    app: tauri::AppHandle,
    agent_id: String,
    dest_root: String,
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
        let rep = migrate::run(&p, &mut cb);
        serde_json::to_value(rep).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("迁移任务异常：{e}"))?
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
/// 只取**第一个真实存在**的数据目录 —— 一个 Agent 声明多个目录时，
/// 界面上的「迁移」按钮对应的是「这个 Agent 的主目录」。
/// 需要逐个处理的场景交给 `plan`/CLI。
fn resolve(agent_id: &str) -> Result<(acm_core::AgentDef, PathBuf), String> {
    let defs = agents::load(None)?;
    let def = defs
        .into_iter()
        .find(|d| d.id == agent_id)
        .ok_or_else(|| format!("注册表里没有 id = {agent_id}"))?;

    for tpl in &def.paths {
        if let Some(p) = paths::expand(tpl) {
            if p.exists() {
                return Ok((def, p));
            }
        }
    }
    Err(format!("{} 的数据目录都不存在", def.name))
}

fn drive_letter(p: &Path) -> String {
    let s = p.to_string_lossy();
    match s.find(":\\") {
        Some(i) => s[..i + 2].to_string(),
        None => String::new(),
    }
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
            rollback,
            snapshots,
            delete_snapshot,
            open_path,
            version,
        ])
        .run(tauri::generate_context!())
        .expect("AgentCache 启动失败");
}
