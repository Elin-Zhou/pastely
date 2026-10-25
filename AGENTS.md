# AGENTS.md

## Project Overview

ClipHub 是一个 macOS 桌面剪切板历史管理工具（Rust）：后台监听剪切板变化并持久化全部历史（文本 + 图片），支持自定义全局快捷键唤起屏幕中央弹窗，输入焦点自动落在模糊搜索框，回车即粘贴选中条目。支持条目失效周期（TTL）清理。

## Knowledge Map

| Source | Path | Purpose |
|--------|------|---------|
| Architecture | `docs/ARCHITECTURE.md` | 模块分层、依赖规则 |
| Development | `docs/DEVELOPMENT.md` | 构建/测试命令 |
| Product Sense | `docs/PRODUCT_SENSE.md` | 业务上下文与需求要点 |
| Design Docs | `docs/design-docs/` | 组件设计文档 |
| CodeGraph | `.codegraph/` | 代码知识图谱索引 |

## Agent Operating Principles

- Start with this file; follow pointers to deeper sources.
- `docs/` for factual understanding of the codebase.
- Never modify code without consulting relevant docs first.
- Use CodeGraph CLI (cd to app dir + `codegraph query/explore`) for precise symbol lookup and call chain analysis.
- 主仓库始终保持 master 分支；编码一律在 `{CODE_ROOT}/worktrees/{branch}/cliphub/` 中进行。

## Current State

> 项目处于初始化阶段（{INIT_DATE}）：仅有 cargo 骨架。架构文档为按需求预写的初始版本，首个功能分支落地后须回头校准。

| 日期 | 事件 |
|------|------|
| 2026-10-25 | 项目初始化（SDD 工作流） |
| 2026-10-25 | MVP v0.1.0 发布（AppKit 面板/设置/DMG）；应用更名 Pastely |
| 2026-10-25 | 新增自动更新（GitHub Releases + 状态栏提示 + 自动安装重启）与 macOS 10.13+ 兼容（版本守卫 + NSVisualEffectView 降级 + universal2 构建）；见 src/update/ src/compat.rs src/autostart_legacy.rs |
