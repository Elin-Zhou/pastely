//! 粘贴回填：辅助功能权限检测（含系统授权弹框）+ 模拟 ⌘V 键盘事件。
//!
//! 非激活面板保证原应用始终保持前台，事件经 `CGEventTapLocation::Session`
//! （kCGSessionEventTap）投递即可把 ⌘V 送回用户原本操作的输入位置，
//! 无需定位目标 pid；Session 口与 HID 口等效但作用域限定登录会话，
//! 受限环境（终端继承 TCC、无 root 守护）下更可靠。

use anyhow::anyhow;
use core_foundation::base::TCFType;
use core_foundation::boolean::CFBoolean;
use core_foundation::dictionary::CFDictionary;
use core_foundation::string::CFString;
use core_graphics::event::{CGEvent, CGEventFlags, CGEventTapLocation};
use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
use objc2::runtime::AnyObject;

/// kVK_ANSI_V（HID 虚拟键码 V = 9）。
const K_VK_ANSI_V: u16 = 9;

/// kVK_ANSI_Left（HID 虚拟键码 左方向键 = 123）。
const K_VK_ANSI_LEFT: u16 = 123;

// ApplicationServices 框架的辅助功能信任检查（无需额外 crate）。
#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn AXIsProcessTrusted() -> u8; // Boolean（unsigned char）
    fn AXIsProcessTrustedWithOptions(options: *const AnyObject) -> u8;
}

/// 当前进程是否被授予「辅助功能」权限（无 prompt 版本，仅查询不弹框）。
pub fn is_ax_trusted() -> bool {
    unsafe { AXIsProcessTrusted() != 0 }
}

/// 查询信任状态；未授权时**触发系统授权弹框**
/// （`AXTrustedCheckOptionPrompt = true`，系统只认这个，不会主动弹）。
/// 返回本次查询结果；弹框授权为异步，用户稍后在弹框/系统设置授权后
/// `is_ax_trusted()` 即变 true，走正常粘贴路径。
pub fn request_ax_permission() -> bool {
    let key = CFString::new("AXTrustedCheckOptionPrompt");
    let dict = CFDictionary::from_CFType_pairs(&[(key, CFBoolean::true_value())]);
    // SAFETY: CFDictionaryRef 与 objc2 AnyObject 同为 Objective-C 对象指针，
    // 仅为跨 FFI 传参的指针形态转换；AXIsProcessTrustedWithOptions 只读该字典
    unsafe { AXIsProcessTrustedWithOptions(dict.as_concrete_TypeRef() as *const AnyObject) != 0 }
}

/// 向会话事件口发送一次 Cmd+V 按下/抬起（kCGSessionEventTap）。
pub fn send_cmd_v() -> anyhow::Result<()> {
    let source = CGEventSource::new(CGEventSourceStateID::HIDSystemState)
        .map_err(|()| anyhow!("创建 CGEventSource 失败"))?;
    let cmd = CGEventFlags::CGEventFlagCommand;
    let down = CGEvent::new_keyboard_event(source, K_VK_ANSI_V, true)
        .map_err(|()| anyhow!("创建 Cmd+V keydown 事件失败"))?;
    down.set_flags(cmd);
    let source = CGEventSource::new(CGEventSourceStateID::HIDSystemState)
        .map_err(|()| anyhow!("创建 CGEventSource 失败"))?;
    let up = CGEvent::new_keyboard_event(source, K_VK_ANSI_V, false)
        .map_err(|()| anyhow!("创建 Cmd+V keyup 事件失败"))?;
    up.set_flags(cmd);
    down.post(CGEventTapLocation::Session);
    up.post(CGEventTapLocation::Session);
    Ok(())
}

/// 向会话事件口发送 n 次左方向键按下/抬起（kVK_ANSI_Left = 123，无修饰键）。
///
/// 用于 `{{cursor}}` 展开后把光标从文本末尾左移至落点（每键一位）。
pub fn send_left_arrows(n: usize) -> anyhow::Result<()> {
    for _ in 0..n {
        let source = CGEventSource::new(CGEventSourceStateID::HIDSystemState)
            .map_err(|()| anyhow!("创建 CGEventSource 失败"))?;
        let down = CGEvent::new_keyboard_event(source, K_VK_ANSI_LEFT, true)
            .map_err(|()| anyhow!("创建左方向键 keydown 事件失败"))?;
        let source = CGEventSource::new(CGEventSourceStateID::HIDSystemState)
            .map_err(|()| anyhow!("创建 CGEventSource 失败"))?;
        let up = CGEvent::new_keyboard_event(source, K_VK_ANSI_LEFT, false)
            .map_err(|()| anyhow!("创建左方向键 keyup 事件失败"))?;
        down.post(CGEventTapLocation::Session);
        up.post(CGEventTapLocation::Session);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ax_trusted_does_not_crash() {
        // 只验证链接与调用安全；机器是否已授权因环境而异，不断言值。
        let _ = is_ax_trusted();
    }

    #[test]
    fn send_zero_left_arrows_is_ok_and_posts_nothing() {
        // n=0 不创建事件、不发送按键，仅验证调用安全。
        send_left_arrows(0).unwrap();
    }
}
