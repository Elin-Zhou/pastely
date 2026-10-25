//! ClipHub 核心逻辑层：数据模型 / SQLite 存储 / 模糊搜索 / 配置 / TTL 清理 / 快捷键解析。
//!
//! 本 crate 刻意不依赖 macOS UI 框架；AppKit 装配层（bin）位于 `src/main.rs`。

pub mod autostart;
pub mod autostart_legacy;
pub mod clipboard;
pub mod compat;
pub mod config;
pub mod hotkey;
pub mod model;
pub mod panel;
pub mod search;
pub mod store;
pub mod ttl;
pub mod update;
