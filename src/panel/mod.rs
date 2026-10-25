//! 弹窗面板与装配中枢。
//!
//! 架构（主线程单访）：
//! - [`Globals`]：泄漏一次的全局状态（store/config/watcher/hotkey/panel/runtime 句柄）
//! - [`AppController`]：NSObject 子类，承载全部 NSTimer tick、状态栏菜单动作与
//!   window/搜索框 delegate 协议
//! - [`PanelController`]：非激活 NSPanel，左右分栏——左列搜索框 + 滚动列表、
//!   右列预览区；真液态玻璃底（NSGlassEffectView）+ 深浅色自适应
//! - [`PanelRuntime`]：field editor 键盘回调访问的列表选择状态
//!
//! 非激活面板保证唤起时原应用不失活——粘贴回填 `CGEventPost` 直接落回原应用。

pub mod list_view;
pub mod paste_back;
pub mod search_field;
pub mod settings;

use std::cell::RefCell;
use std::path::PathBuf;

use objc2::define_class;
use objc2::msg_send;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject, ProtocolObject};
use objc2::{AnyThread, DefinedClass, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSAlert, NSAlertSecondButtonReturn, NSAppearance, NSAppearanceCustomization, NSAppearanceName,
    NSAppearanceNameAqua, NSAppearanceNameDarkAqua, NSApplication, NSApplicationActivationPolicy,
    NSApplicationDelegate, NSBackingStoreType, NSBezelStyle, NSBox, NSBoxType, NSButton, NSColor,
    NSControlTextEditingDelegate, NSEvent, NSFont, NSGlassEffectView, NSGlassEffectViewStyle,
    NSImage, NSImageView, NSMenu, NSMenuItem, NSPanel, NSScreen, NSScrollView, NSScrollerStyle,
    NSSearchField, NSSearchFieldDelegate, NSStatusBar, NSTableCellView, NSTextAlignment,
    NSTextField, NSTextFieldDelegate, NSTextView, NSView, NSWindow, NSWindowDelegate,
    NSWindowStyleMask, NSWorkspace,
};
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use objc2_foundation::{NSData, NSNotification, NSObjectProtocol, NSString};

use crate::clipboard::{self, WatcherState};
use crate::config::PasteMode;
use crate::hotkey::HotkeyManager;
use crate::model::{ClipKind, SearchableItem};
use crate::search::fuzzy_filter;
use crate::store::ClipStore;

use list_view::{build_row, ROW_HEIGHT};
use search_field::PanelFieldEditor;

/// 面板宽度（pt）。
pub const PANEL_WIDTH: f64 = 660.0;
/// 左栏（搜索框 + 列表）宽度（pt）；右栏预览区宽 = PANEL_WIDTH - LIST_WIDTH - 1。
pub const LIST_WIDTH: f64 = 320.0;
/// 搜索栏高度（pt，面板顶部区域）。
pub const SEARCH_BAR_HEIGHT: f64 = 40.0;
/// 上下边距（pt）。
pub const PANEL_PADDING: f64 = 8.0;
/// AX 未授权横幅高度（pt）。
pub const BANNER_HEIGHT: f64 = 28.0;

/// 列表全量重建上限（行）：滚动列表后不再截断到 `max_list_rows`
/// （后者仅决定面板高度）；200 行 × 3 子视图 ≈ 600 view 全建尚可，
/// 实测卡顿再做虚拟化（v1.1）。数字键 1-9 仍只标注前 9 行。
const LIST_REBUILD_LIMIT: usize = 200;

/// `NSStatusItemVariableLength`（AppKit 语义：状态栏 item 宽度随内容自适应）。
/// objc2-app-kit 0.3.2 未导出该常量，按 Apple 文档取值 -1.0。
const NS_STATUS_ITEM_VARIABLE_LENGTH: f64 = -1.0;

fn rect(x: f64, y: f64, w: f64, h: f64) -> CGRect {
    CGRect::new(CGPoint::new(x, y), CGSize::new(w, h))
}

/// 泄漏一次的全局状态（全部主线程访问）。
pub struct Globals {
    pub store: std::sync::Arc<std::sync::Mutex<ClipStore>>,
    /// 当前生效配置（设置窗口保存后原地更新，读点即时生效）。
    pub config: RefCell<crate::config::Config>,
    pub config_path: PathBuf,
    pub watcher: RefCell<WatcherState>,
    pub hotkey: RefCell<HotkeyManager>,
    pub panel: RefCell<Option<&'static PanelController>>,
    /// 装配层泄漏的 `Box<PanelRuntime>`。
    pub runtime: RefCell<Option<*mut PanelRuntime>>,
    pub ax_hint_shown: RefCell<bool>,
    /// AX 授权弹框只触发一次（confirm 首次未授权时）。
    pub ax_prompt_shown: RefCell<bool>,
    /// 设置窗口单例（首次打开时创建并泄漏）。
    pub settings: RefCell<Option<&'static settings::SettingsWindow>>,
}

impl Globals {
    pub fn panel(&self) -> Option<&'static PanelController> {
        *self.panel.borrow()
    }

    fn runtime(&self) -> Option<&'static mut PanelRuntime> {
        let ptr = (*self.runtime.borrow())?;
        Some(unsafe { &mut *ptr })
    }

    /// 查询变化：重算过滤结果并刷新列表（全量重建上限 200 行，主线程亚毫秒）。
    pub fn refresh_search(&self, query: &str) {
        let Some(rt) = self.runtime() else {
            return;
        };
        let items = {
            let store = self.store.lock().expect("store poisoned");
            store.all_searchable().unwrap_or_default()
        };
        rt.filtered = fuzzy_filter(&items, query);
        // 滚动列表：不再截断到 max_list_rows（它仅决定面板高度）
        rt.filtered.truncate(LIST_REBUILD_LIMIT);
        rt.selected = 0;
        if let Some(p) = self.panel() {
            p.update_list(&rt.filtered, rt.selected, &|id| {
                let store = self.store.lock().expect("store poisoned");
                store.image_bytes(id).unwrap_or(None)
            });
        }
    }

    /// 键盘移动选择。
    pub fn move_selection(&self, delta: i32) {
        let Some(rt) = self.runtime() else {
            return;
        };
        if rt.filtered.is_empty() {
            return;
        }
        let len = rt.filtered.len() as i32;
        let next = ((rt.selected as i32) + delta).rem_euclid(len);
        self.move_to(next as usize);
    }

    /// 鼠标点击选择行（仅改选中并刷新，不 confirm、不关面板）。
    pub fn move_to(&self, index: usize) {
        let Some(rt) = self.runtime() else {
            return;
        };
        if index >= rt.filtered.len() {
            return;
        }
        rt.selected = index;
        if let Some(p) = self.panel() {
            p.update_list(&rt.filtered, rt.selected, &|id| {
                let store = self.store.lock().expect("store poisoned");
                store.image_bytes(id).unwrap_or(None)
            });
        }
    }

    /// 数字键直达：选中并确认第 `index` 行（0-based）。
    ///
    /// 越界/空列表返回 false（field editor 据此把按键透传给系统）。
    pub fn confirm_at(&self, index: usize) -> bool {
        let Some(rt) = self.runtime() else {
            return false;
        };
        if index >= rt.filtered.len() {
            return false;
        }
        rt.selected = index;
        self.confirm();
        true
    }

    /// 确认当前选择：写剪切板 → 隐藏 → 防回环 → （auto 且已授权）模拟 ⌘V。
    pub fn confirm(&self) {
        let Some(rt) = self.runtime() else {
            return;
        };
        let Some((item, _)) = rt.filtered.get(rt.selected) else {
            return;
        };
        match item.kind {
            ClipKind::Text => clipboard::pasteboard::write_text(&item.full_text),
            ClipKind::Image => {
                let png = {
                    let store = self.store.lock().expect("store poisoned");
                    store.image_bytes(item.id).unwrap_or(None)
                };
                match png {
                    Some(bytes) => clipboard::pasteboard::write_image_png(&bytes),
                    None => tracing::error!("图片条目 {} 的 BLOB 读取失败", item.id),
                }
            }
        }
        // 防自回环：把基线推进到刚才自己写出的 changeCount
        self.watcher.borrow_mut().last_count = clipboard::pasteboard::change_count();

        if let Some(p) = self.panel() {
            p.hide();
        }
        if matches!(self.config.borrow().paste.mode, PasteMode::Auto) {
            let mode = self.config.borrow().paste.mode;
            let mut trusted = paste_back::is_ax_trusted();
            // 首次未授权：触发系统授权弹框（AXIsProcessTrustedWithOptions prompt，
            // 仅此一次；之后经横幅按钮 / 系统设置授权，is_ax_trusted 即转正常路径）
            if !trusted && !*self.ax_prompt_shown.borrow() {
                *self.ax_prompt_shown.borrow_mut() = true;
                trusted = paste_back::request_ax_permission();
                tracing::info!("paste-back: 已触发系统辅助功能授权弹框，granted_now={trusted}");
            }
            tracing::info!("paste-back: ax_trusted={} mode={:?}", trusted, mode);
            if trusted {
                match paste_back::send_cmd_v() {
                    Ok(()) => tracing::info!("paste-back: Cmd+V 已投递（session tap）"),
                    Err(e) => tracing::error!("paste-back: 投递失败: {e:#}"),
                }
            } else {
                // 针对性提示：ad-hoc 签名每次重新打包 cdhash 变化，
                // TCC 按 designated requirement 匹配 → 旧授权对新 build 失效，
                // 系统设置里看似已开启但 AXIsProcessTrusted() 返回 false
                tracing::warn!(
                    "AX 授权检查未通过（is_ax_trusted=false），本次降级为仅复制。\
                     若系统设置中已开启：ad-hoc 签名在重新打包后需删除旧条目重新授权\
                     （系统设置中选中 Pastely 按 '-' 删除，再触发一次粘贴以弹出授权框）"
                );
                let mut hinted = self.ax_hint_shown.borrow_mut();
                if !*hinted {
                    *hinted = true;
                    tracing::info!(
                        "提示：系统设置 → 隐私与安全性 → 辅助功能 中勾选 Pastely 后可自动粘贴"
                    );
                }
            }
        }
    }

    pub fn hide_panel(&self) {
        if let Some(p) = self.panel() {
            p.hide();
        }
    }

    /// 唤起面板：清空搜索、刷新最近列表、聚焦搜索框、按权限刷新 AX 横幅。
    pub fn show_panel(&self) {
        let Some(p) = self.panel() else {
            return;
        };
        p.reset_input();
        self.refresh_search("");
        // 每次唤起刷新横幅状态（授权后自动消失）
        p.set_ax_banner_visible(!paste_back::is_ax_trusted());
        p.show();
    }

    pub fn toggle_panel(&self) {
        let Some(p) = self.panel() else {
            return;
        };
        if p.is_visible() {
            p.hide();
        } else {
            self.show_panel();
        }
    }
}

/// UI 运行时状态（field editor 键盘回调访问）。
pub struct PanelRuntime {
    /// 指向泄漏的 [`Globals`]。
    pub g: *const Globals,
    pub filtered: Vec<(SearchableItem, Vec<usize>)>,
    pub selected: usize,
}

impl PanelRuntime {
    fn globals(&self) -> &'static Globals {
        unsafe { &*self.g }
    }

    pub fn move_selection(&self, delta: i32) {
        self.globals().move_selection(delta);
    }

    pub fn confirm_current(&self) {
        self.globals().confirm();
    }

    /// 数字键直达：确认第 `index` 行；越界/空列表返回 false。
    pub fn confirm_at(&self, index: usize) -> bool {
        self.globals().confirm_at(index)
    }

    pub fn hide(&self) {
        self.globals().hide_panel();
    }
}

/// AppController 的 ivars（`field_editor` 经 RefCell 后置注入）。
pub struct AppControllerIvars {
    /// 泄漏的 [`Globals`]（进程生命周期）。
    pub globals: *const Globals,
    pub field_editor: RefCell<Option<Retained<PanelFieldEditor>>>,
}

define_class!(
    // SAFETY: NSObject 无子类化约束；本类型不实现 Drop。
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "ClipHubAppController"]
    #[ivars = AppControllerIvars]
    pub struct AppController;

    impl AppController {
        #[unsafe(method(tickWatch:))]
        fn tick_watch(&self, _timer: Option<&AnyObject>) {
            let g = self.ivars().globals();
            let new_item = {
                let mut watcher = g.watcher.borrow_mut();
                let mut item = None;
                clipboard::tick(&mut watcher, |it| item = Some(it));
                item
            };
            if let Some(item) = new_item {
                let id = {
                    let store = g.store.lock().expect("store poisoned");
                    match store.upsert(&item) {
                        Ok(id) => id,
                        Err(e) => {
                            tracing::error!("入库失败: {e}");
                            return;
                        }
                    }
                };
                tracing::debug!("记录条目 id={id} kind={:?}", item.kind);
                if g.panel().is_some_and(|p| p.is_visible()) {
                    let q = g.panel().unwrap().query();
                    g.refresh_search(&q);
                }
            }
        }

        #[unsafe(method(tickHotkey:))]
        fn tick_hotkey(&self, _timer: Option<&AnyObject>) {
            let g = self.ivars().globals();
            let pressed = g.hotkey.borrow().poll();
            if !pressed.is_empty() {
                g.toggle_panel();
            }
        }

        #[unsafe(method(tickTtl:))]
        fn tick_ttl(&self, _timer: Option<&AnyObject>) {
            let g = self.ivars().globals();
            let (ttl_days, max_items) = {
                let cfg = g.config.borrow();
                (cfg.storage.ttl_days, cfg.storage.max_items)
            };
            let (expired, evicted) = {
                let store = g.store.lock().expect("store poisoned");
                crate::ttl::reap_once(&store, ttl_days, max_items).unwrap_or((0, 0))
            };
            if expired + evicted > 0 {
                tracing::info!("清理：TTL 过期 {expired} 条，容量淘汰 {evicted} 条");
            }
        }

        #[unsafe(method(showPanel:))]
        fn show_panel(&self, _sender: Option<&AnyObject>) {
            self.ivars().globals().show_panel();
        }

        #[unsafe(method(openSettings:))]
        fn open_settings(&self, _sender: Option<&AnyObject>) {
            settings::show(self.ivars().globals(), self);
        }

        #[unsafe(method(saveSettings:))]
        fn save_settings(&self, _sender: Option<&AnyObject>) {
            settings::save(self.ivars().globals());
        }

        #[unsafe(method(closeSettings:))]
        fn close_settings(&self, _sender: Option<&AnyObject>) {
            settings::close(self.ivars().globals());
        }

        #[unsafe(method(togglePause:))]
        fn toggle_pause(&self, sender: Option<&NSMenuItem>) {
            let g = self.ivars().globals();
            let paused = {
                let mut w = g.watcher.borrow_mut();
                w.paused = !w.paused;
                w.paused
            };
            if let Some(item) = sender {
                item.setTitle(&NSString::from_str(if paused { "恢复记录" } else { "暂停记录" }));
            }
            tracing::info!("记录{}", if paused { "已暂停" } else { "已恢复" });
        }

        #[unsafe(method(clearHistory:))]
        fn clear_history(&self, _sender: Option<&AnyObject>) {
            let g = self.ivars().globals();
            let mtm = MainThreadMarker::new().expect("main thread");
            unsafe {
                let alert: Retained<NSAlert> = msg_send![NSAlert::alloc(mtm), init];
                alert.setMessageText(&NSString::from_str("清空剪切板历史"));
                alert.setInformativeText(&NSString::from_str(
                    "将删除全部历史条目（含图片），此操作不可撤销。",
                ));
                alert.addButtonWithTitle(&NSString::from_str("取消"));
                alert.addButtonWithTitle(&NSString::from_str("清空"));
                if alert.runModal() == NSAlertSecondButtonReturn {
                    g.store.lock().expect("store poisoned").clear().ok();
                    g.refresh_search("");
                    tracing::info!("已清空历史");
                }
            }
        }

        #[unsafe(method(revealConfig:))]
        fn reveal_config(&self, _sender: Option<&AnyObject>) {
            let g = self.ivars().globals();
            {
                let path = NSString::from_str(&g.config_path.to_string_lossy());
                let url = objc2_foundation::NSURL::fileURLWithPath_isDirectory_relativeToURL(
                    &path, false, None,
                );
                let urls = objc2_foundation::NSArray::from_retained_slice(&[url]);
                NSWorkspace::sharedWorkspace().activateFileViewerSelectingURLs(&urls);
            }
        }

        #[unsafe(method(openAccessibilitySettings:))]
        fn open_accessibility_settings(&self, _sender: Option<&AnyObject>) {
            // 直达 系统设置 → 隐私与安全性 → 辅助功能
            let url_str = NSString::from_str(
                "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility",
            );
            if let Some(url) = objc2_foundation::NSURL::URLWithString(&url_str) {
                let opened = NSWorkspace::sharedWorkspace().openURL(&url);
                tracing::info!("打开系统设置（辅助功能面板）: opened={opened}");
            } else {
                tracing::error!("系统设置 URL 构造失败");
            }
        }

        #[unsafe(method(quitApp:))]
        fn quit_app(&self, _sender: Option<&AnyObject>) {
            NSApplication::sharedApplication(MainThreadMarker::new().unwrap()).terminate(None);
        }
    }

    unsafe impl NSObjectProtocol for AppController {}

    unsafe impl NSApplicationDelegate for AppController {}

    unsafe impl NSWindowDelegate for AppController {
        #[unsafe(method(windowDidResignKey:))]
        fn window_did_resign_key(&self, _notification: &NSNotification) {
            self.ivars().globals().hide_panel();
        }

        #[unsafe(method(windowWillReturnFieldEditor:toObject:))]
        unsafe fn window_will_return_field_editor(
            &self,
            _sender: &NSWindow,
            client: &AnyObject,
        ) -> *mut AnyObject {
            // 仅搜索框走自定义 field editor（键盘路由核心）；
            // 预览 NSTextView 等其余 client 返回 null，走系统默认编辑器
            // （否则绑死搜索框的 editor 会破坏预览文本的鼠标选中行为）
            if client.downcast_ref::<NSSearchField>().is_none() {
                return std::ptr::null_mut();
            }
            match self.ivars().field_editor.borrow().as_ref() {
                Some(fe) => Retained::as_ptr(fe) as *mut AnyObject,
                None => std::ptr::null_mut(),
            }
        }
    }

    unsafe impl NSControlTextEditingDelegate for AppController {
        #[unsafe(method(controlTextDidChange:))]
        fn control_text_did_change(&self, _notification: &NSNotification) {
            let g = self.ivars().globals();
            if let Some(p) = g.panel() {
                let q = p.query();
                g.refresh_search(&q);
            }
        }
    }

    // 搜索框 delegate（NSSearchField::setDelegate）要求协议链
    // NSControlTextEditingDelegate → NSTextFieldDelegate → NSSearchFieldDelegate 全部声明；
    // 三个协议的方法均为 @optional，实际只需上方已实现的 controlTextDidChange:。
    unsafe impl NSTextFieldDelegate for AppController {}

    unsafe impl NSSearchFieldDelegate for AppController {}
);

impl AppControllerIvars {
    fn globals(&self) -> &'static Globals {
        unsafe { &*self.globals }
    }
}

impl AppController {
    /// 创建并把 `globals` 写入 ivars（`field_editor` 由 PanelController 注入）。
    pub fn create(globals: &'static Globals) -> Retained<Self> {
        let mtm = MainThreadMarker::new().expect("main thread");
        let this = Self::alloc(mtm).set_ivars(AppControllerIvars {
            globals,
            field_editor: RefCell::new(None),
        });
        unsafe { msg_send![super(this), init] }
    }
}

/// 面板控制器。
pub struct PanelController {
    pub panel: Retained<NSPanel>,
    pub search_field: Retained<NSSearchField>,
    /// 列表滚动容器（documentView 为 [`Self::list_container`]）。
    list_scroll: Retained<NSScrollView>,
    pub list_container: Retained<ListContainerView>,
    /// 右栏预览区（分栏布局）。
    pub preview_container: Retained<NSView>,
    /// AX 未授权横幅（底部覆盖，默认隐藏）。
    ax_banner: Retained<NSView>,
    rows: RefCell<Vec<Retained<NSTableCellView>>>,
}

/// 列表容器的 ivars（指向泄漏的 [`Globals`]）。
pub struct ListContainerIvars {
    pub globals: *const Globals,
}

define_class!(
    // SAFETY: NSView 无子类化约束；本类型不实现 Drop。
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "ClipHubListContainerView"]
    #[ivars = ListContainerIvars]
    pub struct ListContainerView;

    impl ListContainerView {
        /// 点击全部落在容器上（行内 label/缩略图不再分散命中目标）。
        #[unsafe(method(hitTest:))]
        unsafe fn hit_test(&self, point: CGPoint) -> *mut AnyObject {
            // 父类默认判定该视图层级是否包含该点；非 nil 则改返回自身
            let hit: *mut AnyObject = unsafe { msg_send![super(self), hitTest: point] };
            if hit.is_null() {
                std::ptr::null_mut()
            } else {
                // SAFETY: 与 &self 同址（同一活动对象），调用方仅瞬时持有
                self as *const Self as *mut AnyObject
            }
        }

        /// 单击选择行（move_to，不关面板）；双击确认（明确意图，始终允许）。
        #[unsafe(method(mouseUp:))]
        unsafe fn mouse_up(&self, event: &NSEvent) {
            let globals: &Globals = unsafe { &*self.ivars().globals };
            let local = self.convertPoint_fromView(event.locationInWindow(), None);
            let h = self.frame().size.height;
            if h <= 0.0 {
                return;
            }
            // Cocoa 底左坐标：行自顶部排布，row = (容器高 - y) / 行高
            let row = ((h - local.y) / ROW_HEIGHT).floor();
            if row < 0.0 {
                return;
            }
            let row = row as usize;
            if event.clickCount() >= 2 {
                globals.confirm_at(row);
            } else {
                globals.move_to(row);
            }
        }
    }
);

/// 列表行图片字节加载器（锁定 store 查 BLOB）。
pub type ImageLoader<'a> = &'a dyn Fn(i64) -> Option<Vec<u8>>;

impl PanelController {
    /// 创建非激活面板（左右分栏：搜索框 + 滚动列表 | 预览区）并挂 window delegate。
    ///
    /// 面板尺寸 `PANEL_WIDTH × (SEARCH_BAR_HEIGHT + rows×ROW_HEIGHT + 16)`，
    /// 默认 10 行 = 660×376。真液态玻璃底由铺满 contentView 的
    /// [`NSGlassEffectView`] 承担（Regular style，圆角 12，窗口本体透明）。
    ///
    /// # Safety
    ///
    /// - `runtime` 必须指向泄漏的 `Box<PanelRuntime>`（进程生命周期）
    /// - `globals` 必须是泄漏的 [`Globals`]（列表容器鼠标事件回读用）
    /// - 必须在主线程调用
    pub unsafe fn new(
        mtm: MainThreadMarker,
        max_list_rows: usize,
        runtime: *mut PanelRuntime,
        controller: &AppController,
        globals: &'static Globals,
    ) -> PanelController {
        unsafe {
            let rows_h = max_list_rows as f64 * ROW_HEIGHT;
            let height = SEARCH_BAR_HEIGHT + rows_h + PANEL_PADDING * 2.0;
            let content = rect(0.0, 0.0, PANEL_WIDTH, height);

            let mask = NSWindowStyleMask::Titled
                | NSWindowStyleMask::NonactivatingPanel
                | NSWindowStyleMask::FullSizeContentView;
            let panel = NSPanel::initWithContentRect_styleMask_backing_defer(
                NSPanel::alloc(mtm),
                content,
                mask,
                NSBackingStoreType::Buffered,
                false,
            );

            panel.setTitleVisibility(objc2_app_kit::NSWindowTitleVisibility::Hidden);
            panel.setTitlebarAppearsTransparent(true);
            panel.setBecomesKeyOnlyIfNeeded(true);
            panel.setFloatingPanel(true);
            panel.setLevel(objc2_app_kit::NSFloatingWindowLevel);
            panel.setReleasedWhenClosed(false);
            panel.setHidesOnDeactivate(false);
            // 玻璃面板：窗口本体透明，视觉全部由 glass effect view 承担
            panel.setOpaque(false);
            panel.setBackgroundColor(Some(&NSColor::clearColor()));
            panel.setDelegate(Some(ProtocolObject::from_ref(controller)));

            // 真液态玻璃底（macOS 26 NSGlassEffectView）：Regular style，
            // 自带 cornerRadius（无需手动 layer），材质随系统深浅色自适应
            let glass = NSGlassEffectView::initWithFrame(
                NSGlassEffectView::alloc(mtm),
                rect(0.0, 0.0, PANEL_WIDTH, height),
            );
            glass.setStyle(NSGlassEffectViewStyle::Regular);
            glass.setCornerRadius(12.0);
            if let Some(cv) = panel.contentView() {
                cv.addSubview(&glass);
            }

            // 左栏：搜索框（顶部）
            let search = NSSearchField::initWithFrame(
                NSSearchField::alloc(mtm),
                rect(12.0, height - 36.0, LIST_WIDTH - 24.0, 28.0),
            );
            search.setPlaceholderString(Some(&NSString::from_str("搜索剪切板历史…")));
            // 关键：delegate 指向 AppController，输入变化才会触发 controlTextDidChange:
            search.setDelegate(Some(ProtocolObject::from_ref(controller)));
            if let Some(cv) = panel.contentView() {
                cv.addSubview(&search);
            }

            // 左栏：列表滚动容器（documentView = list_container，内容自顶部排布；
            // 容器为子类：接管鼠标（单击选行 / 双击确认）
            let list_scroll = NSScrollView::initWithFrame(
                NSScrollView::alloc(mtm),
                rect(0.0, 0.0, LIST_WIDTH, rows_h + PANEL_PADDING * 2.0),
            );
            list_scroll.setHasVerticalScroller(true);
            list_scroll.setScrollerStyle(NSScrollerStyle::Overlay);
            list_scroll.setDrawsBackground(false);
            let list_alloc =
                ListContainerView::alloc(mtm).set_ivars(ListContainerIvars { globals });
            let list: Retained<ListContainerView> = msg_send![super(list_alloc), initWithFrame: rect(
                0.0,
                0.0,
                LIST_WIDTH,
                rows_h + PANEL_PADDING * 2.0
            )];
            list_scroll.setDocumentView(Some(&list));
            if let Some(cv) = panel.contentView() {
                cv.addSubview(&list_scroll);
            }

            // 右栏：预览区 + 1pt 竖直分隔线（NSBox separator 随深浅色自适应）
            let preview = NSView::initWithFrame(
                NSView::alloc(mtm),
                rect(
                    LIST_WIDTH + 1.0,
                    0.0,
                    PANEL_WIDTH - LIST_WIDTH - 1.0,
                    height,
                ),
            );
            if let Some(cv) = panel.contentView() {
                cv.addSubview(&preview);
            }
            let separator =
                NSBox::initWithFrame(NSBox::alloc(mtm), rect(LIST_WIDTH, 0.0, 1.0, height));
            separator.setBoxType(NSBoxType::Separator);
            if let Some(cv) = panel.contentView() {
                cv.addSubview(&separator);
            }

            // AX 未授权横幅：面板底部（最后添加 → 最上层），默认隐藏；
            // 动态 accent 淡底 + labelColor 文字，深浅色都协调
            let banner = NSView::initWithFrame(
                NSView::alloc(mtm),
                rect(0.0, 0.0, PANEL_WIDTH, BANNER_HEIGHT),
            );
            banner.setWantsLayer(true);
            if let Some(layer) = banner.layer() {
                let bg = NSColor::controlAccentColor().colorWithAlphaComponent(0.15);
                layer.setBackgroundColor(Some(&bg.CGColor()));
            }
            let hint = NSTextField::labelWithString(
                &NSString::from_str("需要辅助功能权限才能自动粘贴"),
                mtm,
            );
            hint.setFont(Some(&NSFont::systemFontOfSize(11.0)));
            hint.setTextColor(Some(&NSColor::labelColor()));
            hint.setFrame(rect(12.0, (BANNER_HEIGHT - 14.0) / 2.0, 236.0, 14.0));
            banner.addSubview(&hint);
            let open = NSButton::initWithFrame(
                NSButton::alloc(mtm),
                rect(PANEL_WIDTH - 124.0, 3.0, 112.0, 22.0),
            );
            open.setTitle(&NSString::from_str("打开系统设置"));
            open.setBezelStyle(NSBezelStyle::Push);
            let target: &AnyObject = controller;
            open.setTarget(Some(target));
            open.setAction(Some(objc2::sel!(openAccessibilitySettings:)));
            banner.addSubview(&open);
            banner.setHidden(true);
            if let Some(cv) = panel.contentView() {
                cv.addSubview(&banner);
            }

            // field editor：键盘路由核心（安装到 window delegate 的返回值）
            let editor = PanelFieldEditor::new(mtm, runtime);
            controller.ivars().field_editor.borrow_mut().replace(editor);

            PanelController {
                panel,
                search_field: search,
                list_scroll,
                list_container: list,
                preview_container: preview,
                ax_banner: banner,
                rows: RefCell::new(Vec::new()),
            }
        }
    }

    /// 屏幕居中显示 + 搜索框聚焦（焦点直接落在输入框）。
    pub fn show(&self) {
        let mtm = MainThreadMarker::new().expect("main thread");
        if let Some(screen) = NSScreen::mainScreen(mtm) {
            let frame = screen.visibleFrame();
            let size = self.panel.frame().size;
            let x = frame.origin.x + (frame.size.width - size.width) / 2.0;
            let y = frame.origin.y + (frame.size.height - size.height) / 2.0;
            self.panel.setFrame_display(
                CGRect::new(CGPoint::new(x, y), CGSize::new(size.width, size.height)),
                false,
            );
        }
        self.panel.makeKeyAndOrderFront(None);
        self.panel.makeFirstResponder(Some(&self.search_field));
    }

    /// AX 未授权横幅显隐（每次 show_panel 时按最新权限状态刷新）。
    pub fn set_ax_banner_visible(&self, visible: bool) {
        self.ax_banner.setHidden(!visible);
    }

    pub fn hide(&self) {
        self.panel.orderOut(None);
    }

    pub fn is_visible(&self) -> bool {
        self.panel.isVisible()
    }

    pub fn query(&self) -> String {
        self.search_field.stringValue().to_string()
    }

    /// 清空搜索框并聚焦。
    pub fn reset_input(&self) {
        self.search_field.setStringValue(&NSString::from_str(""));
        self.panel.makeFirstResponder(Some(&self.search_field));
    }

    /// 全量重建列表行并标注选中行；内容自顶部排布、选中行滚入可视区；
    /// 尾部同步刷新右栏预览。
    pub fn update_list(
        &self,
        rows: &[(SearchableItem, Vec<usize>)],
        selected: usize,
        load_image: ImageLoader,
    ) {
        unsafe {
            let mtm = MainThreadMarker::new().expect("main thread");
            for old in self.rows.borrow_mut().drain(..) {
                old.removeFromSuperview();
            }
            // documentView 高度 = max(实际行数×ROW_HEIGHT, 滚动区高度)：
            // 行少时短于视口也能从顶部排（非翻转坐标系下短 documentView 会沉底）
            let area_h = self.list_scroll.frame().size.height;
            let container_h = (rows.len() as f64 * ROW_HEIGHT).max(area_h);
            self.list_container
                .setFrame(rect(0.0, 0.0, LIST_WIDTH, container_h));
            let mut selected_png: Option<Vec<u8>> = None;
            for (i, (item, highlights)) in rows.iter().enumerate() {
                let png: Option<Vec<u8>> = match item.kind {
                    ClipKind::Image => load_image(item.id),
                    ClipKind::Text => None,
                };
                if i == selected {
                    selected_png = png.clone();
                }
                let row = build_row(
                    mtm,
                    LIST_WIDTH,
                    item,
                    highlights,
                    png.as_deref(),
                    i == selected,
                );
                // 行 y 从顶部排
                let y = container_h - (i as f64 + 1.0) * ROW_HEIGHT;
                row.setFrame(rect(0.0, y, LIST_WIDTH, ROW_HEIGHT));
                self.list_container.addSubview(&row);
                self.rows.borrow_mut().push(row);
            }
            // 键盘导航跟随：选中行滚入可视区（行少时无可滚区域，自然 no-op）
            if let Some(sel) = self.rows.borrow().get(selected) {
                let _: bool = sel.scrollRectToVisible(sel.bounds());
            }
            // 预览区跟随当前选中行（空列表/越界 → 占位提示）
            match rows.get(selected) {
                Some((item, _)) => self.update_preview(Some(item), selected_png.as_deref()),
                None => self.update_preview(None, None),
            }
        }
    }

    /// 渲染右栏预览：Text → 只读文本；Image → 居中缩放 + 尺寸/体积小字；空 → 占位提示。
    ///
    /// 文本颜色用系统动态色（labelColor/secondaryLabelColor），深浅色自动切换。
    pub fn update_preview(&self, item: Option<&SearchableItem>, png: Option<&[u8]>) {
        unsafe {
            let mtm = MainThreadMarker::new().expect("main thread");
            // 清空旧内容（subviews() 返回快照数组，边遍历边移除安全）
            let subs = self.preview_container.subviews();
            for sub in subs.iter() {
                sub.removeFromSuperview();
            }
            let pw = PANEL_WIDTH - LIST_WIDTH - 1.0;
            let ph = self.preview_container.frame().size.height;
            let Some(item) = item else {
                let hint =
                    NSTextField::labelWithString(&NSString::from_str("输入以过滤剪切板历史"), mtm);
                hint.setFont(Some(&NSFont::systemFontOfSize(13.0)));
                hint.setTextColor(Some(&NSColor::secondaryLabelColor()));
                hint.setAlignment(NSTextAlignment::Center);
                hint.setFrame(rect(8.0, ph / 2.0 - 10.0, pw - 16.0, 20.0));
                self.preview_container.addSubview(&hint);
                return;
            };
            match item.kind {
                ClipKind::Text => {
                    let tv =
                        NSTextView::initWithFrame(NSTextView::alloc(mtm), rect(0.0, 0.0, pw, ph));
                    tv.setEditable(false);
                    tv.setSelectable(true);
                    tv.setDrawsBackground(false);
                    tv.setRichText(false);
                    tv.setFont(Some(&NSFont::systemFontOfSize(13.0)));
                    tv.setTextColor(Some(&NSColor::labelColor()));
                    tv.setTextContainerInset(CGSize::new(8.0, 8.0));
                    tv.setString(&NSString::from_str(&item.full_text));
                    self.preview_container.addSubview(&tv);
                }
                ClipKind::Image => {
                    if let Some(bytes) = png {
                        let data = NSData::dataWithBytes_length(bytes.as_ptr().cast(), bytes.len());
                        if let Some(img) = NSImage::initWithData(NSImage::alloc(), &data) {
                            let iv = NSImageView::new(mtm);
                            iv.setImage(Some(&img));
                            iv.setFrame(rect(0.0, 24.0, pw, (ph - 24.0).max(0.0)));
                            iv.setImageScaling(
                                objc2_app_kit::NSImageScaling::ScaleProportionallyUpOrDown,
                            );
                            self.preview_container.addSubview(&iv);
                        }
                    }
                    let (w, h) = item.img_size.unwrap_or((0, 0));
                    let kb = png.map_or(0, |b| b.len() / 1024);
                    let meta = NSTextField::labelWithString(
                        &NSString::from_str(&format!("{w}×{h} · {kb} KB")),
                        mtm,
                    );
                    meta.setFont(Some(&NSFont::systemFontOfSize(11.0)));
                    meta.setTextColor(Some(&NSColor::secondaryLabelColor()));
                    meta.setAlignment(NSTextAlignment::Center);
                    meta.setFrame(rect(8.0, 4.0, pw - 16.0, 16.0));
                    self.preview_container.addSubview(&meta);
                }
            }
        }
    }
}

/// 按 `ui.theme` 固定面板外观（`auto` 不设置：动态色自动跟随系统深浅色）。
///
/// # Safety
///
/// 必须在主线程调用。
pub unsafe fn apply_panel_theme(panel: &NSPanel, theme: &str) {
    let name: Option<&'static NSAppearanceName> = match theme {
        "light" => Some(unsafe { NSAppearanceNameAqua }),
        "dark" => Some(unsafe { NSAppearanceNameDarkAqua }),
        _ => None,
    };
    if let Some(name) = name {
        if let Some(appearance) = NSAppearance::appearanceNamed(name) {
            // setAppearance 来自 NSAppearanceCustomization 协议（NSPanel 已声明 conforms）
            panel.setAppearance(Some(&appearance));
        }
    }
}

/// 状态栏与菜单装配。
///
/// # Safety
///
/// 必须在主线程调用。
pub unsafe fn build_status_bar() {
    unsafe {
        let mtm = MainThreadMarker::new().expect("main thread");
        let item =
            NSStatusBar::systemStatusBar().statusItemWithLength(NS_STATUS_ITEM_VARIABLE_LENGTH);
        // 原生 SF Symbol 模板图：自动适配菜单栏深浅色（emoji 不随主题）
        let symbol = NSImage::imageWithSystemSymbolName_accessibilityDescription(
            &NSString::from_str("clipboard.on"),
            None,
        )
        .or_else(|| {
            NSImage::imageWithSystemSymbolName_accessibilityDescription(
                &NSString::from_str("clipboard"),
                None,
            )
        });
        if let Some(img) = symbol {
            img.setTemplate(true);
            if let Some(button) = item.button(mtm) {
                button.setImage(Some(&img));
            }
        } else if let Some(button) = item.button(mtm) {
            // 兜底：符号不可用时退回文字
            button.setTitle(&NSString::from_str("📋"));
        }
        let menu = NSMenu::new(mtm);
        let add = |menu: &NSMenu, title: &str, sel: objc2::runtime::Sel, key: &str| {
            let mi = NSMenuItem::initWithTitle_action_keyEquivalent(
                NSMenuItem::alloc(mtm),
                &NSString::from_str(title),
                Some(sel),
                &NSString::from_str(key),
            );
            menu.addItem(&mi);
        };
        add(&menu, "显示 Pastely", objc2::sel!(showPanel:), "");
        add(&menu, "设置…", objc2::sel!(openSettings:), ",");
        add(&menu, "暂停记录", objc2::sel!(togglePause:), "");
        menu.addItem(&NSMenuItem::separatorItem(mtm));
        add(&menu, "清空历史…", objc2::sel!(clearHistory:), "");
        add(
            &menu,
            "在 Finder 中显示配置",
            objc2::sel!(revealConfig:),
            "",
        );
        menu.addItem(&NSMenuItem::separatorItem(mtm));
        add(&menu, "退出 Pastely", objc2::sel!(quitApp:), "q");
        item.setMenu(Some(&menu));
        // 状态栏 item 进程生命周期持有
        std::mem::forget(item);
    }
}

/// 无 Dock 图标。
pub fn set_accessory(app: &NSApplication) {
    app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
}
