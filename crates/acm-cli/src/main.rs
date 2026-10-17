//! `acm` —— AgentCache 命令行。
//!
//! 存在的意义：**让内核可以脱离 GUI 被验证和排障**。
//! 界面上看到的数据，这里都能原样打出来，出问题时不用猜。

use acm_core::{agents, fsutil, migrate, paths, DetectOpts};
use std::path::PathBuf;
use std::process::ExitCode;

const USAGE: &str = "\
acm —— AgentCache 命令行

用法:
  acm detect [选项]          检测本机装了哪些 Agent  acm list                   列出注册表里的 Agent（不检测）
  acm plan <id> [--dest <根>]  为某个 Agent 生成迁移计划（**不动任何文件**）
  acm migrate <id> [选项]    执行迁移（复制 → 校验 → 快照 → 联接，失败自动回滚）
                            --with-env：迁移成功后顺带设置官方重定位环境变量
                            （联接 + 变量双保险；仅限配了 env_relocate 的 Agent）
  acm rollback <id> [--yes]  回滚：把快照改回原位（迁移后新数据会先备份到恢复目录）
  acm repair <id> [选项]     修复：目标数据还在但链接坏了/丢了，只重建链接不复制数据
  acm env <id> [选项]        设置/删除 Agent 官方支持的重定位环境变量（仅限配了 env_relocate 的）
  acm snapshots              列出所有迁移快照（原目录改名保留的那份）
  acm history [--all]        查看迁移历史（最近 20 条，--all 全部）
  acm doctor [--json]        体检：断链 / 运行中 / 快照脏状态 / 孤儿快照，附修复命令
  acm drives                 列出盘符与剩余空间
  acm help                   显示这段帮助

detect 选项:
  --json                 输出 JSON（给脚本/排障用）
  --no-measure           只判存在性，不量体积（快很多）
  --only <id>            只检测某个 id
  --config <路径>        用指定的 agents.toml（默认：内置 + 用户目录覆盖）

plan 选项:
  --dest <根目录>        目标根目录（会拼上 agent id 作为子目录）；不给他们会自动挑
  --config <路径>        同上

migrate 选项:
  --dest <根目录>        同 plan
  --yes                  跳过交互确认（脚本/自动化用；--json 也隐含 --yes）
  --json                 输出 JSON 报告（进度走 stderr）
  --config <路径>        同上

rollback 选项:
  --yes                  跳过交互确认
  --config <路径>        同上

repair 选项:
  --dest <根目录>        数据所在的根（会拼上 agent id 作为子目录）；不给他们会自动挑
  --yes                  跳过交互确认
  --config <路径>        同上

env 选项:
  --dest <根目录>        变量要指向的根（会拼上 agent id 作为子目录）；不给他们会自动挑
  --unset                删除该变量（恢复应用默认路径）
  --yes                  跳过交互确认
  --config <路径>        同上
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cmd = args.first().map(String::as_str).unwrap_or("wizard");

    let r = match cmd {
        "detect" => cmd_detect(&args[1..]),
        "list" => cmd_list(&args[1..]),
        "plan" => cmd_plan(&args[1..]),
        "migrate" => cmd_migrate(&args[1..]),
        "rollback" => cmd_rollback(&args[1..]),
        "repair" => cmd_repair(&args[1..]),
        "env" => cmd_env(&args[1..]),
        "snapshots" => cmd_snapshots(&args[1..]),
        "history" => cmd_history(&args[1..]),
        "doctor" => cmd_doctor(&args[1..]),
        "wizard" => cmd_wizard(args.get(1..).unwrap_or(&[])),
        "drives" => cmd_drives(),
        "help" | "-h" | "--help" => {
            print!("{USAGE}");
            Ok(())
        }
        "version" | "-V" | "--version" => {
            println!("acm {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        other => Err(format!("不认识的命令：{other}\n\n{USAGE}")),
    };

    match r {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("错误：{e}");
            ExitCode::FAILURE
        }
    }
}

// ---------------------------------------------------------------- list

fn cmd_list(args: &[String]) -> Result<(), String> {
    let cfg = flag_path(args, "--config");
    let defs = agents::load(cfg.as_deref())?;
    println!("注册表：{} 条", defs.len());
    for d in &defs {
        println!(
            "  {:<16} {:<20} class={:<8} paths={}",
            d.id,
            d.name,
            d.class,
            d.paths.join(", ")
        );
    }
    Ok(())
}

// ---------------------------------------------------------------- detect

fn cmd_detect(args: &[String]) -> Result<(), String> {
    let json = args.iter().any(|a| a == "--json");
    let no_measure = args.iter().any(|a| a == "--no-measure");
    let only = flag_str(args, "--only");
    let cfg = flag_path(args, "--config");

    let mut defs = agents::load(cfg.as_deref())?;
    if let Some(id) = &only {
        defs.retain(|d| &d.id == id);
        if defs.is_empty() {
            return Err(format!("注册表里没有 id = {id}"));
        }
    }

    let opts = DetectOpts {
        measure: !no_measure,
        ..Default::default()
    };

    // 量体积要时间，先把「正在扫」告诉用户，别让人以为卡死
    if !json && opts.measure {
        eprintln!("正在检测 {} 个 Agent（量体积，稍等）…", defs.len());
    }
    let all = agents::detect_all(&defs, &opts);

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&all).map_err(|e| e.to_string())?
        );
        return Ok(());
    }

    print_report(&all, opts.measure);
    Ok(())
}

fn print_report(all: &[acm_core::DetectedAgent], measured: bool) {
    const LINE: &str = "─────────────────────────────────────────────────────────────────────";
    println!("\nAgentCache · 检测报告");
    println!("{LINE}");

    for a in all {
        let (mark, tag) = match a.state() {
            "moved" => ("●", a.state_label()),
            "free" => ("○", a.state_label()),
            _ => ("·", a.state_label()),
        };
        // 「正在运行」是另一个维度的事实，跟迁移状态并列显示，不互相覆盖
        let tag = if a.running.is_empty() {
            tag.to_string()
        } else {
            format!("{tag}·运行中")
        };

        let size = if !a.detected {
            String::new()
        } else if !measured {
            "未测量（--no-measure）".to_string()
        } else {
            let s = fsutil::human(a.total.bytes);
            let t = if a.total.capped { format!("≥{s}") } else { s };
            format!("{t:>10}  {:>7} 文件", a.total.files)
        };

        println!("{mark} [{tag:<12}] {:<22} {size}", a.name);

        for p in &a.paths {
            if p.kind == "missing" {
                continue;
            }
            println!("      {}", p.resolved);
            if let Some(t) = &p.link_target {
                let flag = if p.link_broken { "  ⚠ 断链" } else { "" };
                println!("        → {t}{flag}");
            }
        }
        for u in &a.updaters {
            println!("      更新器存在：{}", u.to_string_lossy());
        }
        for b in &a.broken_links {
            println!(
                "      ⚠ 断链联接：{}  →  {}",
                b.path,
                b.target.as_deref().unwrap_or("?")
            );
        }
        if !a.running.is_empty() {
            println!("      ⚠ 正在运行：{}", a.running.join(", "));
        }
        if let Some(n) = &a.note {
            println!("      提示：{n}");
        }
        println!();
    }

    println!("{LINE}");
    let detected = all.iter().filter(|a| a.detected).count();
    let moved = all.iter().filter(|a| a.state() == "moved").count();
    let running = all.iter().filter(|a| !a.running.is_empty()).count();
    let broken = all.iter().filter(|a| !a.broken_links.is_empty()).count();
    println!(
        "共 {} 项：检测到 {detected}，已迁移 {moved}，运行中 {running}，有断链 {broken}",
        all.len()
    );
}

/// 取**位置参数**（agent id）。
///
/// 🔴 不能用「第一个不以 -- 开头的参数」：`acm migrate --dest D:\x codex`
/// 里 `D:\x` 也不以 -- 开头，会被误当成 id。已知带值的选项后面要跳过。
fn positional(args: &[String], usage: &str) -> Result<String, String> {
    let mut skip_next = false;
    for a in args {
        if skip_next {
            skip_next = false;
            continue;
        }
        if a == "--dest" || a == "--config" || a == "--only" {
            skip_next = true;
            continue;
        }
        if a.starts_with("--") {
            continue;
        }
        return Ok(a.clone());
    }
    Err(usage.to_string())
}

// ---------------------------------------------------------------- plan

fn cmd_plan(args: &[String]) -> Result<(), String> {
    let id = positional(args, "用法：acm plan <agent-id> [--dest <目标根目录>]")?;
    let cfg = flag_path(args, "--config");

    let dest_root = match flag_path(args, "--dest") {
        Some(d) => d,
        None => migrate::default_dest_root()
            .ok_or("没能自动挑出目标盘（可能只有一个盘），请用 --dest 指定")?,
    };
    println!("目标根目录：{}\n", dest_root.display());

    let defs = agents::load(cfg.as_deref())?;
    let def = defs
        .iter()
        .find(|d| d.id == id)
        .ok_or_else(|| format!("注册表里没有 id = {id}"))?;

    let mut any = false;
    for tpl in &def.paths {
        let Some(src) = paths::expand(tpl) else {
            continue;
        };
        if !src.exists() {
            println!("{tpl} → {}（不存在，跳过）\n", src.display());
            continue;
        }
        any = true;
        match migrate::plan(def, &src, &dest_root) {
            Ok(p) => print_plan(&p),
            Err(e) => println!("✗ {tpl}\n   {e}\n"),
        }
    }
    if !any {
        return Err(format!("{id} 的数据目录都不存在，没什么可迁的"));
    }
    println!("（这只是计划 —— acm plan 不动任何文件）");
    Ok(())
}

fn print_plan(p: &migrate::Plan) {
    println!("【{}】{}", p.agent_id, p.agent_name);
    println!("  源　　{}", p.src);
    println!(
        "  占用　{}  {} 文件{}",
        fsutil::human(p.src_measure.bytes),
        p.src_measure.files,
        if p.src_measure.capped {
            "（下限，已截断）"
        } else {
            ""
        }
    );
    println!("  目标　{}", p.dest);
    println!(
        "  目标盘可用　{}{}",
        fsutil::human(p.dest_free),
        if p.fits { "" } else { "  ⚠ 可能不够" }
    );
    if p.dest_existing.files > 0 {
        println!(
            "  目标已有　{}  {} 文件（增量补齐，不会删）",
            fsutil::human(p.dest_existing.bytes),
            p.dest_existing.files
        );
    }
    if p.large_files > 0 {
        println!(
            "  大文件　{} 个 ≥1GB{}",
            p.large_files,
            if p.large_files_capped {
                "（下限）"
            } else {
                ""
            }
        );
    }
    println!("  快照　{}<时间戳>", p.snapshot_prefix);

    if p.blockers.is_empty() {
        println!("  ✅ 没有拦路虎，可以迁");
    } else {
        println!("  ⛔ 还不能迁：");
        for b in &p.blockers {
            println!("     · {b}");
        }
    }
    if !p.warnings.is_empty() {
        println!("  ⚠ 须知：");
        for w in &p.warnings {
            println!("     · {w}");
        }
    }
    println!();
}

// ---------------------------------------------------------------- migrate / rollback

/// 解析公共参数并解析出 (定义, 源目录)。
fn resolve_target(args: &[String], usage: &str) -> Result<(acm_core::AgentDef, PathBuf), String> {
    let id = positional(args, usage)?;
    let cfg = flag_path(args, "--config");
    let defs = agents::load(cfg.as_deref())?;
    let def = defs
        .iter()
        .find(|d| d.id == id)
        .ok_or_else(|| format!("注册表里没有 id = {id}"))?;
    let src = def
        .paths
        .iter()
        .find_map(|tpl| paths::expand(tpl).filter(|p| p.exists()))
        .ok_or_else(|| format!("{} 的数据目录都不存在", def.name))?;
    Ok((def.clone(), src))
}

fn confirm(prompt: &str, yes: bool) -> Result<bool, String> {
    if yes {
        return Ok(true);
    }
    print!("{prompt} (y/N) ");
    use std::io::Write;
    std::io::stdout().flush().map_err(|e| e.to_string())?;
    let mut ans = String::new();
    std::io::stdin()
        .read_line(&mut ans)
        .map_err(|e| format!("读不到确认输入：{e}"))?;
    Ok(ans.trim().eq_ignore_ascii_case("y"))
}

fn cmd_migrate(args: &[String]) -> Result<(), String> {
    let usage = "用法：acm migrate <agent-id> [--dest <目标根目录>] [--yes] [--json]";
    let yes = args.iter().any(|a| a == "--yes");
    let json = args.iter().any(|a| a == "--json");
    let id = positional(args, usage)?;
    let cfg = flag_path(args, "--config");
    let defs = agents::load(cfg.as_deref())?;
    let def = defs
        .iter()
        .find(|d| d.id == id)
        .ok_or_else(|| format!("注册表里没有 id = {id}"))?;

    // 🔴 Agent 可能声明多个数据目录（如 npm-cache 的两个缓存位置）——
    // 只迁第一个会留下一半在 C 盘。这里逐个迁：任一失败立即停止，其余不动。
    let targets: Vec<PathBuf> = def
        .paths
        .iter()
        .filter_map(|tpl| paths::expand(tpl))
        .filter(|p| p.exists() && !fsutil::is_link(p))
        .collect();
    if targets.is_empty() {
        return Err(format!(
            "{} 没有需要迁移的目录（都不存在或已是链接）",
            def.name
        ));
    }

    let dest_root = match flag_path(args, "--dest") {
        Some(d) => d,
        None => migrate::default_dest_root()
            .ok_or("没能自动挑出目标盘（可能只有一个盘），请用 --dest 指定")?,
    };

    let mut plans = Vec::new();
    for src in &targets {
        let p = migrate::plan(def, src, &dest_root)?;
        if !p.blockers.is_empty() {
            return Err(format!("还不能迁移 {}：{}", p.src, p.blockers.join("；")));
        }
        plans.push(p);
    }

    if !json {
        for p in &plans {
            println!("【{}】{}", p.agent_id, p.agent_name);
            println!("  源　　{}", p.src);
            println!(
                "  占用　{}（{} 文件）",
                fsutil::human(p.src_measure.bytes),
                p.src_measure.files
            );
            println!("  目标　{}", p.dest);
            if p.large_files > 0 {
                println!(
                    "  大文件　{} 个 ≥1GB{}",
                    p.large_files,
                    if p.large_files_capped {
                        "（下限）"
                    } else {
                        ""
                    }
                );
            }
            for w in &p.warnings {
                println!("  ⚠ {w}");
            }
            println!();
        }
    }
    if !confirm("确认执行迁移？", yes || json)? {
        println!("已取消。");
        return Ok(());
    }

    let mut reports: Vec<migrate::MigrateReport> = Vec::new();
    let mut any_failed = false;
    let with_env = args.iter().any(|a| a == "--with-env");
    for (i, p) in plans.iter().enumerate() {
        if plans.len() > 1 && !json {
            println!("\n── [{}/{}] {}", i + 1, plans.len(), p.src);
        }
        let rep = migrate::run(p, &mut |pr| {
            // 进度走 stderr：stdout 留给最终报告/JSON，方便管道
            if !json {
                let pct = pr.percent.map(|p| format!(" {p}%")).unwrap_or_default();
                eprintln!("[{:>14}] {}{}", pr.step, pr.detail, pct);
            }
        });
        let ok = rep.ok;
        if json {
            reports.push(rep.clone());
        } else {
            println!();
            for s in &rep.steps {
                println!(
                    "{} {} —— {}",
                    if s.ok { "✓" } else { "✗" },
                    s.name,
                    s.detail
                );
            }
            if rep.ok {
                println!("\n✅ 迁移完成：{} → {}", rep.src, rep.dest);
                println!(
                    "   （原目录保留在 {}；建议先启动应用确认正常，再清理快照）",
                    rep.snapshot.as_deref().unwrap_or("?")
                );
            } else if let Some(e) = &rep.error {
                eprintln!("\n✗ 迁移失败：{e}（自动回滚：{}）", rep.rolled_back);
            }
        }
        reports.push(rep);
        if !ok {
            any_failed = true;
            break; // 逐目录顺序迁：前一个失败，后面的原地不动
        }
    }

    // 🔴 --with-env：全部迁移成功后，顺带设置官方重定位环境变量。
    // 联接 + 变量双保险 —— 数据可从两条路访问；unset 变量也还有联接兜底。
    if with_env && !any_failed {
        if let Err(e) = migrate::env_relocate_apply(def, Some(&dest_root), false) {
            eprintln!("⚠ 环境变量设置失败（迁移本身已成功，不影响使用）：{e}");
        }
    }

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&reports).map_err(|e| e.to_string())?
        );
    }
    if any_failed {
        return Err("迁移未成功".to_string());
    }
    Ok(())
}

fn cmd_rollback(args: &[String]) -> Result<(), String> {
    let usage = "用法：acm rollback <agent-id> [--yes] [--config <路径>]";
    let yes = args.iter().any(|a| a == "--yes");
    let (def, src) = resolve_target(args, usage)?;

    if !confirm(
        &format!(
            "把 {} 的快照改回原位？（{} 的联接会被摘掉）",
            src.display(),
            def.name
        ),
        yes,
    )? {
        println!("已取消。");
        return Ok(());
    }
    let msg = migrate::rollback(&def, &src)?;
    println!("✅ {msg}");
    Ok(())
}

fn cmd_repair(args: &[String]) -> Result<(), String> {
    let usage = "用法：acm repair <agent-id> [--dest <数据所在的根>] [--yes] [--config <路径>]";
    let yes = args.iter().any(|a| a == "--yes");
    let (def, src) = resolve_target(args, usage)?;

    let dest_root = match flag_path(args, "--dest") {
        Some(d) => d,
        None => migrate::default_dest_root()
            .ok_or("没能自动挑出目标盘（可能只有一个盘），请用 --dest 指定数据所在的根")?,
    };

    if !confirm(
        &format!(
            "修复 {} 的链接？（{}，只重建链接不动数据）",
            src.display(),
            def.name
        ),
        yes,
    )? {
        println!("已取消。");
        return Ok(());
    }
    let rep = migrate::repair(&def, &src, &dest_root)?;
    println!("✅ [{}] {}", rep.action, rep.detail);
    if rep.action != "noop" {
        println!("   {} → {}", rep.src, rep.dest);
    }
    Ok(())
}

/// 设置/删除 Agent 官方支持的重定位环境变量（仅限配了 env_relocate 的）。
///
/// ⚠️ 这是**持久的用户级系统变更**：设置前展示原值；删除会恢复应用默认路径 ——
/// 但如果数据已经搬走而原路径没有联接，删变量后应用会「找不到数据」，
/// 所以 unset 前必须让用户明确确认。
fn cmd_env(args: &[String]) -> Result<(), String> {
    let usage = "用法：acm env <agent-id> [--dest <根>] [--unset] [--yes] [--config <路径>]";
    let yes = args.iter().any(|a| a == "--yes");
    let unset = args.iter().any(|a| a == "--unset");
    let id = positional(args, usage)?;
    let cfg = flag_path(args, "--config");
    let defs = agents::load(cfg.as_deref())?;
    let def = defs
        .iter()
        .find(|d| d.id == id)
        .ok_or_else(|| format!("注册表里没有 id = {id}"))?;

    let env = def.env_relocate.as_ref().ok_or_else(|| {
        format!(
            "{} 没有声明官方支持的环境变量重定位（env_relocate）",
            def.name
        )
    })?;

    if unset {
        let old = acm_core::envvar::get(&env.env);
        if old.is_none() {
            println!("{} 当前未设置，无需删除。", env.env);
            return Ok(());
        }
        if !confirm(
            &format!(
                "删除 {}（当前值 {}）？\n⚠ 如果数据已经搬到别处而原路径没有联接，\
                 删除后应用会找不到数据 —— 请确认原路径可用后再删",
                env.env,
                old.as_deref().unwrap_or("?")
            ),
            yes,
        )? {
            println!("已取消。");
            return Ok(());
        }
        acm_core::envvar::remove(&env.env)?;
        println!("✅ 已删除 {}（重启应用后恢复默认路径）", env.env);
        return Ok(());
    }

    let dest_root = match flag_path(args, "--dest") {
        Some(d) => d,
        None => migrate::default_dest_root().ok_or("没能自动挑出目标盘，请用 --dest 指定")?,
    };
    let value = dest_root.join(&def.id);
    let old = acm_core::envvar::get(&env.env);

    if !confirm(
        &format!(
            "设置 {} = {}？\n当前值：{}\n⚠ 这是持久的用户级变更；重启应用后生效。\
             同一程序有多个版本时，一个变量会把它们指到同一目录 —— 配置里的警告要看",
            env.env,
            value.display(),
            old.as_deref().unwrap_or("（未设置）")
        ),
        yes,
    )? {
        println!("已取消。");
        return Ok(());
    }
    acm_core::envvar::set(&env.env, &value.to_string_lossy())?;
    println!(
        "✅ 已设置 {} = {}（重启应用后生效）",
        env.env,
        value.display()
    );
    if let Some(o) = old {
        println!(
            "   （原值 {} —— 想恢复默认路径可用 acm env {} --unset）",
            o, def.id
        );
    }
    Ok(())
}

// ---------------------------------------------------------------- snapshots

fn cmd_snapshots(args: &[String]) -> Result<(), String> {
    let cfg = flag_path(args, "--config");
    let defs = agents::load(cfg.as_deref())?;
    let snaps = migrate::list_snapshots(&defs);

    if args.iter().any(|a| a == "--json") {
        println!(
            "{}",
            serde_json::to_string_pretty(&snaps).map_err(|e| e.to_string())?
        );
        return Ok(());
    }

    if snaps.is_empty() {
        println!("没有快照。");
        println!(
            "（快照是迁移时原目录改名保留的那份 `<名字>.moved-<时间戳>`，删掉它就不能回滚了。）"
        );
        return Ok(());
    }

    println!("迁移快照（原目录改名保留，删掉就不能回滚）：");
    let mut total = 0u64;
    for s in &snaps {
        total += s.measure.bytes;
        println!(
            "  {:<12} {:<10} {:>10} {:>8} 文件  {}",
            s.agent_id,
            s.created,
            fsutil::human(s.measure.bytes),
            s.measure.files,
            s.path
        );
    }
    println!("\n共 {} 个，合计 {}", snaps.len(), fsutil::human(total));
    Ok(())
}

// ---------------------------------------------------------------- drives

// ---------------------------------------------------------------- history

/// 迁移历史：`%APPDATA%/AgentCache/history.jsonl`（core 在每次 run 后追加）。
fn cmd_history(args: &[String]) -> Result<(), String> {
    let all = args.iter().any(|a| a == "--all");
    let json = args.iter().any(|a| a == "--json");
    let Some(p) = migrate::history_path() else {
        return Err("定位不了历史文件（缺少用户配置目录）".to_string());
    };
    if !p.exists() {
        println!("还没有迁移历史（{}）", p.display());
        return Ok(());
    }
    let text = std::fs::read_to_string(&p).map_err(|e| format!("读 {} 失败：{e}", p.display()))?;
    let mut items: Vec<serde_json::Value> = text
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();
    items.reverse(); // 新的在前
    if !all {
        items.truncate(20);
    }

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&items).map_err(|e| e.to_string())?
        );
        return Ok(());
    }

    println!("迁移历史（最近 {} 条 / 文件 {}）", items.len(), p.display());
    for it in &items {
        let ts = it["ts"].as_str().unwrap_or("?");
        let agent = it["agent"].as_str().unwrap_or("?");
        let src = it["src"].as_str().unwrap_or("?");
        let dest = it["dest"].as_str().unwrap_or("?");
        let ok = it["ok"].as_bool().unwrap_or(false);
        println!(
            "{} {ts}  {agent:<14} {} → {}",
            if ok { "✓" } else { "✗" },
            src,
            dest
        );
        if let Some(e) = it["error"].as_str() {
            println!("      └ {e}");
        }
    }
    Ok(())
}

// ---------------------------------------------------------------- wizard

/// 交互式向导：`acm` 不带参数时进入 —— 不用读文档就能完成迁移/回滚/修复。
///
/// 菜单每轮重新检测（执行完动作状态会变）；所有危险动作走子命令的
/// 确认流程，这里只做「选目标」。
fn cmd_wizard(_args: &[String]) -> Result<(), String> {
    println!("AgentCache 交互模式（输入 q 随时退出）");
    loop {
        println!("\n─────────────────────────────");
        let defs = agents::load(None)?;
        let detected = agents::detect_all(&defs, &agents::DetectOpts::default());

        // 只列「有得操作」的：有真实目录（可迁移）或有联接（可回滚/修复）
        let actionable: Vec<&acm_core::DetectedAgent> = detected
            .iter()
            .filter(|a| a.paths.iter().any(|p| p.kind == "dir" || p.is_link))
            .collect();

        if actionable.is_empty() {
            println!("没有检测到可操作的 Agent（应用可能都没装）。");
            println!("装好应用后再运行 `acm` 即可。");
            return Ok(());
        }

        for (i, a) in actionable.iter().enumerate() {
            let link = a.paths.iter().any(|p| p.is_link);
            let broken = !a.broken_links.is_empty();
            let size = if a.total.files > 0 {
                format!("，{}", fsutil::human(a.total.bytes))
            } else {
                String::new()
            };
            let status = if broken {
                "⚠ 有断链"
            } else if link {
                "已迁移"
            } else {
                "可迁移"
            };
            println!(
                "  {:>2}) {:<16} {:<18} {}{}",
                i + 1,
                a.id,
                a.name,
                status,
                size
            );
        }

        println!("\n命令：编号 = 迁移｜r 编号 = 回滚｜f 编号 = 修复｜d = 体检｜q = 退出");
        print!("> ");
        use std::io::Write as _;
        std::io::stdout()
            .flush()
            .map_err(|e| format!("写不进控制台：{e}"))?;

        let mut line = String::new();
        std::io::stdin()
            .read_line(&mut line)
            .map_err(|e| format!("读不到输入：{e}"))?;
        let t = line.trim().to_string();

        match t.as_str() {
            "q" | "quit" | "exit" | "" => return Ok(()),
            "d" | "doctor" => {
                let _ = cmd_doctor(&[]);
                continue;
            }
            _ => {}
        }

        let (verb, idx_str) = if let Some(rest) = t.strip_prefix('r') {
            ("r", rest.trim())
        } else if let Some(rest) = t.strip_prefix('f') {
            ("f", rest.trim())
        } else {
            ("m", t.as_str())
        };

        let Ok(n) = idx_str.parse::<usize>() else {
            println!("看不懂输入：{t:?}（编号、r 编号、f 编号、d 或 q）");
            continue;
        };
        if n == 0 || n > actionable.len() {
            println!("编号超出范围（1-{}）", actionable.len());
            continue;
        }
        let id = actionable[n - 1].id.clone();

        let r = match verb {
            "r" => cmd_rollback(std::slice::from_ref(&id)),
            "f" => cmd_repair(std::slice::from_ref(&id)),
            _ => cmd_migrate(std::slice::from_ref(&id)),
        };
        if let Err(e) = r {
            eprintln!("✗ {e}");
        }
    }
}

// ---------------------------------------------------------------- doctor

/// 体检：只读检查，把状态里的毛病一次报出来，每条都带可复制的修复命令。
fn cmd_doctor(args: &[String]) -> Result<(), String> {
    let json = args.iter().any(|a| a == "--json");
    let cfg = flag_path(args, "--config");
    let defs = agents::load(cfg.as_deref())?;
    let findings = acm_core::doctor::run_checks(&defs);

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&findings).map_err(|e| e.to_string())?
        );
        return Ok(());
    }

    println!("🔍 健康检查\n");
    for f in &findings {
        let mark = match f.level.as_str() {
            "ok" => "✓",
            "info" => "ℹ",
            "warn" => "⚠",
            _ => "✗",
        };
        println!("[{}] {} {}", f.area, mark, f.message);
        if let Some(h) = &f.hint {
            println!("        ↳ 建议：{h}");
        }
    }
    let (errors, warnings) = acm_core::doctor::summarize(&findings);
    println!("\n结论：{} 个错误，{} 个警告", errors, warnings);
    if errors > 0 {
        return Err(format!("{errors} 个需要处理的错误"));
    }
    Ok(())
}

fn cmd_drives() -> Result<(), String> {
    println!("盘符：");
    let sys = migrate::system_drive();
    for d in migrate::drives() {
        let free = migrate::free_space(std::path::Path::new(&d));
        let tag = if Some(d.trim_end_matches('\\').to_string()) == sys {
            "  （系统盘）"
        } else {
            ""
        };
        match free {
            Some(b) => println!("  {d:<5} 可用 {}{tag}", fsutil::human(b)),
            None => println!("  {d:<5} （读不到）{tag}"),
        }
    }
    match migrate::default_dest_root() {
        Some(r) => println!("\n自动挑的目标根：{}", r.display()),
        None => println!("\n没有可用的非系统盘，需要 --dest 手工指定"),
    }
    Ok(())
}

// ---------------------------------------------------------------- 极简参数解析
//
// 刻意不引 clap：这里总共几个带值的选项，手写反而更容易读懂行为。

fn flag_str(args: &[String], name: &str) -> Option<String> {
    let i = args.iter().position(|a| a == name)?;
    args.get(i + 1).cloned()
}

fn flag_path(args: &[String], name: &str) -> Option<PathBuf> {
    flag_str(args, name).map(PathBuf::from)
}
