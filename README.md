# Pastely

macOS 剪贴板历史管理器：菜单栏常驻，全局快捷键唤起液态玻璃弹窗，输入即模糊搜索全部复制历史（文本 + 图片），回车直接粘贴回原应用。纯 Rust + AppKit 实现。

## 安装

从 [Releases](https://github.com/Elin-Zhou/pastely/releases) 下载最新 `Pastely-x.y.z.dmg`，打开后把 Pastely 拖入 Applications。

首次打开若被 macOS 拦截（“无法验证开发者”）：系统设置 → 隐私与安全性 → 点「仍要打开」；或执行 `xattr -cr /Applications/Pastely.app`。

首次运行需在 系统设置 → 隐私与安全性 → 辅助功能 中授权 Pastely（用于自动粘贴）。

## 快捷键

- `Ctrl+Cmd+V` 唤起 / 输入即搜索 / `↑↓` 选择 / `↩` 粘贴 / `Esc` 关闭
- `⌘,` 打开设置（快捷键、失效周期、容量、开机自启等）

## 从源码构建

```bash
cargo build --release
bash scripts/bundle_app.sh   # 产出 dist/Pastely.app 与 DMG
```

要求 macOS 26+。

## License

[MIT](LICENSE)
