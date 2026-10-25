# Pastely

macOS 剪贴板历史管理器：菜单栏常驻，全局快捷键唤起液态玻璃弹窗，输入即模糊搜索全部复制历史（文本 + 图片），回车直接粘贴回原应用。纯 Rust + AppKit 实现，支持 Intel 与 Apple Silicon。

## 安装

方式一（Homebrew，无 Gatekeeper 弹窗）：

```bash
brew tap elin-zhou/pastely
brew trust elin-zhou/pastely
brew install --cask pastely
```

方式二（终端下载，无 Gatekeeper 弹窗）：

```bash
curl -LO https://github.com/Elin-Zhou/pastely/releases/latest/download/Pastely.dmg
```
（重命名版本号下载后打开 DMG 拖入 Applications；见 [Releases](https://github.com/Elin-Zhou/pastely/releases)）

方式三（浏览器直接下载 DMG）：打开后拖入 Applications。首次打开若被 macOS 拦截（“无法验证开发者”）：系统设置 → 隐私与安全性 → 点「仍要打开」；或执行 `xattr -cr /Applications/Pastely.app`。

首次运行需在 系统设置 → 隐私与安全性 → 辅助功能 中授权 Pastely（用于自动粘贴）。

## 更新

- 默认每 30 分钟自动检测 GitHub Releases 新版本（`config.toml` 的 `[update]` 节可调间隔/关闭）；
- 发现新版本时菜单栏图标切换为下载箭头，菜单出现「发现新版本（点击安装）」；
- 确认后自动下载、安装并重启应用；也可随时从菜单「检查更新…」手动检测。

## 快捷键

- `Ctrl+Cmd+V` 唤起 / 输入即搜索 / `↑↓` 选择 / `↩` 粘贴 / `Esc` 关闭
- `⌘,` 打开设置（快捷键、失效周期、容量、开机自启等）

## 从源码构建

```bash
cargo build --release
bash scripts/bundle_app.sh   # 产出 dist/Pastely.app 与 DMG（默认 universal2 双架构，UNIVERSAL=0 单架构）
```

要求 macOS 10.13+（Intel 与 Apple Silicon）。macOS 26+ 显示真液态玻璃弹窗；老系统自动降级为磨砂半透明材质（NSVisualEffectView）。

## License

[MIT](LICENSE)
