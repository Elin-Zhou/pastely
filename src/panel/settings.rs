//! 设置窗口：单例玻璃窗口 + 表单（快捷键 / 存储限额 / 粘贴模式 / 主题 / 列表行数）。
//!
//! 打开入口：状态栏菜单「设置…」（⌘,）与主面板 field editor 的 ⌘,。
//! 保存/取消经 nil-target 之外显式 target 到 [`super::AppController`] 的
//! `saveSettings:` / `closeSettings:`；保存校验失败弹 NSAlert 且不关窗。

use objc2::define_class;
use objc2::msg_send;
use objc2::rc::Retained;
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSAlert, NSApplication, NSBackingStoreType, NSBeep, NSBezelStyle, NSButton, NSButtonType,
    NSColor, NSEvent, NSEventModifierFlags, NSFont, NSGlassEffectView, NSGlassEffectViewStyle,
    NSPopUpButton, NSTextField, NSView, NSWindow, NSWindowStyleMask,
};
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use objc2_foundation::NSString;

use super::AppController;
use crate::config::{Config, PasteMode};

/// 窗口尺寸。
pub const SETTINGS_WIDTH: f64 = 400.0;
pub const SETTINGS_HEIGHT: f64 = 470.0;
/// 顶部内缩：FullSizeContentView 下红绿灯画在内容区左上，表单整体下移避开。
const TOP_INSET: f64 = 36.0;

/// 表单容器高（= 窗口高 - 顶部内缩）。
fn form_height() -> f64 {
    SETTINGS_HEIGHT - TOP_INSET
}

/// 表单行几何：第 `k` 行控件底 y（容器内非翻转坐标，自顶部下排）。
fn row_y(k: usize) -> f64 {
    let row_h = 24.0;
    let gap = 10.0;
    let top = form_height() - 16.0;
    top - (k as f64 + 1.0) * row_h - k as f64 * gap
}

fn rect(x: f64, y: f64, w: f64, h: f64) -> CGRect {
    CGRect::new(CGPoint::new(x, y), CGSize::new(w, h))
}

/// 快捷键录制框的 ivars（录制状态 + Globals 指针供 suspend/resume）。
pub struct HotkeyRecorderIvars {
    /// 进入录制前的原值（None = 未在录制）。
    saved: std::cell::RefCell<Option<String>>,
    globals: *const super::Globals,
}

define_class!(
    // SAFETY: NSTextField 无子类化约束；本类型不实现 Drop。
    #[unsafe(super(NSTextField))]
    #[thread_kind = MainThreadOnly]
    #[name = "PastelyHotkeyRecorder"]
    #[ivars = HotkeyRecorderIvars]
    pub struct HotkeyRecorderField;

    impl HotkeyRecorderField {
        /// 非可编辑字段默认不接受焦点，录制需要点击获得焦点。
        #[unsafe(method(acceptsFirstResponder))]
        unsafe fn accepts_first_responder(&self) -> bool {
            true
        }

        /// 点击进入录制：保存原值、显示提示态、暂停全局快捷键。
        #[unsafe(method(mouseDown:))]
        unsafe fn mouse_down(&self, _event: &NSEvent) {
            if self.ivars().saved.borrow().is_some() {
                return; // 已在录制
            }
            *self.ivars().saved.borrow_mut() = Some(self.stringValue().to_string());
            self.setStringValue(&NSString::from_str(""));
            self.setPlaceholderString(Some(&NSString::from_str("按下快捷键…")));
            let globals = unsafe { &*self.ivars().globals };
            if let Err(e) = globals.hotkey.borrow_mut().suspend() {
                tracing::warn!("暂停全局快捷键失败: {e:#}");
            }
            if let Some(w) = self.window() {
                w.makeFirstResponder(Some(self));
            }
        }

        /// 失焦（捕获成功退出 / 点击别处 / 关窗）：恢复原值并恢复全局快捷键。
        #[unsafe(method(resignFirstResponder))]
        unsafe fn resign_first_responder(&self) -> bool {
            self.finish_recording();
            let resumed: bool = unsafe { msg_send![super(self), resignFirstResponder] };
            resumed
        }

        /// 捕获按键：修饰键 + 主键（字母/数字/F1-F12）合成 "Ctrl+Cmd+V" 风格字符串。
        /// 纯修饰键不产生 keyDown（flagsChanged），自然忽略；不支持的键提示音并继续等待。
        #[unsafe(method(keyDown:))]
        unsafe fn key_down(&self, event: &NSEvent) {
            let mods = event.modifierFlags();
            let mut parts: Vec<String> = Vec::new();
            if mods.contains(NSEventModifierFlags::Control) {
                parts.push("Ctrl".to_string());
            }
            if mods.contains(NSEventModifierFlags::Option) {
                parts.push("Alt".to_string());
            }
            if mods.contains(NSEventModifierFlags::Shift) {
                parts.push("Shift".to_string());
            }
            if mods.contains(NSEventModifierFlags::Command) {
                parts.push("Cmd".to_string());
            }
            let chars = event
                .charactersIgnoringModifiers()
                .map(|s| s.to_string())
                .unwrap_or_default();
            match vk_to_key_name(event.keyCode(), &chars) {
                Some(name) if !parts.is_empty() => {
                    parts.push(name);
                    let joined = parts.join("+");
                    // 先记为新值再退焦：resign 恢复的就是捕获结果
                    *self.ivars().saved.borrow_mut() = Some(joined.clone());
                    self.setStringValue(&NSString::from_str(&joined));
                    // 退出录制，焦点还给窗口
                    if let Some(w) = self.window() {
                        w.makeFirstResponder(None);
                    }
                }
                _ => {
                    NSBeep();
                }
            }
        }
    }
);

impl HotkeyRecorderField {
    /// 结束录制：恢复原值/捕获值 + 恢复全局快捷键（幂等；未在录制时为 no-op）。
    pub(crate) fn finish_recording(&self) {
        if let Some(saved) = self.ivars().saved.borrow_mut().take() {
            self.setStringValue(&NSString::from_str(&saved));
            self.setPlaceholderString(Some(&NSString::from_str("点击后按组合键录制")));
            let globals = unsafe { &*self.ivars().globals };
            if let Err(e) = globals.hotkey.borrow_mut().resume() {
                tracing::warn!("恢复全局快捷键失败: {e:#}");
            }
        }
    }
}

/// 硬件 keyCode（+ 主键字符）→ `parse_hotkey` 兼容的主键名。
/// 仅支持字母 / 数字 / F1-F12（与 parse_hotkey 支持集一致）；其余 `None`。
fn vk_to_key_name(key_code: u16, chars: &str) -> Option<String> {
    /// Apple kVK_F1..F12 硬件码表（不连续，必须查表）。
    const F_KEYS: [(u16, u8); 12] = [
        (122, 1),
        (119, 2),
        (99, 3),
        (118, 4),
        (96, 5),
        (97, 6),
        (98, 7),
        (100, 8),
        (101, 9),
        (109, 10),
        (103, 11),
        (111, 12),
    ];
    if let Some((_, n)) = F_KEYS.iter().find(|(code, _)| *code == key_code) {
        return Some(format!("F{n}"));
    }
    let mut it = chars.chars();
    match (it.next(), it.next()) {
        (Some(c), None) if c.is_ascii_alphabetic() => Some(c.to_ascii_uppercase().to_string()),
        (Some(c), None) if c.is_ascii_digit() => Some(c.to_string()),
        _ => None,
    }
}

/// 设置窗口 + 表单控件句柄（泄漏单例，主线程访问）。
pub struct SettingsWindow {
    pub window: Retained<NSWindow>,
    launch_btn: Retained<NSButton>,
    ax_status: Retained<NSTextField>,
    ax_open_btn: Retained<NSButton>,
    hotkey_field: Retained<HotkeyRecorderField>,
    ttl_field: Retained<NSTextField>,
    capacity_field: Retained<NSTextField>,
    paste_popup: Retained<NSPopUpButton>,
    theme_popup: Retained<NSPopUpButton>,
    rows_field: Retained<NSTextField>,
}

impl SettingsWindow {
    /// 每次打开时从当前 config 回填控件。
    fn refill(&self, config: &Config) {
        self.launch_btn.setState(if config.general.launch_at_login {
            objc2_app_kit::NSControlStateValueOn
        } else {
            objc2_app_kit::NSControlStateValueOff
        });
        self.hotkey_field
            .setStringValue(&NSString::from_str(&config.hotkey.main));
        self.ttl_field
            .setStringValue(&NSString::from_str(&config.storage.ttl_days.to_string()));
        self.capacity_field
            .setStringValue(&NSString::from_str(&config.storage.max_items.to_string()));
        self.paste_popup.removeAllItems();
        self.paste_popup
            .addItemWithTitle(&NSString::from_str("自动粘贴"));
        self.paste_popup
            .addItemWithTitle(&NSString::from_str("仅复制"));
        self.paste_popup.selectItemAtIndex(match config.paste.mode {
            PasteMode::Auto => 0,
            PasteMode::CopyOnly => 1,
        });
        self.theme_popup.removeAllItems();
        self.theme_popup
            .addItemWithTitle(&NSString::from_str("跟随系统"));
        self.theme_popup
            .addItemWithTitle(&NSString::from_str("浅色"));
        self.theme_popup
            .addItemWithTitle(&NSString::from_str("深色"));
        self.theme_popup
            .selectItemAtIndex(match config.ui.theme.as_str() {
                "light" => 1,
                "dark" => 2,
                _ => 0,
            });
        self.rows_field
            .setStringValue(&NSString::from_str(&config.ui.max_list_rows.to_string()));
        self.set_ax_status(super::paste_back::is_ax_trusted());
    }

    // —— 保存时读值（AppController::saveSettings: 调用）——

    pub(crate) fn launch_checked(&self) -> bool {
        self.launch_btn.state() == objc2_app_kit::NSControlStateValueOn
    }

    pub(crate) fn hotkey_string(&self) -> String {
        self.hotkey_field.stringValue().to_string()
    }

    pub(crate) fn ttl_string(&self) -> String {
        self.ttl_field.stringValue().to_string()
    }

    pub(crate) fn capacity_string(&self) -> String {
        self.capacity_field.stringValue().to_string()
    }

    pub(crate) fn rows_string(&self) -> String {
        self.rows_field.stringValue().to_string()
    }

    pub(crate) fn paste_mode(&self) -> PasteMode {
        if self.paste_popup.indexOfSelectedItem() == 1 {
            PasteMode::CopyOnly
        } else {
            PasteMode::Auto
        }
    }

    pub(crate) fn theme_string(&self) -> String {
        match self.theme_popup.indexOfSelectedItem() {
            1 => "light".to_string(),
            2 => "dark".to_string(),
            _ => "auto".to_string(),
        }
    }

    pub(crate) fn order_out(&self) {
        self.window.orderOut(None);
    }

    /// 结束录制（取消/关窗/保存前调用，恢复全局快捷键）。
    pub(crate) fn finish_recording(&self) {
        self.hotkey_field.finish_recording();
    }

    /// 刷新授权状态行（refill 与重新授权后调用）。
    pub(crate) fn set_ax_status(&self, trusted: bool) {
        self.ax_status.setStringValue(&NSString::from_str(if trusted {
            "已授权 ✓"
        } else {
            "未授权"
        }));
        self.ax_status.setTextColor(Some(if trusted {
            &NSColor::labelColor()
        } else {
            &NSColor::systemOrangeColor()
        }));
        self.ax_open_btn.setHidden(trusted);
    }
}

/// 打开设置窗口（单例复用；每次打开回填当前配置）。
pub fn show(globals: &'static super::Globals, controller: &AppController) {
    // Accessory 应用未激活时，普通窗口会排在当前前台应用的所有窗口之后：
    // 先激活自身再 makeKey，保证设置窗口置顶可见（复用与首次两个分支都需要）
    let mtm = MainThreadMarker::new().expect("main thread");
    NSApplication::sharedApplication(mtm).activate();
    if let Some(w) = *globals.settings.borrow() {
        w.refill(&globals.config.borrow());
        w.window.makeKeyAndOrderFront(None);
        return;
    }
    let w = unsafe { build(mtm, controller, globals) };
    w.refill(&globals.config.borrow());
    w.window.center();
    w.window.makeKeyAndOrderFront(None);
    // 泄漏：进程生命周期（releasedWhenClosed=false，窗口可反复复用）
    let leaked: &'static SettingsWindow = Box::leak(Box::new(w));
    *globals.settings.borrow_mut() = Some(leaked);
}

/// 校验失败弹窗（不关窗，用户可改后重试）。
pub fn show_error(title: &str, msg: &str) {
    let mtm = MainThreadMarker::new().expect("main thread");
    unsafe {
        let alert: Retained<NSAlert> = msg_send![NSAlert::alloc(mtm), init];
        alert.setMessageText(&NSString::from_str(title));
        alert.setInformativeText(&NSString::from_str(msg));
        alert.runModal();
    }
}

/// 保存：校验 → 持久化 → 更新 Globals.config → 快捷键热重载 → 主题即时生效 → 关窗。
/// 校验/持久化失败弹窗且不关窗。
pub fn save(globals: &'static super::Globals) {
    let Some(w) = *globals.settings.borrow() else {
        return;
    };
    w.finish_recording();

    // —— 校验（任一失败弹窗且不关窗）——
    let hotkey = w.hotkey_string();
    if let Err(e) = crate::hotkey::parse_hotkey(&hotkey) {
        show_error("快捷键无效", &format!("{e:#}\n示例：Ctrl+Cmd+V"));
        return;
    }
    let ttl_days: u32 = match w.ttl_string().trim().parse() {
        Ok(v) => v,
        Err(_) => {
            show_error("失效周期无效", "须为非负整数（天），0 表示永久保留。");
            return;
        }
    };
    let max_items: usize = match w.capacity_string().trim().parse() {
        Ok(v) => v,
        Err(_) => {
            show_error("容量上限无效", "须为非负整数（条）。");
            return;
        }
    };
    let max_list_rows: usize = match w.rows_string().trim().parse() {
        Ok(v) => v,
        Err(_) => {
            show_error("列表行数无效", "须为非负整数（行）。");
            return;
        }
    };

    let launch_now = w.launch_checked();
    let launch_prev = globals.config.borrow().general.launch_at_login;
    if launch_now != launch_prev {
        if let Err(e) = crate::autostart::set_launch_at_login(launch_now) {
            show_error(
                "开机自启动设置失败",
                &format!("{e:#}\n（cargo run 直跑时不可用；请使用打包后的 Pastely.app）"),
            );
            return;
        }
        tracing::info!(
            "开机自启动已{}（SMAppService）",
            if launch_now { "开启" } else { "关闭" }
        );
    }

    let mut cfg = globals.config.borrow().clone();
    cfg.general.launch_at_login = launch_now;
    cfg.general.first_run_done = true;
    cfg.hotkey.main = hotkey;
    cfg.storage.ttl_days = ttl_days;
    cfg.storage.max_items = max_items;
    cfg.paste.mode = w.paste_mode();
    cfg.ui.theme = w.theme_string();
    cfg.ui.max_list_rows = max_list_rows;

    // —— 持久化 ——
    if let Err(e) = cfg.save(&globals.config_path) {
        show_error("保存配置失败", &format!("{e:#}"));
        return;
    }
    *globals.config.borrow_mut() = cfg.clone();

    // watcher 限额同步：tick 直接读 WatcherState 字段，须显式同步才即时生效
    {
        let mut watcher = globals.watcher.borrow_mut();
        watcher.max_text_chars = cfg.storage.max_text_chars;
        watcher.max_image_mb = cfg.storage.max_image_mb;
    }

    // 快捷键热重载（register 先 unregister 全部旧键）
    match globals.hotkey.borrow_mut().register(&cfg.hotkey.main) {
        Ok(_) => tracing::info!("快捷键已热重载: {:?}", cfg.hotkey.main),
        Err(e) => tracing::error!("快捷键热重载失败（原快捷键保留，下次启动重试）: {e:#}"),
    }

    // 主题即时生效（面板已装配时；行数为下次启动生效）
    if let Some(p) = globals.panel() {
        let theme = cfg.ui.theme.clone();
        unsafe { super::apply_panel_theme(&p.panel, &theme) };
    }

    tracing::info!(
        "设置已保存：hotkey={:?} ttl_days={} max_items={} mode={:?} theme={} rows={}（行数下次启动生效）",
        cfg.hotkey.main,
        cfg.storage.ttl_days,
        cfg.storage.max_items,
        cfg.paste.mode,
        cfg.ui.theme,
        cfg.ui.max_list_rows
    );
    w.order_out();
}

/// 关闭设置窗口（取消 / Esc）。
pub fn close(globals: &'static super::Globals) {
    if let Some(w) = *globals.settings.borrow() {
        w.finish_recording();
        w.order_out();
    }
}

/// 「重新授权」：弹系统授权框（prompt 版），完成后刷新状态行。
pub fn request_permission(globals: &'static super::Globals) {
    if let Some(w) = *globals.settings.borrow() {
        let trusted = super::paste_back::request_ax_permission();
        w.set_ax_status(trusted);
        tracing::info!("重新授权完成：trusted={trusted}");
    }
}

/// 构建窗口 + 表单（仅首次打开调用；必须主线程）。
///
/// # Safety
///
/// 必须在主线程调用（mtm 保证）。
unsafe fn build(
    mtm: MainThreadMarker,
    controller: &AppController,
    globals: &'static super::Globals,
) -> SettingsWindow {
    unsafe {
        let content = rect(0.0, 0.0, SETTINGS_WIDTH, SETTINGS_HEIGHT);
        let mask = NSWindowStyleMask::Titled
            | NSWindowStyleMask::Closable
            | NSWindowStyleMask::Resizable
            | NSWindowStyleMask::FullSizeContentView;
        let window = NSWindow::initWithContentRect_styleMask_backing_defer(
            NSWindow::alloc(mtm),
            content,
            mask,
            NSBackingStoreType::Buffered,
            false,
        );
        window.setTitle(&NSString::from_str("Pastely 设置"));
        window.setTitleVisibility(objc2_app_kit::NSWindowTitleVisibility::Hidden);
        window.setTitlebarAppearsTransparent(true);
        window.setReleasedWhenClosed(false);
        // 可激活常规窗口：文本框可编辑输入
        window.setOpaque(false);
        window.setBackgroundColor(Some(&NSColor::clearColor()));

        // 玻璃底（与主面板同款）
        let glass = NSGlassEffectView::initWithFrame(
            NSGlassEffectView::alloc(mtm),
            rect(0.0, 0.0, SETTINGS_WIDTH, SETTINGS_HEIGHT),
        );
        glass.setStyle(NSGlassEffectViewStyle::Regular);
        glass.setCornerRadius(12.0);
        if let Some(cv) = window.contentView() {
            cv.addSubview(&glass);
        }

        // 表单容器：整体位于红绿灯区域之下（TOP_INSET），行几何全部相对容器
        let form = NSView::initWithFrame(
            NSView::alloc(mtm),
            rect(0.0, 0.0, SETTINGS_WIDTH, form_height()),
        );
        if let Some(cv) = window.contentView() {
            cv.addSubview(&form);
        }

        let label = |text: &str, y: f64| {
            let l = NSTextField::labelWithString(&NSString::from_str(text), mtm);
            l.setFont(Some(&NSFont::systemFontOfSize(12.0)));
            l.setTextColor(Some(&NSColor::labelColor()));
            l.setFrame(rect(16.0, y + 4.0, 136.0, 16.0));
            l
        };
        let field = |placeholder: &str, y: f64, w: f64| {
            let f = NSTextField::initWithFrame(NSTextField::alloc(mtm), rect(158.0, y, w, 24.0));
            f.setPlaceholderString(Some(&NSString::from_str(placeholder)));
            f
        };
        let popup = |y: f64| {
            NSPopUpButton::initWithFrame_pullsDown(
                NSPopUpButton::alloc(mtm),
                rect(158.0, y, 226.0, 24.0),
                false,
            )
        };

        // 0. 开机自启动（checkbox，Switch 样式）
        let launch_btn =
            NSButton::initWithFrame(NSButton::alloc(mtm), rect(158.0, row_y(0), 226.0, 24.0));
        launch_btn.setTitle(&NSString::from_str(""));
        launch_btn.setButtonType(NSButtonType::Switch);
        // 1. 快捷键（录制控件：点击后按组合键捕获）
        let hotkey_alloc = HotkeyRecorderField::alloc(mtm)
            .set_ivars(HotkeyRecorderIvars {
                saved: std::cell::RefCell::new(None),
                globals: globals as *const super::Globals,
            });
        let hotkey_field: Retained<HotkeyRecorderField> = msg_send![super(hotkey_alloc), initWithFrame: rect(
            158.0,
            row_y(1),
            226.0,
            24.0
        )];
        hotkey_field.setPlaceholderString(Some(&NSString::from_str("点击后按组合键录制")));
        hotkey_field.setEditable(false);
        hotkey_field.setSelectable(false);
        // 2. 失效周期（天，0=永久）
        let ttl_field = field("0 = 永久", row_y(2), 226.0);
        // 3. 容量上限（条）
        let capacity_field = field("1000", row_y(3), 226.0);
        // 4. 粘贴模式
        let paste_popup = popup(row_y(4));
        // 5. 主题
        let theme_popup = popup(row_y(5));
        // 6. 列表行数（旁注：下次启动生效）
        let rows_field = field("10", row_y(6), 80.0);
        let rows_note = {
            let n = NSTextField::labelWithString(&NSString::from_str("下次启动生效"), mtm);
            n.setFont(Some(&NSFont::systemFontOfSize(10.0)));
            n.setTextColor(Some(&NSColor::secondaryLabelColor()));
            n.setFrame(rect(246.0, row_y(6) + 5.0, 138.0, 14.0));
            n
        };

        // 7. 自动粘贴授权：状态文本 + 重新授权 + 打开系统设置（未授权时显示）
        let ax_status = {
            let l = NSTextField::labelWithString(&NSString::from_str("未授权"), mtm);
            l.setFont(Some(&NSFont::systemFontOfSize(11.0)));
            l.setFrame(rect(158.0, row_y(7) + 5.0, 66.0, 14.0));
            l
        };
        let ax_request_btn = NSButton::initWithFrame(
            NSButton::alloc(mtm),
            rect(230.0, row_y(7), 62.0, 22.0),
        );
        ax_request_btn.setTitle(&NSString::from_str("重新授权"));
        ax_request_btn.setBezelStyle(NSBezelStyle::Push);
        ax_request_btn.setFont(Some(&NSFont::systemFontOfSize(11.0)));
        let target: &objc2::runtime::AnyObject = controller;
        ax_request_btn.setTarget(Some(target));
        ax_request_btn.setAction(Some(objc2::sel!(requestAxPermission:)));
        let ax_open_btn = NSButton::initWithFrame(
            NSButton::alloc(mtm),
            rect(296.0, row_y(7), 88.0, 22.0),
        );
        ax_open_btn.setTitle(&NSString::from_str("打开系统设置"));
        ax_open_btn.setBezelStyle(NSBezelStyle::Push);
        ax_open_btn.setFont(Some(&NSFont::systemFontOfSize(11.0)));
        ax_open_btn.setTarget(Some(target));
        ax_open_btn.setAction(Some(objc2::sel!(openAccessibilitySettings:)));

        // 底部按钮：保存（默认高亮，回车）/ 取消（Esc）
        let target: &objc2::runtime::AnyObject = controller;
        let save_btn = NSButton::initWithFrame(
            NSButton::alloc(mtm),
            rect(SETTINGS_WIDTH - 16.0 - 96.0, 16.0, 96.0, 28.0),
        );
        save_btn.setTitle(&NSString::from_str("保存"));
        save_btn.setBezelStyle(NSBezelStyle::Push);
        save_btn.setKeyEquivalent(&NSString::from_str("\r"));
        save_btn.setTarget(Some(target));
        save_btn.setAction(Some(objc2::sel!(saveSettings:)));
        let cancel_btn = NSButton::initWithFrame(
            NSButton::alloc(mtm),
            rect(SETTINGS_WIDTH - 16.0 - 96.0 - 8.0 - 96.0, 16.0, 96.0, 28.0),
        );
        cancel_btn.setTitle(&NSString::from_str("取消"));
        cancel_btn.setBezelStyle(NSBezelStyle::Push);
        cancel_btn.setKeyEquivalent(&NSString::from_str("\u{1b}"));
        cancel_btn.setTarget(Some(target));
        cancel_btn.setAction(Some(objc2::sel!(closeSettings:)));

        // AX 授权提示（ad-hoc 签名重打包后需重新授权）
        let ax_note = {
            let n = NSTextField::labelWithString(
                &NSString::from_str(
                    "若已授权但无法自动粘贴：重新打包后请在系统设置删除 Pastely 并重新授权",
                ),
                mtm,
            );
            n.setFont(Some(&NSFont::systemFontOfSize(10.0)));
            n.setTextColor(Some(&NSColor::secondaryLabelColor()));
            n.setFrame(rect(16.0, 52.0, SETTINGS_WIDTH - 32.0, 14.0));
            n
        };

        // 挂载：玻璃 → 表单容器（标签 + 控件 + 注记 + 按钮）
        if let Some(cv) = window.contentView() {
            cv.addSubview(&form);
        }
        for (text, y) in [
            ("开机自启动", row_y(0)),
            ("快捷键", row_y(1)),
            ("失效周期（天，0=永久）", row_y(2)),
            ("容量上限（条）", row_y(3)),
            ("粘贴模式", row_y(4)),
            ("主题", row_y(5)),
            ("列表行数", row_y(6)),
            ("自动粘贴授权", row_y(7)),
        ] {
            let l = label(text, y);
            form.addSubview(&l);
        }
        form.addSubview(&launch_btn);
        form.addSubview(&hotkey_field);
        form.addSubview(&ttl_field);
        form.addSubview(&capacity_field);
        form.addSubview(&paste_popup);
        form.addSubview(&theme_popup);
        form.addSubview(&rows_field);
        form.addSubview(&rows_note);
        form.addSubview(&ax_status);
        form.addSubview(&ax_request_btn);
        form.addSubview(&ax_open_btn);
        form.addSubview(&ax_note);
        form.addSubview(&save_btn);
        form.addSubview(&cancel_btn);

        SettingsWindow {
            window,
            launch_btn,
            ax_status,
            ax_open_btn,
            hotkey_field,
            ttl_field,
            capacity_field,
            paste_popup,
            theme_popup,
            rows_field,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hotkey::parse_hotkey;
    use global_hotkey::hotkey::{Code, Modifiers};

    #[test]
    fn vk_letters_and_digits() {
        assert_eq!(vk_to_key_name(9, "v"), Some("V".to_string()));
        assert_eq!(vk_to_key_name(0, "a"), Some("A".to_string()));
        assert_eq!(vk_to_key_name(18, "1"), Some("1".to_string()));
    }

    #[test]
    fn vk_f_key_table() {
        assert_eq!(vk_to_key_name(122, ""), Some("F1".to_string()));
        assert_eq!(vk_to_key_name(99, ""), Some("F3".to_string()));
        assert_eq!(vk_to_key_name(111, ""), Some("F12".to_string()));
    }

    #[test]
    fn vk_unsupported_is_none() {
        // 方向键功能字符 / 空格 / 多字符（死键组合）/ 空串都不支持
        assert_eq!(vk_to_key_name(125, "\u{f701}"), None);
        assert_eq!(vk_to_key_name(49, " "), None);
        assert_eq!(vk_to_key_name(0, "ab"), None);
        assert_eq!(vk_to_key_name(0, ""), None);
    }

    #[test]
    fn recorder_output_parses() {
        // 录制产物（修饰序固定 Ctrl,Alt,Shift,Cmd + 主键名）必然可解析，
        // 且与 format_hotkey 的往返性质一致
        for (mods, code) in [
            (Modifiers::CONTROL | Modifiers::META, Code::KeyV),
            (Modifiers::ALT, Code::F5),
            (Modifiers::SHIFT, Code::Digit9),
        ] {
            let s = crate::hotkey::format_hotkey(mods, code);
            assert_eq!(parse_hotkey(&s).unwrap(), (mods, code), "{s}");
        }
    }
}
