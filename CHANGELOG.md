# Changelog

格式参照 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/)，版本号遵循 [SemVer](https://semver.org/lang/zh-CN/)。

## [Unreleased]

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

## [0.1.1] - 2026-10 之前

初始公开版本（详见仓库提交历史）。
