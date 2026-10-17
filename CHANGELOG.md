# Changelog

格式参照 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/)，版本号遵循 [SemVer](https://semver.org/lang/zh-CN/)。

## [0.8.0] - 2026-10-17

### 新增

- **`acm doctor` 健康检查**：只读体检，一次报出四类问题 —— 数据目录健康状态（断链/缺失/正常）、运行中进程、快照脏状态（重复快照 / 原位与快照并存 / 孤儿快照）、环境变量重定位当前值；每条发现附**可直接执行的修复命令**（如 `acm repair codex`）。`--json` 给脚本用；有错误时退出码非 0。核心检查带跨平台测试（快照反推原位、孤儿快照检测）。
- **交互式向导**：`acm` 不带参数时进入菜单模式 —— 列出可操作的 Agent（含体积与状态），编号迁移 / `r` 回滚 / `f` 修复 / `d` 体检 / `q` 退出；危险动作仍走子命令的确认流程。新手不用读文档。

### 修正

- **`paths::expand` 绝对路径 bug**（真 bug）：`agents.toml` 里写绝对 Windows 路径（如 `C:\Users\...`）会被逐段 push 拼成**盘符相对路径** `C:Users\...`（分隔符丢失）—— classify 判 missing、迁移根本找不到目录；unix 的 `/Users/...` 同样被拼成相对路径。现在绝对路径（Windows 盘符 / UNC / unix 根）直接归一化返回，附回归测试。

## [0.7.1] - 2026-10-17

### 修正

- **环境变量变更也记入迁移历史**：`acm env` / 界面「用环境变量重定位」的 set/unset 现在会追加 env 事件到 `history.jsonl`（带 `type` 字段区分 migrate/env）—— 此前改了用户级系统状态却无账可查；历史卡片按类型渲染。

## [0.7.0] - 2026-10-17

### 新增

- **检测并行化**：量体积改用 `std::thread::scope` 4 路并发（零新依赖）—— 18 个 Agent 串行量体积最坏要几分钟，现在约 1/4；结果按索引写回，界面顺序不变。
- **macOS 更新器指纹**：Electron 更新器缓存补上 macOS 路径（`~/Library/Caches/<appId>-updater`），mac 上「装了没」的检测不再只依赖数据目录存在性。
- **界面环境变量重定位入口**：配了 `env_relocate` 的 Agent（Codex / Claude Code）在状态卡片出现「用环境变量重定位」按钮 —— 设置前确认、说明持久变更与多版本互踩警告（`acm env` 的 GUI 版）。

## [0.6.0] - 2026-10-17

### 新增

- **env_relocate 自动设置**（此前 README 明确「暂未实现」的最后一项）：`acm env <id> [--dest <根>]` 一键设置 Agent 官方支持的重定位环境变量（Windows：写 `HKCU\Environment` + 广播 `WM_SETTINGCHANGE`，重启应用生效）；`--unset` 恢复默认路径。仅限配置里声明了 `env_relocate` 的 Agent（Codex `CODEX_HOME`、Claude Code `CLAUDE_CONFIG_DIR`）；设置前展示原值，删除前明确警告「数据已搬走时先确认原路径可用」。unix 不自动改 shell profile（各 shell 语法不同），明确报错。
- **内置注册表新增 2 个国际版**：Trae 国际版（`%APPDATA%\Trae`）、CodeBuddy 国际版（`%APPDATA%\CodeBuddy`）—— 共 18 个。

### 变更

- **CI clippy 从「观察期」转为硬约束**（`-D warnings`）：存量警告已清零。

## [0.5.0] - 2026-10-17

### 新增

- **目标卷文件系统检查**：计划阶段读取目标卷的文件系统（Windows `GetVolumeInformationW`），**非 NTFS 一律拦下**（blocker）—— exFAT/FAT32 无法保留属性与 ACL（robocopy /COPY:DAT 会失败），FAT32 还有 4GB 单文件上限。同类工具（OPTOPIA、DriveKeeper）也都强制 NTFS。unix 无对应概念，跳过检查。
- **界面新增「迁移历史」卡片**：`history.jsonl` 落盘的数据在界面上可见了 —— 每次迁移（含失败的）何时、对哪个目录、结果如何；支持展开全部。

## [0.4.0] - 2026-10-17

### 新增

- **迁移历史落盘**：每次迁移（无论成败）追加一条到 `%APPDATA%/AgentCache/history.jsonl` —— 事后能回答「什么时候对哪个目录做过什么」（借鉴 OPTOPIA 的迁移状态清单）；CLI 新增 `acm history [--all] [--json]`。
- **计划报告大文件统计**：plan 会列出 ≥1GB 的大文件数（借鉴 zhao-wuyan/directory-migration-tool 的 LargeFileThreshold）—— 帮助预估复制耗时（几个大文件 ≠ 几十万小文件）；CLI 与界面都展示。
- **CLI migrate 支持多目录 Agent**：声明了多个数据目录的 Agent（如 npm-cache 的两个缓存位置）此前只迁第一个 —— 现在逐个迁移、任一失败立即停止。
- **内置注册表新增 3 个 Agent**：Windsurf（`%APPDATA%\Windsurf`）、通义灵码（`~/.lingma`）、Gemini CLI（`~/.gemini`）。
- 迁移完成的提示明确「重启应用后新写入会直接落到目标盘；确认正常后再清理快照」（借鉴 DriveKeeper 的 live-write 验证思想）。

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
