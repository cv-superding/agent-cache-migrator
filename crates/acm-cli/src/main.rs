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
  acm detect [选项]          检测本机装了哪些 Agent
  acm list                   列出注册表里的 Agent（不检测）
  acm plan <id> [--dest <根>]  为某个 Agent 生成迁移计划（**不动任何文件**）
  acm snapshots              列出所有迁移快照（原目录改名保留的那份）
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
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cmd = args.first().map(String::as_str).unwrap_or("help");

    let r = match cmd {
        "detect" => cmd_detect(&args[1..]),
        "list" => cmd_list(&args[1..]),
        "plan" => cmd_plan(&args[1..]),
        "snapshots" => cmd_snapshots(&args[1..]),
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
            let t = if a.total.capped {
                format!("≥{s}")
            } else {
                s
            };
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

// ---------------------------------------------------------------- plan

fn cmd_plan(args: &[String]) -> Result<(), String> {
    let id = args
        .iter()
        .find(|a| !a.starts_with("--"))
        .ok_or("用法：acm plan <agent-id> [--dest <目标根目录>]")?
        .clone();
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
        let Some(src) = paths::expand(tpl) else { continue };
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
        if p.src_measure.capped { "（下限，已截断）" } else { "" }
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
        println!("（快照是迁移时原目录改名保留的那份 `<名字>.moved-<时间戳>`，删掉它就不能回滚了。）");
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
