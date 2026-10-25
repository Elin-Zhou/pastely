//! OS 版本探测与特性开关：全部版本门槛收敛在此（规约：禁止魔法值散落）。
//!
//! objc2 的类查找是运行时 `objc_getClass` 惰性解析，高版本类只需在调用前
//! 用对应开关守卫即可；extern static 符号例外（dyld 启动期解析），须改为
//! 运行时字符串（见 panel::apply_panel_theme）。

use std::sync::OnceLock;

use objc2::rc::Retained;
use objc2_app_kit::NSColor;
use objc2_foundation::NSProcessInfo;

/// (major, minor) 主次版本（patch 不参与特性判断）。
fn os_version() -> (u64, u64) {
    static CACHE: OnceLock<(u64, u64)> = OnceLock::new();
    *CACHE.get_or_init(|| {
        let v = NSProcessInfo::processInfo().operatingSystemVersion();
        (v.majorVersion as u64, v.minorVersion as u64)
    })
}

fn at_least(major: u64, minor: u64) -> bool {
    let (m, n) = os_version();
    m > major || (m == major && n >= minor)
}

/// `NSGlassEffectView`（液态玻璃底）——macOS 26+。
pub fn glass_available() -> bool {
    at_least(26, 0)
}

/// `SMAppService`（现代登录项）——macOS 13+；否则走 `autostart_legacy`。
pub fn sm_app_service_available() -> bool {
    at_least(13, 0)
}

/// `NSImage::imageWithSystemSymbolName`（SF Symbol）——macOS 11+。
pub fn symbol_image_available() -> bool {
    at_least(11, 0)
}

/// `NSColor::controlAccentColor`——macOS 10.14+；否则 `alternateSelectedControlColor`。
pub fn accent_color_available() -> bool {
    at_least(10, 14)
}

/// 强调色（选中高亮等）：10.14+ 系统 accent；老系统聚焦环色（视觉近似）。
///
/// NSColor 类方法本身全版本存在，但 10.14 前调用 `controlAccentColor`
/// 会触发 doesNotRecognizeSelector，必须守卫。老系统替代 API 已被 Apple
/// 标记 deprecated（仅对新 API 而言），属预期内的兼容选择。
#[allow(deprecated)]
pub fn accent_color() -> Retained<NSColor> {
    if accent_color_available() {
        NSColor::controlAccentColor()
    } else {
        NSColor::alternateSelectedControlColor()
    }
}
