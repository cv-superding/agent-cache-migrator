# Changelog

格式参照 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/)，版本号遵循 [SemVer](https://semver.org/lang/zh-CN/)。

## [0.3.0] - 2026-10-17

### 新增

- **修复模式（Repair）**：检测到断链联接（`broken_links`）时，界面的状态卡片会出现「修复链接（不动数据）」按钮；CLI 对应 `acm repair <id>`。只重建链接、不复制数据 —— 针对「目标数据还在、链接坏了/被误删」的场景（借鉴 zhao-wuyan/directory-migration-tool 的 Repair 模式）。防御：候选目标必须是有内容的真实目录；链接完好但指向别处时拒绝擅改；应用运行中拒绝修复。
- **回滚安全增强**：摘链接前先把「迁移后新增/变更的」文件（相对快照：不存在或大小不同）复制到目标盘旁的 `<名字>.rescued-<时间戳>` 恢复目录 —— **回滚零丢失**（借鉴 OPTOPIA/appdata-migrate-tool 的安全回滚）。备份失败则中止回滚、保持原状。

### 修正

- **CLI 位置参数**：`acm migrate --dest D:\x codex` 会把 `D:\x` 当成 agent id —— 现在统一跳过带值选项（`--dest`/`--config`/`--only`）的值。
- **进度事件洪泛**：大目录复制时逐文件 emit 会打爆 Tauri 事件队列 —— 非最终进度 200ms 节流。
- **界面**：「移入回收站」按钮点击后关闭确认框。

## [0.2.0] - 2026-10-16

### 新增

- **复制阶段真实进度**：Windows 端逐行读取 robocopy 输出、Unix 端复制回调计数，界面的复制进度条不再从 5% 直接跳到 70%（`src_measure` 被上限截断时依旧不显示百分比，只报已复制数）。
- **快照「移入回收站」**：Windows 经 `SHFileOperation` + `FOF_ALLOWUNDO` 实现（可从系统回收站还原）；永久删除仍是显式选择。界面删除确认框新增「移入回收站」按钮。unix 上明确报错（各桌面回收站约定不统一）。
- **CLI 新增 `migrate` / `rollback` 子命令**：headless 排障时可以脱离 GUI 复现迁移与回滚；`--yes` 跳过确认（脚本/自动化）、`--json` 输出机器可读报告（进度走 stderr）。
- **占用检测可用性上报**：Restart Manager 会话创建失败时，`plan()` 会给出明确警告（「检测不可用 ≠ 没有占用」），unix 上也会如实提示「本平台无自动占用检测」。同项目调研：[`OPTOPIA/appdata-migrate-tool`](https://github.com/OPTOPIA/appdata-migrate-tool)、[`zhao-wuyan/directory-migration-tool`](https://github.com/zhao-wuyan/directory-migration-tool)（回滚前备份、Repair 模式列入后续方向）。

### 变更

- 版本号升至 0.2.0；`agents.toml` 未变 —— 老配置完全兼容。

## [Unreleased]（0.1.1 → 0.1.x 修复，随 0.2.0 一并发布）

### 新增

- **`skip` 字段真正生效**：`agents.toml` 里配置的校验排除 glob（如 `logs/**`、`**/*.log`）现在会同时作用于 Windows（robocopy 校验清单过滤）与 Unix（`pending_count` 排除）。复制仍全量进行，仅这些条目不参与「0 差异」收敛判定。计划（Plan）中会携带 `skip` 字段。
- **Windows 端到端回归测试**：复制（/XJ 跳过联接）→ 校验（含 skip）→ 快照 → 建联接 → 穿过联接验证 → 回滚的完整 roundtrip；以及「`remove_dir_forced` 绝不穿过联接删真实数据」的安全用例。
- **CI 增加 Windows job**：Windows 是主战场，迁移主路径此前只靠手工验证，现在每次推送都会在 windows runner 上真跑端到端测试。
- **CI 增加 clippy 观察期 job**（报告不阻塞，清零后转硬约束）。
- **`schema_version` 校验**：`agents.toml` 的 `schema_version != 1` 会被明确拒绝，为将来 schema 演进兜底。
- 新增 `globs` 模块（极简 glob 匹配，`*` 段内通配 / `**` 跨段）。

### 安全

- **`delete_snapshot` 加固**：删除前强制校验路径 —— 文件名必须符合 `<名字>.moved-YYYYMMDD-HHMMSS` 约定、且必须位于某个已知 Agent 数据目录的父目录下并前缀一致。该命令底层是「解 ACL → 清属性 → 递归删」，此前接受任意路径等于给了前端一个「删任意目录」原语。
- **Tauri CSP**：从 `null` 收紧为 `default-src 'self'; connect-src ipc: http://ipc.localhost; style-src 'self' 'unsafe-inline'; img-src 'self' data:`。

### 修正

- **文档如实化**：README「两种迁移策略」改为如实描述 —— 「改环境变量」策略目前**仅提示、未实现自动设置**；`agents.toml` 字段注释与 `plan()` 警告文案同步修正。
- **存量测试缺陷**：`detects_*_if_present` 允许「仅更新器指纹命中」（装过但从未启动）的情况。

## [0.1.1] - 2026-10 之前

初始公开版本（详见仓库提交历史）。
