# ARCHITECTURE.md

> ⚠️ 初始版本（2026-10-25）：项目尚无业务代码，本文档基于需求预写，供技术方案阶段参考。首个版本落地后必须校准。

## 技术栈

- 语言：Rust（edition 2021）
- 平台：macOS（darwin，Apple Silicon 优先）
- 存储：内嵌 SQLite（rusqlite，schema 由应用自管）
- UI/弹窗与全局快捷键：待技术方案阶段定稿（候选：Tauri / egui+winit / 原生 AppKit 绑定）

## 预期模块分层（草案）

```
cliphub/
├── src/
│   ├── main.rs          # 入口：启动后台服务 + 事件循环
│   ├── clipboard/       # 剪切板监听与读写（文本/图片，含差异检测）
│   ├── store/           # 持久化（SQLite：条目、TTL 过期清理）
│   ├── hotkey/          # 全局快捷键注册与分发
│   ├── ui/              # 弹窗 UI（搜索框焦点、模糊搜索、列表渲染）
│   ├── search/          # 模糊匹配算法
│   └── config/          # 用户配置（快捷键、TTL、开关）
```

## 依赖规则

- `ui` 不得直接访问 `store` 内部 SQL，一律走 `store` 暴露的查询接口
- `clipboard` 产生的条目经 `store` 落库后才对 `ui` 可见
- `config` 为最底层，被所有模块依赖，不依赖任何业务模块

## 关键决策记录

| 日期 | 决策 | 状态 |
|------|------|------|
| 2026-10-25 | 使用 SQLite 做历史持久化 | 待方案确认 |
| 2026-10-25 | UI 框架选型 | 待 brainstorming |
