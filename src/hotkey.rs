//! 全局快捷键：字符串解析（纯函数，唯一实现处）+ 注册管理器。
//! 解析结果直接面向 [`global_hotkey`] 的 `Modifiers`/`Code`。

use std::str::FromStr;

use global_hotkey::hotkey::{Code, HotKey, Modifiers};
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};

/// 解析快捷键字符串（如 `"Cmd+Shift+V"`）为 `(修饰键, 主键)`。
///
/// - 按 `+` 分段，段 trim，空段 = `Err`
/// - 修饰键（大小写不敏感别名）：`cmd|command|meta|super|win` → `SUPER`；
///   `shift` → `SHIFT`；`alt|option` → `ALT`；`ctrl|control` → `CONTROL`
/// - 主键：单字母 `A-Z` → `KeyA..KeyZ`（大小写均可）；数字 `0-9` → `Digit0..Digit9`；
///   `F1..F12` → `Code::F1..F12`；其余 `Err`
/// - 至少含一个修饰键且恰有一个主键，否则 `Err`
pub fn parse_hotkey(s: &str) -> anyhow::Result<(Modifiers, Code)> {
    let mut modifiers = Modifiers::empty();
    let mut main: Option<Code> = None;
    for segment in s.split('+') {
        let segment = segment.trim();
        if segment.is_empty() {
            anyhow::bail!("empty hotkey segment in {s:?}");
        }
        if let Some(m) = parse_modifier(segment) {
            modifiers |= m;
        } else {
            let code = parse_main_key(segment)
                .ok_or_else(|| anyhow::anyhow!("unsupported hotkey key {segment:?} in {s:?}"))?;
            if main.replace(code).is_some() {
                anyhow::bail!("multiple main keys in {s:?}");
            }
        }
    }
    if modifiers.is_empty() {
        anyhow::bail!("hotkey needs at least one modifier: {s:?}");
    }
    let code = main.ok_or_else(|| anyhow::anyhow!("hotkey needs a main key: {s:?}"))?;
    Ok((modifiers, code))
}

/// 修饰键段（已 trim）→ `Modifiers` 旗标；非修饰键返回 `None`。
fn parse_modifier(segment: &str) -> Option<Modifiers> {
    match segment.to_ascii_lowercase().as_str() {
        "cmd" | "command" | "meta" | "super" | "win" => Some(Modifiers::META),
        "shift" => Some(Modifiers::SHIFT),
        "alt" | "option" => Some(Modifiers::ALT),
        "ctrl" | "control" => Some(Modifiers::CONTROL),
        _ => None,
    }
}

/// 主键段（已 trim）→ `Code`；仅支持单字母、单数字与 `F1..F12`。
fn parse_main_key(segment: &str) -> Option<Code> {
    let code_str = canonical_code_string(segment)?;
    Code::from_str(&code_str).ok()
}

/// 把主键段归一化为 keyboard-types 的 code 字符串（`KeyA` / `Digit0` / `F1`）。
fn canonical_code_string(segment: &str) -> Option<String> {
    let mut chars = segment.chars();
    match (chars.next(), chars.next(), chars.next()) {
        // 单字母（大小写均可）
        (Some(c), None, _) if c.is_ascii_alphabetic() => {
            Some(format!("Key{}", c.to_ascii_uppercase()))
        }
        // 单数字
        (Some(c), None, _) if c.is_ascii_digit() => Some(format!("Digit{c}")),
        // Fn 键：F + 1..=12（f 大小写均可）
        _ => {
            let (first, rest) = segment.split_at(1.min(segment.len()));
            if first.eq_ignore_ascii_case("F") {
                let n: u8 = rest.parse().ok()?;
                (1..=12).contains(&n).then(|| format!("F{n}"))
            } else {
                None
            }
        }
    }
}

/// 全局快捷键注册管理器（装配层唯一触点）。
///
/// 注册依赖运行中的窗口系统，本类型不做单测；`parse_hotkey` 为纯函数全覆盖单测。
pub struct HotkeyManager {
    manager: GlobalHotKeyManager,
    registered: Vec<HotKey>,
}

impl HotkeyManager {
    /// 创建管理器（内部 `GlobalHotKeyManager::new()`）。
    pub fn new() -> anyhow::Result<Self> {
        Ok(Self {
            manager: GlobalHotKeyManager::new()?,
            registered: Vec::new(),
        })
    }

    /// 解析并注册快捷键，返回热键 id（事件轮询用）。
    /// 重新 register 前先 unregister 全部旧键；解析失败/注册失败均 `Err`。
    pub fn register(&mut self, hotkey_str: &str) -> anyhow::Result<u32> {
        let (mods, code) = parse_hotkey(hotkey_str)?;
        if !self.registered.is_empty() {
            self.manager
                .unregister_all(&self.registered)
                .map_err(|e| anyhow::anyhow!("unregister old hotkeys: {e}"))?;
            self.registered.clear();
        }
        let hotkey = HotKey::new(Some(mods), code);
        self.manager
            .register(hotkey)
            .map_err(|e| anyhow::anyhow!("register hotkey {hotkey_str:?}: {e}"))?;
        self.registered.push(hotkey);
        Ok(hotkey.id())
    }

    /// 取尽事件队列，返回被**按下**（Pressed）的热键 id 列表。
    /// macOS 后端对一次物理按压会同时送出 Pressed/Released，这里只上报 Pressed，
    /// 避免上层同一按压触发两次。
    pub fn poll(&self) -> Vec<u32> {
        let receiver = GlobalHotKeyEvent::receiver();
        let mut triggered = Vec::new();
        while let Ok(event) = receiver.try_recv() {
            if event.state() == HotKeyState::Pressed {
                triggered.push(event.id);
            }
        }
        triggered
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ok_cmd_shift_v() {
        let (mods, code) = parse_hotkey("Cmd+Shift+V").unwrap();
        assert!(mods.contains(Modifiers::META));
        assert!(mods.contains(Modifiers::SHIFT));
        assert_eq!(code, Code::KeyV);
    }

    #[test]
    fn ok_ctrl_alt_t_lowercase() {
        let (mods, code) = parse_hotkey("ctrl+alt+t").unwrap();
        assert!(mods.contains(Modifiers::CONTROL));
        assert!(mods.contains(Modifiers::ALT));
        assert!(!mods.contains(Modifiers::META));
        assert_eq!(code, Code::KeyT);
    }

    #[test]
    fn modifier_aliases_map_to_super() {
        for alias in ["Cmd", "command", "META", "super", "Win"] {
            let (mods, code) = parse_hotkey(&format!("{alias}+P")).unwrap();
            assert!(mods.contains(Modifiers::META), "alias {alias:?}");
            assert_eq!(code, Code::KeyP);
        }
    }

    #[test]
    fn modifier_alias_option_maps_to_alt() {
        let (mods, _) = parse_hotkey("Option+K").unwrap();
        assert!(mods.contains(Modifiers::ALT));
        assert!(!mods.contains(Modifiers::META));
    }

    #[test]
    fn ok_digit_and_function_keys() {
        assert_eq!(parse_hotkey("Control+1").unwrap().1, Code::Digit1);
        assert_eq!(parse_hotkey("Control+0").unwrap().1, Code::Digit0);
        assert_eq!(parse_hotkey("Alt+F1").unwrap().1, Code::F1);
        assert_eq!(parse_hotkey("Alt+F12").unwrap().1, Code::F12);
        assert_eq!(parse_hotkey("Alt+f5").unwrap().1, Code::F5);
    }

    #[test]
    fn segments_are_trimmed() {
        let (mods, code) = parse_hotkey("  Cmd + Shift + V  ").unwrap();
        assert!(mods.contains(Modifiers::META) && mods.contains(Modifiers::SHIFT));
        assert_eq!(code, Code::KeyV);
    }

    #[test]
    fn err_option_space_unsupported() {
        assert!(parse_hotkey("Option+Space").is_err());
        assert!(parse_hotkey("Cmd+space").is_err());
    }

    #[test]
    fn err_empty_string() {
        assert!(parse_hotkey("").is_err());
    }

    #[test]
    fn err_main_key_without_modifier() {
        assert!(parse_hotkey("V").is_err());
    }

    #[test]
    fn err_trailing_empty_segment() {
        assert!(parse_hotkey("Cmd+").is_err());
        assert!(parse_hotkey("+Cmd+V").is_err());
        assert!(parse_hotkey("Cmd++V").is_err());
    }

    #[test]
    fn err_unknown_main_key() {
        assert!(parse_hotkey("Cmd+Foo").is_err());
        assert!(parse_hotkey("Cmd+Enter").is_err());
    }

    #[test]
    fn err_modifiers_without_main_key() {
        assert!(parse_hotkey("cmd+shift").is_err());
    }

    #[test]
    fn err_multiple_main_keys() {
        assert!(parse_hotkey("Cmd+V+A").is_err());
    }

    #[test]
    fn err_f_key_out_of_range() {
        assert!(parse_hotkey("Cmd+F0").is_err());
        assert!(parse_hotkey("Cmd+F13").is_err());
    }
}
