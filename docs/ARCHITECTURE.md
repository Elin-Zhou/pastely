# ARCHITECTURE.md

> 校准版本（2026-10-26）：基于 master@cae7bcc 实际代码生成，替代 2026-10-25 的预写草案。

## 技术栈

- 语言：Rust（edition 2021），UI 直接用 objc2 系 crate 调原生 AppKit（非 Tauri/egui）
- 平台：macOS 10.13+（高版本特性经 `compat.rs` 特性开关守卫）
- 存储：内嵌 SQLite（rusqlite，WAL，`user_version` 迁移链）
- 搜索：fuzzy-matcher（SkimMatcherV2 评分 + 预览区高亮）
- 配置：TOML（serde + toml）
- 更新检测：ureq（GitHub Releases API）

## 实际模块结构

```
src/
├── main.rs              # 装配层：目录初始化、依赖注入、AppKit 事件循环
├── lib.rs
├── model.rs             # ClipItem 模型（200 字符预览规则）；ListItem 统一行模型
├── expand.rs            # 片段变量展开（{{clipboard}}/{{date:…}}/{{cursor}}，纯函数）
├── clipboard/           # NSPasteboard 监听（changeCount 轮询；文本/图片、截断限制）
│   ├── mod.rs           # Watcher（主线程 NSTimer tick）
│   └── pasteboard.rs
├── store/               # SQLite 持久化（v2 迁移：+ snippets 表）
│   ├── mod.rs           # ClipStore（查询接口供 panel 使用，含片段 CRUD）
│   └── migrations.rs    # user_version 迁移链
├── ttl.rs               # reap_once：过期清理 + 容量驱逐合并
├── search.rs            # fuzzy_filter（SkimMatcherV2）
├── hotkey.rs            # parse_hotkey 纯解析器 + HotkeyManager（global-hotkey）
├── config.rs            # TOML 配置（spec 默认值、load/save、PasteMode、数据目录迁移）
├── panel/               # 弹窗面板与装配中枢（主线程单访）
│   ├── mod.rs           # Globals（泄漏一次的全局态）、AppController（NSTimer tick/状态栏菜单/delegate）
│   ├── list_view.rs     # 分栏玻璃布局、列表渲染
│   ├── search_field.rs  # 非激活 NSPanel 的 field editor 键盘路由、IME 守卫
│   ├── paste_back.rs    # 会话条目回贴 + AX banner
│   ├── snippet_tab.rs   # 设置「快捷片段」Tab：片段列表 + 编辑/删除/新增入口
│   ├── snippet_sheet.rs # 片段新增/编辑 sheet（「插入变量」菜单复用 expand::vars）
│   └── settings.rs      # 设置窗口（Tab 化：通用表单 + 快捷片段 / 快捷键录制等）
├── update/              # 自动更新（GitHub Releases）
│   ├── mod.rs           # UpdateManager 状态机（后台线程 + mpsc → 主线程 tickUpdate:）
│   └── installer.rs     # 下载 DMG→挂载→ditto 拷贝→三段换名原子替换（失败严格回滚）
├── autostart.rs         # 开机自启分派：13+ SMAppService
├── autostart_legacy.rs  # 10.13–12.x LSSharedFileList（常量一律运行时构造字符串）
├── compat.rs            # OS 版本探测与特性开关（glass/SMAppService/symbol image/accent）
└── bin/gen_dmg_bg.rs    # DMG 安装背景生成器（AppKit 渲染，构建辅助）
```

## 依赖规则

- UI（panel）不直接访问 store 内部 SQL，一律走 `ClipStore` 查询接口
- clipboard 产生的条目经 store 落库后才对 panel 可见
- config 为最底层，被所有模块依赖，不依赖任何业务模块
- 全部 OS 版本门槛收敛在 `compat.rs`（规约：禁止魔法值散落）；extern static 禁止直接引用（dyld 启动期解析崩险），高版本 API 调用前须经对应开关守卫

## 关键决策记录

| 日期 | 决策 | 状态 |
|------|------|------|
| 2026-10-25 | 使用 SQLite（rusqlite + WAL + user_version 迁移）做历史持久化 | 已落地 |
| 2026-10-25 | UI 选型：原生 AppKit（objc2）玻璃面板，非 Tauri/egui | 已落地 |
| 2026-10-26 | 自动更新走 GitHub Releases + DMG 原子替换安装 | 已落地（d7eb702） |
| 2026-10-26 | 兼容 macOS 10.13：compat.rs 特性开关 + 运行时字符串规避 extern static | 已落地（d7eb702） |
