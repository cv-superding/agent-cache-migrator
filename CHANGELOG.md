# Changelog

格式参照 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/)，版本号遵循 [SemVer](https://semver.org/lang/zh-CN/)。

## [未发布]

### 修复（迁移内核 · 第二轮）

- **预览里的「最终落点」跟实际复制不是一回事**：`plan` 还在用 `dest_root/<id>` 算落点，而真正复制/修复用的是能区分多目录的命名 —— 于是预览、目标体检、同盘判断全看错了地方，第二个数据目录照样会撞主目录的落点。现在三处统一成同一个命名规则（并加了断言：两个目录算出的落点必须不同）。

- **路径里带 `&` / `^` 就建不出联接**：`make_junction` 原来走 `cmd /C mklink /J`，命令行会被 cmd 再解释一遍 —— 用户名或目录名里有 cmd 元字符就建错位置/建不出来，而且每次都要起一个 cmd 进程。改成直接 `CreateFileW` + `FSCTL_SET_REPARSE_POINT` 写重解析点（和 `mklink /J` 底层同一件事，同样**不需要管理员权限**）。踩了两个坑：mount point 的重解析缓冲区**没有** `Flags` 字段（那是符号链接才有的），且两个名字各自**带 NUL 结尾**、打印名偏移 = 替代名长度 + 2 —— 少一个就是 `ERROR_INVALID_REPARSE_DATA (4392)`（对照 `mklink` 建出的真实字节才确认）。
- **中文路径下校验 `skip` glob 完全失效**：`pending_count` 原来解析 robocopy `/L` 的清单文本，而 robocopy 输出是**控制台代码页（中文机器是 GBK）**，按 UTF-8 读出来中文路径全是乱码 → glob 匹配不上、还会把目标盘「多出来的文件」算成待复制的。改成自己走一遍源树逐文件比对（大小 + 修改时间 3 秒容差，与 `/E` 的收敛条件一致），顺手和复制阶段的 `/XD` 对齐跳过 `$RECYCLE.BIN` / `System Volume Information`。
- **复制进度虚高**：robocopy 的报错行（`… ERROR 5 (0x00000005) Copying File …`）也被算成「复制了一个文件」，百分比偏高还会把真正的错误行挤出「最近 3 行」缓冲。现在报错行只进缓冲、不计数。
- **多目录 Agent 的落点会撞车**：豆包有两个数据目录（`%LOCALAPPDATA%\Doubao` 真数据 + `%APPDATA%\Doubao` 两个配置），两个都按 `dest_root/<id>` 算落点 —— 第二个目录会被当成「第一个目标的增量」复制进去，两套数据混成一坨、联接还指到同一处。现在主目录仍用 `<id>`，其余目录用 `<id>-<父目录>-<目录名>`（豆包 → `doubao-Roaming-Doubao`）。
- **迁移后运行时联接凭空消失**：复制阶段带 `/XJ`（不跟进重解析点，这是对的），但源树里的联接因此也没被复制。WorkBuddy 的 `binaries/python/versions/*` 这类运行时目录正是指向共享运行时的联接 —— 搬完应用就找不到 Python。新增「联接」步骤：迁移完按原样把源树里的联接重建到目标（指向原目标），失败只记录、不推翻已成功的复制。
- **空目录迁移被误判失败**：`verify_through_link` 把「联接两边都是 0 个条目」当错误 → 刚装好还没跑过的应用迁一次就回滚。空目录本来就是合法的，改判成功。
- **目标本身是联接时没拦住**：`/XJ` 只挡源树里的重解析点，目标目录若是联接，robocopy 会照常写进去 —— 数据根本没落到这块盘。现在列为阻断项，提示换落点或先摘联接。
- **GUI 里第二个数据目录迁不了**：主目录迁完（成了联接）后，「迁移」按钮每次都撞「已经是联接了」，用户看不到另外那个目录。`resolve` 改成优先取**还没迁移的真实目录**，全迁完了才报「已迁移」并说明数据在哪。
- **同盘迁移没提醒**：目标和源在同一个盘时搬完并不省空间（还多占一份），现在给警告。
- **永久删快照可能动到别人的 ACL**：快照本身若是联接，`icacls /reset /T` 会顺着联接把目标盘的整棵子树 ACL 全改掉。现在先分类，联接管只摘链接、绝不递归。

### 修复

- **体积数字「看着不准」**：列表里的体积是**有上限的近似值**（扫到 25 万文件 / 20 秒就停，WorkBuddy 这种 19 GB / 上百万文件的目录只会显示下限），但侧栏没标 `≥`，看起来就像算错了。现在侧栏也标 `≥`，并且「占用」后面多了 **「精确统计」** 按钮 —— 不设上限慢慢扫，给出准数（大目录可能要几十秒到几分钟）。
- **侧栏体积被滚动条切掉**：`GB` 被裁成 `GE` / 直接看不见。给导航列留出滚动条位置（`scrollbar-gutter: stable`）并给体积列限宽，名字过长改为省略号。
- **多数据目录说不清**：豆包这类应用的数据分两处（`%LOCALAPPDATA%\Doubao` 是 GB 级真数据、`%APPDATA%\Doubao` 只有 2 个配置文件），界面上只写「检测到 2 个数据目录」，用户不知道搬哪个。现在每个目录后面显示各自的体积/文件数，并标 **「迁移它」** / **「留在原地（只有配置）」**（迁移只会搬第一个存在的目录）。
- **计划串台**：切换 Agent 后，右下的「最终落点」还会短暂显示上一个 Agent 的路径（如选中豆包却显示 `E:\AgentCache\trae`）。换 Agent 时立即作废旧计划。
- **清理缓存连「只有配置」的小目录一起清**：之前只清主数据目录的话，`%APPDATA%\Doubao` 那 2 个配置文件还留着，重扫仍然认为它装着。现在清单里两个都列（标签区分「主数据目录」/「附带目录（多为配置）」），一次清干净才真正落到「未安装」。
- **迁移历史被单元测试污染**：`cargo test` 会真的跑迁移/回滚，测试用例里的假记录（`AC_TEST_ROLLBACK_VAR`、`%TEMP%\acm-rbenv-xxxx`）被写进用户真实的历史文件，界面上看起来像「用户自己干过这些事」。现在 `history_path()` 在 `cfg!(test)` 下直接返回 `None`，测试不再落盘。
- **历史日志可以清空**：界面「迁移历史」卡片新增「清空历史」（确认后删掉整份 `history.jsonl`，迁移结果 / 联接 / 快照都不动），CLI 对应 `acm history --clear`。

### 新增

- **「已卸载 · 只剩缓存」状态**：`agents.toml` 新增 `install = { uninstall = [...], exe = [...] }` —— 用 Windows 卸载清单（HKCU / HKLM / WOW6432Node 三处，**注册表原生 API 读 UTF-16**）判断应用本身还在不在。数据目录还在、卸载清单里已经没有它 → 界面标「已卸载」、侧栏单独成组「已卸载 · 只剩缓存（n）」，详情页只剩「清理缓存」一个主操作（迁移 / 目标盘两张卡片直接收起），清完就落到「未安装」。已给 uTools / Trae 国际版 / Trae CN / DeepSeek Harness / 豆包 / ZCode / Ollama 配了线索；没配的 Agent 一律 `unknown`，绝不瞎猜。
- **侧栏区分「Agent」与「缓存目录」**：`agents.toml` 新增 `category`（`agent` 默认 | `cache`），npm 缓存这类工具链目录标成 `cache` —— 侧栏拆成 `Agent（n）` / `缓存目录（n）` 两组，顶栏改成「已安装 n 项 · 共 N 项」，不再把所有东西都喊成「Agent」；详情页给缓存类条目挂一个「缓存」徽章。
- **清理缓存（卸载残留）**：状态卡片底部新增「清理缓存」——把该 Agent 的**数据目录 / 更新器目录整体移入系统回收站**，清完重扫就落回「未安装」。典型场景：应用早就卸了，`%APPDATA%\Xxx` 还留着几百 MB。点之前先弹确认框，逐条列出将被清理的路径与体积（只读预览，`purge_agent` 的 `preview` 模式）。内核 `acm_core::purge` 守住三条：只动 `agents.toml` 声明过的路径；**联接一律跳过**（背后是真数据，先回滚再清）；有进程在跑先拒绝。回收站可还原，空间清空回收站才真正释放。CLI 同步 `acm purge <id> [--yes] [--json]`。
- **CLI `acm purge`**（同上，`--json` 出报告给脚本用）。

### 修正

- **DeepSeek Harness 的图标不对**：之前用的是深度求索的品牌鲸鱼标；换成从 `DeepSeek Harness.exe` 里取出的 256px 应用图标（深色鲸鱼 + 浅色圆角方块），打包进 `src/assets/agents/deepseek.png`。

### 修正

- **豆包体积显示成 1.89 KB**：`agents.toml` 里 `doubao` 只写了 `%APPDATA%/Doubao`，而那里只有一个 2 KB 的 `public_config.json`；真正的 Chromium 数据（`Application` + `User Data`，GB 级）在 `%LOCALAPPDATA%/Doubao`。补上该路径后本机实测从 1.89 KB → 3.75 GB / 37,636 文件。
- **`DeepSeek` 其实叫 DeepSeek Harness**：注册表条目 `name` 改成 `DeepSeek Harness`（DSH），`processes` 从 `DeepSeek*.exe` 校准为 `DeepSeek Harness.exe / DeepSeekHarness.exe`，并补上「数据目录带 `@` 前缀、装完没启动过时只有更新器目录」的提示。

### 变更（界面）

- **每个 Agent 换成真实图标**：侧栏、标题、快照表、迁移历史里的图标不再是「名字首字母色块」—— 桌面应用用其官方应用图标（WorkBuddy / CodeBuddy / ZCode / 豆包 / 飞书 / Cherry Studio / Chatbox / uTools / 通义灵码 / OpenAI），编程类 Agent 用官方矢量标志（Claude / Cursor / Trae / DeepSeek / Gemini / LM Studio / Ollama / Windsurf / npm）。图标随安装包打包，离线可用；`agents.toml` 里自己加的 Agent 仍走首字母徽章兜底。
- **整体视觉重做**：中性灰底 + 白色卡片 + 单一绿色强调色；卡片带标题强调条与更克制的阴影，盘符卡片把「可用空间」提到主位并被选中态高亮，进度条 / 步骤条 / 弹窗 / Toast / 提示块统一重做，滚动条收细。所有配色集中在 `src/styles.css` 的 `:root`。
- 顶部新增 `图标 + 状态 + 占用 / 文件数 / 目录数` 概览条；历史与快照列表补上 Agent 真名和图标。
- README 的两张界面截图换成新界面。

### 修正

- **卡片在窄窗口下被压扁**（`overflow:hidden` 的卡片放进 flex 列后 `min-height:auto` 失效，内容会互相叠住）—— 给 `.body` 的直接子项显式 `flex: 0 0 auto`。
- **盘符卡片在窄窗口下换行掉到第二排**：原来是 `grid auto-fill minmax(186px,1fr)`，宽度不够就把第 4 个盘挤到下一行。改成 `flex` 等分（`flex: 1 1 0; min-width: 0`），盘有几个、窗口多宽都固定一行；卡片内的「可用 / 共 N」在牌面过窄时整块换行，数字不截断。

### 构建

- **MinGW 工具链链接修复**：Rust cdylib 符号过多会让 mingw 的 `ld.exe` 报 `export ordinal too large`，加 `.cargo/config.toml` 对 `x86_64-pc-windows-gnu` 关掉自动导出（`-Wl,--exclude-all-symbols`），MSVC 工具链不受影响。

## [0.10.1] - 2026-10-18

### 修正

- **🔴 回滚时自动撤销指向迁移目标的 `--with-env` 环境变量**：回滚后数据回到了原位，但变量仍指向目标盘的旧副本 —— 应用优先读变量会拿到**过期数据**。现在回滚时若变量恰好指向本次迁移目标 → 自动撤销并入账历史；指向别处（用户自定义值）→ 不动但消息里明确警告；撤销失败 → 不中止回滚（数据已安全），消息里给出手工命令。附完整回归测试（搭真实迁移现场 → 回滚 → 断言变量消失）。
- **GUI「取消」的 toast 不再误报失败**：取消的报告错误文案含「已取消」，界面据此显示中性提示而不是红色「迁移失败」。

## [0.10.0] - 2026-10-18

### 新增

- **迁移取消**（对齐 AppDataMover 的 CancellationToken 设计）：GUI 执行卡片在迁移中出现「取消」按钮（`cancel_migrate` 命令）；核心用全局取消标记，复制循环在每行输出之间响应，命中即杀掉 robocopy 子进程。**取消 ≠ 失败**：已复制的部分保留，重新执行自动续传；报告步骤里明确说明。
- **可选 SHA-256 哈希校验**（对齐 OPTOPIA 的 `-VerifyHash`）：CLI `acm migrate --verify-hash` 或 `RunOptions::verify_hash` —— 校验阶段在「0 差异」之外逐文件计算 SHA-256 与目标比对（字节级一致，比 robocopy `/L` 的大小+时间戳更严）。**手写纯 Rust SHA-256**（零依赖哲学的延续，~150 行），FIPS 180-4 标准测试向量 + 流式分块一致性 + 文件哈希三重测试锁死正确性；64KB 块流式读取，不整读进内存。大目录慢，所以是选项不是默认。

### 变更

- CLI `migrate` 新增 `--verify-hash`；核心 `run` 拆出 `run_with_options`（`run` 保持兼容）。

### 测试

- 新增 4 个：SHA-256 FIPS 向量 / 流式一致性 / 文件哈希、取消中止 + 哈希校验联动（串行验证，避免全局标记并行竞争）。

## [0.9.4] - 2026-10-18

### 新增（第四轮源码对照：AppDataMover `Mover.cs` 全文 + OPTOPIA `AppDataMigrate.ps1` 流水线）

- **`/MT:16` 多线程复制**（对齐 zennos0609-dotcom/AppDataMover 的 `RunRobocopy`）—— 大目录复制显著加速；每文件仍输出一行，行计数进度的口径不变。
- **robocopy 日志落盘**（对齐 OPTOPIA 的 `/LOG` + manifest.LogPath）：完整日志写到 `%APPDATA%\AgentCache\logs\<名>-<时间戳>.robocopy.log`（`/TEE` 保证 stdout 进度不受影响）—— 事后可审计「当时到底复制了什么」；已由迁移回环测试实测落盘。
- **对照确认我们更强的地方**（不改）：`/DCOPY:DAT`（目录属性完整保留，OPTOPIA 只保时间戳）、`/XJ`（AppDataMover 复制时没加，源里有联接会被跟进去）；校验用 robocopy `/L` 独立重数 0 差异（比双侧 CountTree 对比更严）。
- **评估不采纳**：OPTOPIA 的 `-VerifyHash`（SHA-256 全量校验）—— 大目录太慢，列为 backlog；复制失败的半成品清理（AppDataMover 会删）—— 我们的半成品是「增量续传」的基础，删除反而违背设计。

## [0.9.3] - 2026-10-17

### 新增（继续深读 zhao-wuyan ★81 的核心服务源码）

- **源路径体检**（对齐其 `PathValidator` 的黑名单/云盘警告）：
  - **系统关键目录 blocker**：源路径含 `C:\Windows`、`Program Files`、`ProgramData\Microsoft`、`System Volume Information`（unix：`/usr`、`/etc`、`/bin`、`/sbin`、`/System`、`/private/var`）→ 直接拦下 —— 说明 `agents.toml` 配置错了，宁可拒绝也不动；
  - **云盘同步目录 warning**：源路径含 OneDrive / Dropbox / Google Drive / iCloud → 警告「迁移后云盘认为文件被删，可能触发同步冲突」。
  附单测（Windows/Linux 各覆盖）。另评估了其 `HandleHelper`（NtQuerySystemInformation 句柄枚举兜底，300 行 unsafe）：RM 已修复可用，收益低风险中，**暂不采纳**并在此记录。

### 说明

- 对照过的状态机（`MigrationStateDetector`）确认我们的等价覆盖：`Inconsistent`（联接存在目标缺失）= doctor 的断链检查；「复制中断可恢复」= 重跑 migrate 的增量补齐；`NeedsCleanup`（快照待清理）= 界面快照卡片。

## [0.9.2] - 2026-10-17

### 修正（继续深读参考项目源码，揪出占用检测的两个严重 bug）

对照 [`zennos0609-dotcom/AppDataMover`](https://github.com/zennos0609-dotcom/AppDataMover) 的 Restart Manager 实现（`NativeMethods.cs` 的 `ExpandToFiles` + 两段式 `RmGetList`），发现并修复：

- **🔴 RM 注册混入目录导致整个检测静默失效**：`RmRegisterResources` **只接受文件路径**，混入目录会让注册调用失败——旧代码把失败静默当成「没有占用」并标记「可用」。这意味着**此前的占用检测（blockers 的「谁锁着目录」）从来没有真正工作过**。现在注册前过滤出文件；注册失败如实标记「不可用」，让界面提示用户自行确认（「明确失败优于静默」的落点）。
- **🔴 探测深度不够漏报深层锁**：应用锁的几乎总是深层文件（`User\globalStorage\state.vscdb` 这类），旧探测只查目录本身 + 一级子项。现在广度优先递归（上限 2000、**跳过联接子目录**——不穿过联接去探测目标盘），plan 的探测上限同步 400 → 2000。做法对齐 AppDataMover 的 `ExpandToFiles`。
- 新增回归测试：独占锁一个二级子文件（`share_mode(0)`），`holders_of_dir` 必须报出；probe_paths 递归/上限（unix 版给 Linux CI）。

**说明**：此前测试 47 个全绿但没覆盖「深层文件锁」——这正是「自己从 0 写、没有参考」的盲区；AppDataMover 的 `ExpandToFiles` 存在本身就是这个坑的答案。占用检测失效的兜底是进程名匹配（procs），所以实际风险被部分对冲，但 blockers 的精确性现在才真正成立。

## [0.9.1] - 2026-10-17

### 修正与充实（针对「Agent 条目没有出处」的审查意见：真去读了参考项目的源码）

- **Cursor 条目补全双路径**：`%APPDATA%\Cursor` + `%LOCALAPPDATA%\Cursor` —— 出处 [`taod518/cursor-appdata-mover`](https://github.com/taod518/cursor-appdata-mover) 实证（此前只配了 Roaming 一侧，会漏迁一半数据）。
- **新增飞书条目**：`%APPDATA%\LarkShell` —— 出处 [`linad3d/drivekeeper`](https://github.com/linad3d/drivekeeper) 的 `app-cache-atlas.md` 明确实证「联接后登录保留、新数据写到目标盘」；应用内「下载位置」设置管不到该目录，联接是正解。skip 按 atlas 的子目录判决表配置。
- **`agents.toml` 头部新增「条目来源与验证状态」**：每个条目标注 ✅ 本机实测 / 📄 官方文档 / 📚 开源实证 / ⚠️ Electron 约定推断 —— 把「哪些没验证」诚实摆出来，社区可按条目报 issue。
- **写入机制优先级**（drivekeeper `relocation-methods.md` 的方法论）：应用内设置 > 环境变量 > NTFS 联接（联接是万能兜底而非首选）。
- **写入「明确不建议迁移」清单**：微信 / 企业微信 / QQ（本地消息库是仅本机存在的用户数据，应用内置设置更可靠）、浏览器 profile —— 防止未来误加条目。
- README 调研章节补全：本次实际通读了 5 个参考项目的源码/文档（OPTOPIA、zhao-wuyan、drivekeeper、cursor-appdata-mover、ccmv），标注各自借鉴点。

## [0.9.0] - 2026-10-17

### 新增

- **GUI 中英双语**：零依赖轻量 i18n（`src/i18n.tsx`）—— 界面操作层（侧栏 / 卡片标题 / 主按钮 / 确认框 / 徽章 / 空态 / toast）全部接入；默认跟随系统语言，侧栏底部一键切换并持久化。长说明性文案暂保持中文（与 README 联动）。
- **界面历史筛选与搜索**：历史卡片新增关键字搜索（agent / 路径 / 错误）与类型过滤（全部 / 迁移 / 环境变量 / 失败）。
- **`--with-env` 环境变量策略进迁移主流程**：CLI `acm migrate --with-env` 与界面勾选项 —— 迁移成功后顺带设置官方重定位环境变量（联接 + 变量双保险，数据可从两条路访问，撤销变量也有联接兜底）；GUI 中结果作为 `set-env` 步骤附加到报告。
- **内置注册表新增 4 个**：LM Studio（`~/.lmstudio`）、Ollama（`~/.ollama`，class=safe）、Chatbox（`%APPDATA%\Chatbox`）、Jan（`%APPDATA%\Jan`）—— 共 **22 个**。

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
