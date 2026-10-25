//! 粘贴回填：辅助功能权限检测 + 模拟 ⌘V 键盘事件。
//!
//! 非激活面板保证原应用始终保持前台，因此 `CGEventPost(kCGHIDEventTap)`
//! 即可把事件送回用户原本操作的输入位置，无需定位目标 pid。

use anyhow::anyhow;
use core_graphics::event::{CGEvent, CGEventFlags, CGEventTapLocation};
use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};

/// kVK_ANSI_V（HID 虚拟键码 V = 9）。
const K_VK_ANSI_V: u16 = 9;

// ApplicationServices 框架的辅助功能信任检查（无需额外 crate）。
#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn AXIsProcessTrusted() -> u8; // Boolean（unsigned char）
}

/// 当前进程是否被授予「辅助功能」权限。
pub fn is_ax_trusted() -> bool {
    unsafe { AXIsProcessTrusted() != 0 }
}

/// 向系统 HID 事件口发送一次 Cmd+V 按下/抬起。
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
    down.post(CGEventTapLocation::HID);
    up.post(CGEventTapLocation::HID);
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
}
