//! # acm-core —— AgentCache 内核
//!
//! 零 GUI、零平台 UI 依赖，纯逻辑，可单测。分成四块：
//!
//! - [`agents`] —— Agent 注册表（`agents.toml`）的加载、合并与**检测**
//! - [`paths`] —— 路径模板展开（`~` / `%APPDATA%` / `%LOCALAPPDATA%`）
//! - [`fsutil`] —— 重解析点判定、联接目标、体积统计
//! - [`locks`] —— **精确占用检测**（Windows Restart Manager：谁锁着这个目录）
//! - [`procs`] —— 进程探测（按名字匹配，作为兜底）
//! - [`migrate`] —— **迁移内核**：复制 → 校验 → 快照 → 建联接 → 验证（含回滚）
//!
//! 设计原则：**内核不认识任何具体 Agent**。所有 Agent 知识都在 `agents.toml` 里，
//! 加一个 Agent 只改配置。这样内核可以同时被 GUI、CLI 和别的项目复用。

pub mod agents;
pub mod fsutil;
pub mod locks;
pub mod migrate;
pub mod model;
pub mod paths;
pub mod procs;

pub use agents::{detect, detect_all, load, parse, DetectOpts, BUILTIN_TOML};
pub use locks::Holder;
pub use migrate::{MigrateReport, Plan, Progress, Snapshot};
pub use model::{AgentDef, DetectedAgent, PathHit, RegistryFile};
