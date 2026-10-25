//! 弹窗面板与装配中枢。
//!
//! 架构（主线程单访）：
//! - [`Globals`]：泄漏一次的全局状态（store/config/watcher/hotkey/panel/runtime 句柄）
//! - [`AppController`]：NSObject 子类，承载全部 NSTimer tick、状态栏菜单动作与
//!   window/搜索框 delegate 协议
//! - [`PanelController`]：非激活 NSPanel + 搜索框 + 列表容器
//! - [`PanelRuntime`]：field editor 键盘回调访问的列表选择状态
//!
//! 非激活面板保证唤起时原应用不失活——粘贴回填 `CGEventPost` 直接落回原应用。

pub mod list_view;
pub mod paste_back;
pub mod search_field;

use std::cell::RefCell;
use std::path::PathBuf;

use objc2::define_class;
use objc2::msg_send;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject, ProtocolObject};
use objc2::MainThreadOnly;
use objc2::{DefinedClass, MainThreadMarker};
use objc2_app_kit::{
    NSAlert, NSAlertSecondButtonReturn, NSApplication, NSApplicationActivationPolicy,
    NSApplicationDelegate, NSBackingStoreType, NSColor, NSControlTextEditingDelegate, NSMenu,
    NSMenuItem, NSPanel, NSScreen, NSSearchField, NSSearchFieldDelegate, NSStatusBar,
    NSTableCellView, NSTextFieldDelegate, NSView, NSWindow, NSWindowDelegate, NSWindowStyleMask,
    NSWorkspace,
};
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use objc2_foundation::{NSNotification, NSObjectProtocol, NSString};

use crate::clipboard::{self, WatcherState};
use crate::config::PasteMode;
use crate::hotkey::HotkeyManager;
use crate::model::{ClipKind, SearchableItem};
use crate::search::fuzzy_filter;
use crate::store::ClipStore;

use list_view::{build_row, ROW_HEIGHT};
use search_field::PanelFieldEditor;

/// 面板宽度（pt）。
pub const PANEL_WIDTH: f64 = 680.0;
/// 搜索栏高度（pt）。
pub const SEARCH_BAR_HEIGHT: f64 = 44.0;
/// 上下边距（pt）。
pub const PANEL_PADDING: f64 = 8.0;

/// `NSStatusItemVariableLength`（AppKit 语义：状态栏 item 宽度随内容自适应）。
/// objc2-app-kit 0.3.2 未导出该常量，按 Apple 文档取值 -1.0。
const NS_STATUS_ITEM_VARIABLE_LENGTH: f64 = -1.0;

fn rect(x: f64, y: f64, w: f64, h: f64) -> CGRect {
    CGRect::new(CGPoint::new(x, y), CGSize::new(w, h))
}

/// 泄漏一次的全局状态（全部主线程访问）。
pub struct Globals {
    pub store: std::sync::Arc<std::sync::Mutex<ClipStore>>,
    pub config: crate::config::Config,
    pub config_path: PathBuf,
    pub watcher: RefCell<WatcherState>,
    pub hotkey: RefCell<HotkeyManager>,
    pub panel: RefCell<Option<&'static PanelController>>,
    /// 装配层泄漏的 `Box<PanelRuntime>`。
    pub runtime: RefCell<Option<*mut PanelRuntime>>,
    pub ax_hint_shown: RefCell<bool>,
}

impl Globals {
    pub fn panel(&self) -> Option<&'static PanelController> {
        *self.panel.borrow()
    }

    fn runtime(&self) -> Option<&'static mut PanelRuntime> {
        let ptr = (*self.runtime.borrow())?;
        Some(unsafe { &mut *ptr })
    }

    /// 查询变化：重算过滤结果并刷新列表（≤1000 条，主线程亚毫秒）。
    pub fn refresh_search(&self, query: &str) {
        let Some(rt) = self.runtime() else {
            return;
        };
        let items = {
            let store = self.store.lock().expect("store poisoned");
            store.all_searchable().unwrap_or_default()
        };
        rt.filtered = fuzzy_filter(&items, query);
        rt.filtered.truncate(self.config.ui.max_list_rows);
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
        rt.selected = next as usize;
        if let Some(p) = self.panel() {
            p.update_list(&rt.filtered, rt.selected, &|id| {
                let store = self.store.lock().expect("store poisoned");
                store.image_bytes(id).unwrap_or(None)
            });
        }
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
        if matches!(self.config.paste.mode, PasteMode::Auto) {
            if paste_back::is_ax_trusted() {
                if let Err(e) = paste_back::send_cmd_v() {
                    tracing::error!("模拟 ⌘V 失败: {e:#}");
                }
            } else {
                tracing::warn!("辅助功能未授权，本次降级为仅复制");
                let mut hinted = self.ax_hint_shown.borrow_mut();
                if !*hinted {
                    *hinted = true;
                    tracing::info!(
                        "提示：系统设置 → 隐私与安全性 → 辅助功能 中勾选 ClipHub 后可自动粘贴"
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

    /// 唤起面板：清空搜索、刷新最近列表、聚焦搜索框。
    pub fn show_panel(&self) {
        let Some(p) = self.panel() else {
            return;
        };
        p.reset_input();
        self.refresh_search("");
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
            let (expired, evicted) = {
                let store = g.store.lock().expect("store poisoned");
                crate::ttl::reap_once(
                    &store,
                    g.config.storage.ttl_days,
                    g.config.storage.max_items,
                )
                .unwrap_or((0, 0))
            };
            if expired + evicted > 0 {
                tracing::info!("清理：TTL 过期 {expired} 条，容量淘汰 {evicted} 条");
            }
        }

        #[unsafe(method(showPanel:))]
        fn show_panel(&self, _sender: Option<&AnyObject>) {
            self.ivars().globals().show_panel();
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
            _client: &AnyObject,
        ) -> *mut AnyObject {
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
    pub list_container: Retained<NSView>,
    rows: RefCell<Vec<Retained<NSTableCellView>>>,
}

/// 列表行图片字节加载器（锁定 store 查 BLOB）。
pub type ImageLoader<'a> = &'a dyn Fn(i64) -> Option<Vec<u8>>;

impl PanelController {
    /// 创建非激活面板（搜索框 + 列表容器）并挂 window delegate。
    ///
    /// # Safety
    ///
    /// - `runtime` 必须指向泄漏的 `Box<PanelRuntime>`（进程生命周期）
    /// - 必须在主线程调用
    pub unsafe fn new(
        mtm: MainThreadMarker,
        max_list_rows: usize,
        runtime: *mut PanelRuntime,
        controller: &AppController,
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
            panel.setBackgroundColor(Some(&NSColor::windowBackgroundColor()));
            panel.setDelegate(Some(ProtocolObject::from_ref(controller)));

            let search = NSSearchField::initWithFrame(
                NSSearchField::alloc(mtm),
                rect(
                    12.0,
                    height - SEARCH_BAR_HEIGHT + 2.0,
                    PANEL_WIDTH - 24.0,
                    28.0,
                ),
            );
            search.setPlaceholderString(Some(&NSString::from_str("搜索剪切板历史…")));
            // 关键：delegate 指向 AppController，输入变化才会触发 controlTextDidChange:
            search.setDelegate(Some(ProtocolObject::from_ref(controller)));
            if let Some(cv) = panel.contentView() {
                cv.addSubview(&search);
            }

            let list = NSView::initWithFrame(
                NSView::alloc(mtm),
                rect(0.0, 0.0, PANEL_WIDTH, rows_h + PANEL_PADDING),
            );
            if let Some(cv) = panel.contentView() {
                cv.addSubview(&list);
            }

            // field editor：键盘路由核心（安装到 window delegate 的返回值）
            let editor = PanelFieldEditor::new(mtm, runtime);
            controller.ivars().field_editor.borrow_mut().replace(editor);

            PanelController {
                panel,
                search_field: search,
                list_container: list,
                rows: RefCell::new(Vec::new()),
            }
        }
    }

    /// 居中显示 + 搜索框聚焦（焦点直接落在输入框）。
    pub fn show(&self) {
        let mtm = MainThreadMarker::new().expect("main thread");
        if let Some(screen) = NSScreen::mainScreen(mtm) {
            let frame = screen.visibleFrame();
            let size = self.panel.frame().size;
            let x = frame.origin.x + (frame.size.width - size.width) / 2.0;
            let y = frame.origin.y + frame.size.height - size.height - 120.0;
            self.panel.setFrame_display(
                CGRect::new(CGPoint::new(x, y), CGSize::new(size.width, size.height)),
                false,
            );
        }
        self.panel.makeKeyAndOrderFront(None);
        self.panel.makeFirstResponder(Some(&self.search_field));
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

    /// 全量重建列表行并标注选中行。
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
            for (i, (item, highlights)) in rows.iter().enumerate() {
                let png: Option<Vec<u8>> = match item.kind {
                    ClipKind::Image => load_image(item.id),
                    ClipKind::Text => None,
                };
                let row = build_row(
                    mtm,
                    PANEL_WIDTH,
                    item,
                    highlights,
                    png.as_deref(),
                    i == selected,
                );
                let y = (rows.len() - 1 - i) as f64 * ROW_HEIGHT;
                row.setFrame(rect(0.0, y, PANEL_WIDTH, ROW_HEIGHT));
                self.list_container.addSubview(&row);
                self.rows.borrow_mut().push(row);
            }
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
        if let Some(button) = item.button(mtm) {
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
        add(&menu, "显示 ClipHub", objc2::sel!(showPanel:), "");
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
        add(&menu, "退出 ClipHub", objc2::sel!(quitApp:), "q");
        item.setMenu(Some(&menu));
        // 状态栏 item 进程生命周期持有
        std::mem::forget(item);
    }
}

/// 无 Dock 图标。
pub fn set_accessory(app: &NSApplication) {
    app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
}
