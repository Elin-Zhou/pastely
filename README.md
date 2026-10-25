# Pastely

A macOS clipboard history manager with fuzzy search, built in **pure Rust + AppKit** — no WebView, no Electron.

Press a global hotkey to summon a centered liquid-glass panel, type to fuzzy-search everything you've ever copied (text & images), hit <kbd>↩</kbd> to paste it right where you were.

<!-- TODO: add panel screenshot: <div align="center"><img src=".github/screenshot.png" width="640" alt="Pastely panel"></div> -->

## Features

- 📋 **Full clipboard history** — every text snippet and image you copy is captured in the background (deduplicated, capacity-capped, TTL-expired)
- 🔍 **Fuzzy search** — instant filtering with match highlighting; focus lands in the search field automatically, Chinese IME friendly
- 🖼 **Image previews** — thumbnails in the list, full-size preview pane
- ⚡️ **Paste-back** — pick an entry and it's pasted into the app you were working in (via Accessibility, with graceful copy-only fallback)
- 🧊 **Liquid Glass UI** — macOS 26 `NSGlassEffectView`, split-pane layout with live preview, automatic light/dark adaptation
- ⌨️ **Keyboard-first** — ↑↓ navigate, ↩ paste, Esc dismiss, mouse click to select / double-click to paste
- ⚙️ **In-app settings** — hotkey recorder, TTL, capacity, paste mode, theme, launch-at-login (asked on first run)

## Install

Grab the latest `Pastely-x.y.z.dmg` from [Releases](https://github.com/Elin-Zhou/pastely/releases), open it and drag **Pastely** onto **Applications**.

> **First launch**: macOS will ask for **Accessibility** permission (System Settings → Privacy & Security → Accessibility → enable **Pastely**) so Pastely can paste into other apps. Without it, selecting an entry still copies it to the clipboard.
>
> Ad-hoc signed builds (e.g. built from source) need re-authorization after each rebuild.

## Build from source

```bash
cargo build --release
bash scripts/bundle_app.sh   # produces dist/Pastely.app + dist/Pastely-x.y.z.dmg
```

## Configuration

All settings live in the in-app settings window (⌘, or the status-bar menu), persisted to
`~/Library/Application Support/Pastely/config.toml`.

| Setting | Default | Notes |
|---------|---------|-------|
| Hotkey | `Ctrl+Cmd+V` | recorded by pressing the combo |
| History TTL | 30 days | `0` = keep forever; pinned entries exempt |
| Capacity | 1000 entries | oldest unpinned evicted |
| Paste mode | auto | `auto` = simulate ⌘V after copying |
| Theme | follow system | `light` / `dark` / `auto` |
| Launch at login | off | asked on first run |

## Architecture

Single native process — AppKit run loop on the main thread (panel, status item, timers), SQLite (`rusqlite`) for history, `global-hotkey` for the shortcut, `fuzzy-matcher` (SkimMatcherV2) for search. No third-party UI framework.

```
src/
├── main.rs          # assembly: data dir/migration, logging, timers, status bar
├── clipboard/       # NSPasteboard watcher (changeCount polling, limits, self-echo guard)
├── store/           # SQLite history store + migrations
├── panel/           # NSPanel (nonactivating), search field editor, list, preview, paste-back
├── settings.rs      # glass settings window + hotkey recorder
├── config.rs        # TOML config with defaults
├── search.rs        # fuzzy filter + highlight mapping (char → UTF-16)
├── hotkey.rs        # hotkey parsing/formatting + GlobalHotKeyManager wrapper
├── model.rs         # data model, hashing, preview
└── ttl.rs           # expiry/capacity reaping
```

## 中文简介

macOS 剪贴板历史管理器（纯 Rust + AppKit，无 WebView）：菜单栏常驻，全局快捷键唤起液态玻璃分栏弹窗，输入即模糊搜索全部历史（文本 + 图片），回车直接粘贴回原应用。支持失效周期、容量上限、开机自启动与深浅色自适应。构建与打包见上文，安装包可从 Releases 页面下载。

## License

[MIT](LICENSE) © Elin-Zhou
