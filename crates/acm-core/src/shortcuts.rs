//! **快捷方式（.lnk）里加启动参数** —— `launch_arg` 迁移方式的执行部分。
//!
//! 为什么走 PowerShell 的 `WScript.Shell`：读/写 `.lnk` 需要 COM（IShellLink +
//! IPersistFile），在 Rust 里要拉一堆 windows crate；而 `WScript.Shell.CreateShortcut`
//! 是系统自带、语义稳定的一行调用，且我们只在用户点「改用启动参数」时跑一次。
//! 为了避开中文路径的编码坑（PS 5.1 的控制台输出是 GBK），**结果写 JSON 文件再读**，
//! 不走 stdout。
//!
//! 安全边界：只改「目标可执行文件名在 `shortcut_exes` 里」的快捷方式；改之前
//! 把原 `.lnk` 复制到 `%APPDATA%/AgentCache/lnk-backup/<id>-<时间>/`，并写
//! `manifest.json`（原参数 → 新参数），随时可一键还原。

use crate::model::AgentDef;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// 一条被改写的快捷方式。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChangedLink {
    pub path: String,
    pub before: String,
    pub after: String,
}

/// 应用启动参数的结果。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplyReport {
    /// 改成什么目录（展开后的 `--user-data-dir` 目标）
    pub dest: String,
    pub changed: Vec<ChangedLink>,
    /// 目标匹配、但没改的（已经在用同一参数 / 只读）
    pub skipped: Vec<String>,
    pub problems: Vec<String>,
    /// 备份目录（还原时用）
    pub backup: Option<String>,
}

/// 快捷方式可能出现的位置。
fn candidate_dirs() -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut push = |p: Option<PathBuf>| {
        if let Some(p) = p {
            if p.exists() {
                out.push(p);
            }
        }
    };
    // 用户 + 公共桌面
    push(crate::paths::expand("%USERPROFILE%/Desktop"));
    push(crate::paths::expand("%PUBLIC%/Desktop"));
    // 开始菜单（用户 + 全局，含子目录）
    push(crate::paths::expand(
        "%APPDATA%/Microsoft/Windows/Start Menu/Programs",
    ));
    push(crate::paths::expand(
        "%ProgramData%/Microsoft/Windows/Start Menu/Programs",
    ));
    // 任务栏固定项（改完要重启资源管理器才刷新图标区，但生效）
    push(crate::paths::expand(
        "%APPDATA%/Microsoft/Internet Explorer/Quick Launch/User Pinned/TaskBar",
    ));
    out
}

fn collect_lnks(dir: &Path, depth: u8, out: &mut Vec<PathBuf>) {
    if depth > 3 {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            collect_lnks(&p, depth + 1, out);
        } else if p
            .extension()
            .map(|x| x.eq_ignore_ascii_case("lnk"))
            .unwrap_or(false)
        {
            out.push(p);
        }
    }
}

/// 所有候选 `.lnk`（只列路径，读内容交给 PowerShell）。
fn all_links() -> Vec<PathBuf> {
    let mut out = Vec::new();
    for d in candidate_dirs() {
        collect_lnks(&d, 0, &mut out);
    }
    out
}

/// 目标目录：`launch_args` 覆盖的是应用的 userData 根，落点就是 `dest_root/<id>`。
pub fn dest_for(def: &AgentDef, dest_root: &Path) -> PathBuf {
    dest_root.join(&def.id)
}

/// 跑一段 PowerShell 脚本（结果写文件，不走 stdout —— 中文路径不乱码）。
fn run_ps(script_body: &str, out_file: &Path) -> Result<(), String> {
    let sysroot = std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".into());
    let ps = PathBuf::from(&sysroot)
        .join("System32")
        .join("WindowsPowerShell")
        .join("v1.0")
        .join("powershell.exe");

    let script_path = out_file.with_extension("ps1");
    // 带 BOM：Windows PowerShell 5.1 按 ANSI 读无 BOM 的 UTF-8 文件，中文路径会烂
    let body = format!("\u{feff}${script_body}");
    std::fs::write(&script_path, body).map_err(|e| format!("写脚本失败：{e}"))?;

    let out = crate::syscmd::command(&ps.to_string_lossy())
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-File",
            &script_path.to_string_lossy(),
        ])
        .output()
        .map_err(|e| format!("起不了 PowerShell：{e}"))?;
    if !out.status.success() {
        let msg = String::from_utf8_lossy(&out.stdout);
        return Err(format!(
            "PowerShell 退出码 {}：{}",
            out.status.code().unwrap_or(-1),
            msg.chars().take(300).collect::<String>()
        ));
    }
    Ok(())
}

fn read_json<T: for<'de> Deserialize<'de>>(p: &Path) -> Result<T, String> {
    let text = std::fs::read_to_string(p).map_err(|e| format!("读结果失败：{e}"))?;
    // Windows PowerShell 5.1 的 `Out-File -Encoding UTF8` 会带 BOM，serde_json 不认
    let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
    serde_json::from_str(text).map_err(|e| {
        format!(
            "解析结果失败：{e}\n{}",
            text.chars().take(400).collect::<String>()
        )
    })
}

/// 读一批 `.lnk` 的（目标可执行文件, 参数）。
fn read_links(links: &[PathBuf]) -> Result<Vec<(String, String, String)>, String> {
    if links.is_empty() {
        return Ok(Vec::new());
    }
    let tmp = std::env::temp_dir().join(format!("acm-lnk-read-{}", std::process::id()));
    let out_json = tmp.with_extension("json");
    let list: Vec<String> = links
        .iter()
        .map(|p| p.to_string_lossy().to_string())
        .collect();
    let json_lit = serde_json::to_string(&list).map_err(|e| e.to_string())?;
    // 把路径列表塞进脚本：用 here-string 读 JSON 更安全
    let script = format!(
        r#"
$sh = New-Object -ComObject WScript.Shell
$list = ConvertFrom-Json @'
{json_lit}
'@
$rows = @()
foreach ($p in $list) {{
  try {{
    $s = $sh.CreateShortcut($p)
    $rows += [pscustomobject]@{{ p = [string]$p; t = [string]$s.TargetPath; a = [string]$s.Arguments }}
  }} catch {{ }}
}}
ConvertTo-Json -InputObject $rows -Compress | Out-File -FilePath '{out}' -Encoding UTF8
"#,
        out = out_json.to_string_lossy().replace('\'', "''")
    );
    run_ps(&script, &out_json)?;
    let rows: Vec<LinkRow> = read_json(&out_json)?;
    let _ = std::fs::remove_file(&out_json);
    let _ = std::fs::remove_file(out_json.with_extension("ps1"));
    Ok(rows.into_iter().map(|r| (r.p, r.t, r.a)).collect())
}

#[derive(Deserialize)]
struct LinkRow {
    p: String,
    t: String,
    a: String,
}

/// 展开 `--user-data-dir={dest}`，得到最终要写进快捷方式的参数串。
fn build_args(def: &AgentDef, dest: &Path) -> String {
    def.launch_args
        .iter()
        .map(|a| a.replace("{dest}", &dest.to_string_lossy()))
        .collect::<Vec<_>>()
        .join(" ")
}

/// **执行**：把所有指向该应用的快捷方式改成带启动参数，并备份原值。
///
/// 目标目录不存在时先建 —— Chromium 在 `--user-data-dir` 指向不存在目录时会自己建，
/// 但提前建好更稳（也方便迁移把数据搬进去）。
pub fn apply_launch_args(def: &AgentDef, dest_root: &Path) -> Result<ApplyReport, String> {
    if def.launch_args.is_empty() || def.shortcut_exes.is_empty() {
        return Err(format!("{} 没有声明 launch_args / shortcut_exes", def.name));
    }
    let dest = dest_for(def, dest_root);
    if !dest.exists() {
        std::fs::create_dir_all(&dest).map_err(|e| format!("建目标目录失败：{e}"))?;
    }
    let want = build_args(def, &dest);

    let all = all_links();
    let rows = read_links(&all)?;
    let exes: Vec<String> = def
        .shortcut_exes
        .iter()
        .map(|s| s.to_ascii_lowercase())
        .collect();

    let mut rep = ApplyReport {
        dest: dest.to_string_lossy().to_string(),
        ..Default::default()
    };

    let mut targets: Vec<(PathBuf, String)> = Vec::new(); // (lnk 路径, 原参数)
    for (p, t, a) in rows {
        let Some(name) = Path::new(&t).file_name() else {
            continue;
        };
        let name = name.to_string_lossy().to_ascii_lowercase();
        if !exes.iter().any(|e| name == *e) {
            continue;
        }
        if a.contains("--user-data-dir") {
            rep.skipped.push(format!("{p}（已经在用 --user-data-dir）"));
            continue;
        }
        targets.push((PathBuf::from(&p), a));
    }

    if targets.is_empty() {
        rep.problems.push(
            "没有找到指向该应用的快捷方式（桌面 / 开始菜单 / 任务栏都没有）。\
             可以直接右键目标程序 → 属性 → 目标 里手写参数："
                .to_string(),
        );
        return Ok(rep);
    }

    // 备份：原 .lnk 复制一份 + manifest（原/新参数），还原时按 manifest 写回
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S").to_string();
    let backup = crate::paths::expand("%APPDATA%/AgentCache/lnk-backup")
        .ok_or_else(|| "展开 %APPDATA% 失败".to_string())?
        .join(format!("{}-{stamp}", def.id));
    std::fs::create_dir_all(&backup).map_err(|e| format!("建备份目录失败：{e}"))?;
    let mut changed = Vec::new();
    for (p, before) in &targets {
        let name = p.file_name().map(|x| x.to_string_lossy().to_string()).unwrap_or_default();
        // 同名 .lnk 可能出现在多个目录（桌面 + 开始菜单），用序号区分
        let safe = format!(
            "{}-{}",
            changed.len(),
            name.replace(['\\', '/', ':'], "_")
        );
        let _ = std::fs::copy(p, backup.join(&safe));
        changed.push(ChangedLink {
            path: p.to_string_lossy().to_string(),
            before: before.clone(),
            after: want.clone(),
        });
    }
    let manifest = serde_json::json!({
        "agent": def.id,
        "dest": rep.dest,
        "newArgs": want,
        "links": changed,
    });
    std::fs::write(
        backup.join("manifest.json"),
        serde_json::to_string_pretty(&manifest).map_err(|e| e.to_string())?,
    )
    .map_err(|e| format!("写 manifest 失败：{e}"))?;

    // 写回：一条脚本处理所有目标
    let tmp = std::env::temp_dir().join(format!("acm-lnk-write-{}", std::process::id()));
    let out_json = tmp.with_extension("json");
    let jobs: Vec<serde_json::Value> = changed
        .iter()
        .map(|c| serde_json::json!({ "p": c.path, "a": c.after }))
        .collect();
    let jobs_lit = serde_json::to_string(&jobs).map_err(|e| e.to_string())?;
    let script = format!(
        r#"
$sh = New-Object -ComObject WScript.Shell
$jobs = ConvertFrom-Json @'
{jobs_lit}
'@
$res = @()
foreach ($j in $jobs) {{
  try {{
    $s = $sh.CreateShortcut([string]$j.p)
    $s.Arguments = [string]$j.a
    $s.Save()
    $res += [pscustomobject]@{{ p = [string]$j.p; ok = $true; err = "" }}
  }} catch {{
    $res += [pscustomobject]@{{ p = [string]$j.p; ok = $false; err = $_.Exception.Message }}
  }}
}}
ConvertTo-Json -InputObject $res -Compress | Out-File -FilePath '{out}' -Encoding UTF8
"#,
        out = out_json.to_string_lossy().replace('\'', "''")
    );
    run_ps(&script, &out_json)?;

    #[derive(Deserialize)]
    struct R {
        #[allow(dead_code)]
        p: String,
        ok: bool,
        err: String,
    }
    let res: Vec<R> = read_json(&out_json)?;
    let _ = std::fs::remove_file(&out_json);
    let _ = std::fs::remove_file(out_json.with_extension("ps1"));

    let mut ok_links = Vec::new();
    for (i, r) in res.iter().enumerate() {
        if r.ok {
            ok_links.push(changed[i].clone());
        } else {
            rep.problems
                .push(format!("{} 写不进去：{}", changed[i].path, r.err));
        }
    }
    rep.changed = ok_links;
    rep.backup = Some(backup.to_string_lossy().to_string());
    Ok(rep)
}

/// **还原**：按备份目录里的 manifest 把参数写回原值。
pub fn restore(backup_dir: &Path) -> Result<usize, String> {
    let manifest_path = backup_dir.join("manifest.json");
    let text = std::fs::read_to_string(&manifest_path)
        .map_err(|e| format!("读不了 {}：{e}", manifest_path.display()))?;
    #[derive(Deserialize)]
    struct M {
        #[allow(dead_code)]
        #[serde(default)]
        links: Vec<ChangedLink>,
    }
    let m: M = serde_json::from_str(&text).map_err(|e| format!("manifest 解析失败：{e}"))?;
    if m.links.is_empty() {
        return Ok(0);
    }
    let tmp = std::env::temp_dir().join(format!("acm-lnk-restore-{}", std::process::id()));
    let out_json = tmp.with_extension("json");
    let jobs: Vec<serde_json::Value> = m
        .links
        .iter()
        .map(|l| serde_json::json!({ "p": l.path, "a": l.before }))
        .collect();
    let jobs_lit = serde_json::to_string(&jobs).map_err(|e| e.to_string())?;
    let script = format!(
        r#"
$sh = New-Object -ComObject WScript.Shell
$jobs = ConvertFrom-Json @'
{jobs_lit}
'@
$n = 0
foreach ($j in $jobs) {{
  try {{
    $s = $sh.CreateShortcut([string]$j.p)
    $s.Arguments = [string]$j.a
    $s.Save()
    $n++
  }} catch {{ }}
}}
$n | Out-File -FilePath '{out}' -Encoding UTF8
"#,
        out = out_json.to_string_lossy().replace('\'', "''")
    );
    run_ps(&script, &out_json)?;
    let n: usize = std::fs::read_to_string(&out_json)
        .map_err(|e| e.to_string())?
        .trim()
        .trim_start_matches('\u{feff}')
        .trim()
        .parse()
        .unwrap_or(0);
    let _ = std::fs::remove_file(&out_json);
    let _ = std::fs::remove_file(out_json.with_extension("ps1"));
    Ok(n)
}

/// 列出已有的备份（给界面「还原启动参数」用）。
pub fn backups(agent_id: &str) -> Vec<String> {
    let Some(root) = crate::paths::expand("%APPDATA%/AgentCache/lnk-backup") else {
        return Vec::new();
    };
    let Ok(rd) = std::fs::read_dir(root) else { return Vec::new() };
    let mut out: Vec<String> = rd
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with(&format!("{agent_id}-")))
        .map(|e| e.path().to_string_lossy().to_string())
        .collect();
    out.sort();
    out.reverse();
    out
}
