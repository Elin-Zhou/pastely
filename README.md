# Pastely

macOS 剪贴板历史管理器（Rust + AppKit）：菜单栏常驻、模糊搜索弹窗、图片预览、真液态玻璃分栏 UI（macOS 26 `NSGlassEffectView`）。

## 构建

```bash
cargo build --release
```

## 打包

```bash
bash scripts/bundle_app.sh
```

产出 `dist/Pastely.app` 与 `dist/Pastely-0.1.0.dmg`（ad-hoc 签名；分发请用开发者证书重签）。

## 安装

打开 `dist/Pastely-0.1.0.dmg`，把 **Pastely.app** 拖动到 Applications 后启动。

**首次运行需授权辅助功能**（自动粘贴依赖模拟 ⌘V）：
系统设置 → 隐私与安全性 → 辅助功能 → 勾选 Pastely。
未授权时自动粘贴降级为仅复制，面板顶部会出现横幅，点「打开系统设置」可直达授权页。

## 使用

- 默认快捷键 `Ctrl+Cmd+V` 唤起面板；输入即模糊过滤，↑↓ 选择、⏎ 确认、数字键 `1-9` 直达
- 菜单栏 📋 图标：显示 / 设置…（⌘,）/ 暂停记录 / 清空历史 / 在 Finder 中显示配置 / 退出

## 配置

菜单栏「设置…」可调整：快捷键（保存即热重载）、失效周期（天，0=永久）、容量上限（条）、粘贴模式（自动粘贴 / 仅复制）、主题（跟随系统 / 浅色 / 深色，保存即生效）、列表行数（下次启动生效）。

配置文件位置：`~/Library/Application Support/cliphub/config.toml`（历史库同目录 `history.db`）。
