//! 片段新增/编辑弹框（sheet）：名称/关键词同一行、多行内容、「插入变量」
//! 菜单（剪切板/日期/光标，与 [`crate::expand::vars`] 常量集一致）与
//! 保存/取消（trim 校验非空，失败弹窗不关 sheet）。
//!
//! ## 生命周期
//!
//! 每次 [`show`] 新建 sheet 窗口与控制器，用后不回收（`std::mem::forget`
//! 泄漏）：片段的新增/编辑是低频人工操作，单窗口泄漏量可忽略；复用则需
//! 处理 `endSheet:` 异步动画期间的窗口回收与三字段复位，复杂度不成比例。
//! 控制器同时被其按钮/菜单项 target 持有（AppKit retain target），forget
//! 后由 ObjC 引用计数兜底，不会出现悬垂。
//!
//! ## 「插入变量」菜单
//!
//! objc2-app-kit 0.3.2 已暴露 `NSMenu::popUpMenuPositioningItem_atLocation_inView`
//! （即 `popUpMenu:positioning:at:in:`），直接用它弹出独立菜单，无需退回
//! `NSPopUpButton`；菜单项 ivars 携带要插入的变量串（[`VarMenuItem`]）。
//!
//! ## 内容框占位提示
//!
//! 采用次级标签方案（内容区下方的变量说明行），不用「初始文案 + 首次
//! 编辑清空」——后者会把占位文案混入真实内容，需要额外的清空状态机。

use std::cell::Cell;

use objc2::define_class;
use objc2::msg_send;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::runtime::NSObject;
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSBackingStoreType, NSBezelStyle, NSButton, NSColor, NSFont, NSMenu, NSMenuItem, NSScrollView,
    NSScrollerStyle, NSTextField, NSTextView, NSWindow, NSWindowStyleMask,
};
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use objc2_foundation::{NSRange, NSString};

use super::Globals;
use crate::expand::vars;
use crate::model::SnippetItem;

/// 窗口尺寸。
const SHEET_WIDTH: f64 = 460.0;
const SHEET_HEIGHT: f64 = 360.0;

fn rect(x: f64, y: f64, w: f64, h: f64) -> CGRect {
    CGRect::new(CGPoint::new(x, y), CGSize::new(w, h))
}

/// 变量 token 统一拼装：`{{<inner>}}`。inner 一律取自 [`vars`] 常量
/// （规约：禁止手写魔法串，与 expand.rs 的解析严格同源）。
fn var_token(inner: &str) -> String {
    format!("{{{{{inner}}}}}")
}

/// 日期变量 token：`{{date:<fmt>}}`。
fn date_var_token(fmt: &str) -> String {
    var_token(&format!("{}{fmt}", vars::DATE_PREFIX))
}

/// 「插入变量」菜单项的 ivars：携带要插入的变量串。
pub struct VarMenuItemIvars {
    var_str: String,
}

define_class!(
    // SAFETY: NSMenuItem 无子类化约束；本类型不实现 Drop。
    #[unsafe(super(NSMenuItem))]
    #[thread_kind = MainThreadOnly]
    #[name = "PastelyVarMenuItem"]
    #[ivars = VarMenuItemIvars]
    pub struct VarMenuItem;
);

/// Sheet 控制器的 ivars：字段句柄 + Globals + 编辑目标 id。
pub struct SnippetSheetIvars {
    globals: *const Globals,
    /// 挂 sheet 的父窗口（设置窗口，进程生命周期泄漏单例）。
    parent_window: *const NSWindow,
    name_field: Retained<NSTextField>,
    keyword_field: Retained<NSTextField>,
    content_view: Retained<NSTextView>,
    sheet_window: Retained<NSWindow>,
    /// 编辑模式的片段 id（None = 新增）。
    edit_id: Cell<Option<i64>>,
}

define_class!(
    // SAFETY: NSObject 无子类化约束；本类型不实现 Drop。
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "PastelySnippetSheetController"]
    #[ivars = SnippetSheetIvars]
    pub struct SnippetSheetController;

    impl SnippetSheetController {
        /// 「插入变量…」按钮：弹出变量菜单（见模块注释，NSMenu popUp 方案）。
        #[unsafe(method(showVarMenu:))]
        fn show_var_menu(&self, sender: Option<&NSButton>) {
            let Some(button) = sender else {
                return;
            };
            let menu = build_var_menu(MainThreadMarker::new().expect("main thread"), self);
            // 菜单弹出在按钮上方（底左坐标：按钮顶边 + 2pt）
            menu.popUpMenuPositioningItem_atLocation_inView(
                None,
                CGPoint::new(0.0, button.bounds().size.height + 2.0),
                Some(button),
            );
        }

        /// 菜单项 action：把 sender ivars 携带的变量串插入内容框。
        #[unsafe(method(insertVariable:))]
        fn insert_variable(&self, sender: Option<&VarMenuItem>) {
            let Some(item) = sender else {
                return;
            };
            let var_str = item.ivars().var_str.clone();
            let ivars = self.ivars();
            let tv = &ivars.content_view;
            let current = tv.string().to_string();

            // 聚焦判定：内容框是 firstResponder 才信任其 selectedRange，
            // 否则视为未聚焦 → 末尾追加
            let focused = tv.window().is_some_and(|w| {
                w.firstResponder().is_some_and(|responder| {
                    std::ptr::eq(
                        Retained::as_ptr(&responder).cast::<objc2::runtime::AnyObject>(),
                        Retained::as_ptr(tv).cast::<objc2::runtime::AnyObject>(),
                    )
                })
            });
            // selectedRange.location 为 UTF-16 偏移，须先转 char 偏移再拼装
            //（否则含 emoji/增补平面字符的文本会插错位）
            let utf16_loc = if focused {
                tv.selectedRange().location
            } else {
                current.encode_utf16().count()
            };
            let char_loc = utf16_offset_to_char_index(&current, utf16_loc);
            let chars: Vec<char> = current.chars().collect();
            let char_loc = char_loc.min(chars.len());
            let mut new_text: String = chars[..char_loc].iter().collect();
            new_text.push_str(&var_str);
            new_text.extend(chars[char_loc..].iter());

            tv.setString(&NSString::from_str(&new_text));
            // 插入点移到新内容之后（变量 token 均为 ASCII：char 数 == UTF-16 数）
            let after = char_index_to_utf16_offset(&new_text, char_loc)
                + var_str.encode_utf16().count();
            tv.setSelectedRange(NSRange::new(after, 0));
            // 焦点回内容框（deref 强转 NSResponder 与既有 makeFirstResponder 用法一致）
            if let Some(w) = tv.window() {
                w.makeFirstResponder(Some(tv));
            }
        }

        /// 「保存」（回车）：trim 校验三项非空 → upsert → 刷新 Tab → endSheet。
        /// 校验/入库失败弹窗且不关 sheet。
        #[unsafe(method(saveSnippet:))]
        fn save_snippet(&self, _sender: Option<&AnyObject>) {
            let ivars = self.ivars();
            let globals = unsafe { &*ivars.globals };
            let name = ivars.name_field.stringValue().to_string();
            let keyword = ivars.keyword_field.stringValue().to_string();
            let content = ivars.content_view.string().to_string();
            if name.trim().is_empty() || keyword.trim().is_empty() || content.trim().is_empty() {
                super::settings::show_error("片段不完整", "名称、关键词与内容均不能为空。");
                return;
            }
            let result = {
                let store = globals.store.lock().expect("store poisoned");
                store.upsert_snippet(
                    ivars.edit_id.get(),
                    name.trim(),
                    keyword.trim(),
                    &content,
                    crate::model::now_ms(),
                )
            };
            match result {
                Ok(id) => tracing::info!("片段已保存 id={id} name={name:?} keyword={keyword:?}"),
                Err(e) => {
                    super::settings::show_error("保存片段失败", &format!("{e:#}"));
                    return;
                }
            }
            super::snippet_tab::refresh(globals);
            self.end_sheet();
        }

        /// 「取消」（Esc）：直接关 sheet。
        #[unsafe(method(cancelSnippet:))]
        fn cancel_snippet(&self, _sender: Option<&AnyObject>) {
            self.end_sheet();
        }
    }
);

impl SnippetSheetController {
    /// 结束 sheet（保存成功 / 取消共用）：`NSWindow.endSheet:` + orderOut 兜底。
    fn end_sheet(&self) {
        let parent = unsafe { &*self.ivars().parent_window };
        parent.endSheet(&self.ivars().sheet_window);
        self.ivars().sheet_window.orderOut(None);
    }
}

/// 构建「插入变量」菜单：一级 `剪切板内容` / `日期 ▸`（子菜单）/ `光标`；
/// 菜单项 ivars 携带变量串（与 [`vars`] 常量集一致）。
fn build_var_menu(mtm: MainThreadMarker, controller: &SnippetSheetController) -> Retained<NSMenu> {
    unsafe {
        let menu = NSMenu::new(mtm);
        let target: &objc2::runtime::AnyObject = controller;
        let add_item = |dst: &NSMenu, title: &str, var_str: String| {
            let title_ns = NSString::from_str(title);
            let empty = NSString::from_str("");
            let alloc = VarMenuItem::alloc(mtm).set_ivars(VarMenuItemIvars { var_str });
            let item: Retained<VarMenuItem> = msg_send![
                super(alloc),
                initWithTitle: &*title_ns,
                action: objc2::sel!(insertVariable:),
                keyEquivalent: &*empty
            ];
            item.setTarget(Some(target));
            dst.addItem(&item);
        };

        add_item(&menu, "剪切板内容", var_token(vars::CLIPBOARD));

        // 日期子菜单：四项格式与 expand.rs 支持集一一对应
        let date_menu = NSMenu::new(mtm);
        add_item(
            &date_menu,
            vars::DATE_FMT_YMD,
            date_var_token(vars::DATE_FMT_YMD),
        );
        add_item(
            &date_menu,
            vars::DATE_FMT_YMD_HMS,
            date_var_token(vars::DATE_FMT_YMD_HMS),
        );
        add_item(
            &date_menu,
            "时间戳（秒）",
            date_var_token(vars::DATE_FMT_TS_S),
        );
        add_item(
            &date_menu,
            "时间戳（毫秒）",
            date_var_token(vars::DATE_FMT_TS_MS),
        );
        let date_item = NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(mtm),
            &NSString::from_str("日期"),
            None,
            &NSString::from_str(""),
        );
        date_item.setSubmenu(Some(&date_menu));
        menu.addItem(&date_item);

        add_item(&menu, "光标", var_token(vars::CURSOR));
        menu
    }
}

/// 打开新增/编辑 sheet（挂到 `parent`，通常为设置窗口；生命周期见模块注释）。
pub fn show(globals: &'static Globals, parent: &NSWindow, edit: Option<SnippetItem>) {
    let mtm = MainThreadMarker::new().expect("main thread");
    unsafe {
        let (title, edit_id) = match &edit {
            Some(snippet) => (format!("编辑片段「{}」", snippet.name), Some(snippet.id)),
            None => ("新增片段".to_string(), None),
        };
        let content = rect(0.0, 0.0, SHEET_WIDTH, SHEET_HEIGHT);
        let mask = NSWindowStyleMask::Titled | NSWindowStyleMask::Closable;
        let window = NSWindow::initWithContentRect_styleMask_backing_defer(
            NSWindow::alloc(mtm),
            content,
            mask,
            NSBackingStoreType::Buffered,
            false,
        );
        window.setTitle(&NSString::from_str(&title));
        window.setReleasedWhenClosed(false);

        // 背景底（与设置窗口同款：26+ 玻璃 / 老系统磨砂）
        let backdrop = super::make_backdrop(mtm, content, 0.0);
        if let Some(cv) = window.contentView() {
            cv.addSubview(&backdrop);
        }

        let label = |text: &str, x: f64, y: f64, w: f64| {
            let l = NSTextField::labelWithString(&NSString::from_str(text), mtm);
            l.setFont(Some(&NSFont::systemFontOfSize(12.0)));
            l.setTextColor(Some(&NSColor::labelColor()));
            l.setFrame(rect(x, y, w, 16.0));
            l
        };
        let field = |placeholder: &str, x: f64, y: f64, w: f64| {
            let f = NSTextField::initWithFrame(NSTextField::alloc(mtm), rect(x, y, w, 24.0));
            f.setPlaceholderString(Some(&NSString::from_str(placeholder)));
            f
        };

        // 第一行：名称 + 关键词（左右各半，带标签）
        let row_y = SHEET_HEIGHT - 16.0 - 24.0;
        let name_label = label("名称", 16.0, row_y + 5.0, 36.0);
        let name_field = field("如：工作签名", 56.0, row_y, 140.0);
        let keyword_label = label("关键词", 208.0, row_y + 5.0, 44.0);
        let keyword_field = field("如：sig", 256.0, row_y, SHEET_WIDTH - 256.0 - 16.0);

        // 内容：标签 + 多行纯文本（scrollable）
        let content_label = label("内容", 16.0, row_y - 32.0, 40.0);
        let scroll_h = row_y - 32.0 - 16.0 - 96.0;
        let scroll = NSScrollView::initWithFrame(
            NSScrollView::alloc(mtm),
            rect(16.0, 96.0, SHEET_WIDTH - 32.0, scroll_h),
        );
        scroll.setHasVerticalScroller(true);
        scroll.setScrollerStyle(NSScrollerStyle::Overlay);
        scroll.setDrawsBackground(false);
        let content_view = NSTextView::initWithFrame(
            NSTextView::alloc(mtm),
            rect(0.0, 0.0, SHEET_WIDTH - 32.0, scroll_h),
        );
        content_view.setRichText(false);
        content_view.setFont(Some(&NSFont::systemFontOfSize(13.0)));
        content_view.setTextColor(Some(&NSColor::labelColor()));
        scroll.setDocumentView(Some(&content_view));

        // 变量行：「插入变量…」按钮 + 次级说明标签（占位提示采用次级标签
        // 方案，见模块注释）
        let insert_btn =
            NSButton::initWithFrame(NSButton::alloc(mtm), rect(16.0, 60.0, 92.0, 24.0));
        insert_btn.setTitle(&NSString::from_str("插入变量…"));
        insert_btn.setBezelStyle(NSBezelStyle::Push);
        insert_btn.setFont(Some(&NSFont::systemFontOfSize(11.0)));
        let var_hint = NSTextField::labelWithString(
            &NSString::from_str("支持 {{clipboard}}、{{date:…}}、{{cursor}} 变量"),
            mtm,
        );
        var_hint.setFont(Some(&NSFont::systemFontOfSize(10.0)));
        var_hint.setTextColor(Some(&NSColor::secondaryLabelColor()));
        var_hint.setFrame(rect(116.0, 65.0, SHEET_WIDTH - 132.0, 14.0));

        // 底部：保存（默认高亮，回车）/ 取消（Esc）
        let save_btn = NSButton::initWithFrame(
            NSButton::alloc(mtm),
            rect(SHEET_WIDTH - 16.0 - 80.0, 14.0, 80.0, 26.0),
        );
        save_btn.setTitle(&NSString::from_str("保存"));
        save_btn.setBezelStyle(NSBezelStyle::Push);
        save_btn.setKeyEquivalent(&NSString::from_str("\r"));
        let cancel_btn = NSButton::initWithFrame(
            NSButton::alloc(mtm),
            rect(SHEET_WIDTH - 16.0 - 80.0 - 8.0 - 80.0, 14.0, 80.0, 26.0),
        );
        cancel_btn.setTitle(&NSString::from_str("取消"));
        cancel_btn.setBezelStyle(NSBezelStyle::Push);
        cancel_btn.setKeyEquivalent(&NSString::from_str("\u{1b}"));

        if let Some(cv) = window.contentView() {
            cv.addSubview(&name_label);
            cv.addSubview(&name_field);
            cv.addSubview(&keyword_label);
            cv.addSubview(&keyword_field);
            cv.addSubview(&content_label);
            cv.addSubview(&scroll);
            cv.addSubview(&insert_btn);
            cv.addSubview(&var_hint);
            cv.addSubview(&save_btn);
            cv.addSubview(&cancel_btn);
        }

        // 控制器（ivars 持字段句柄；target 接三个按钮/菜单 action）
        let controller_alloc = SnippetSheetController::alloc(mtm).set_ivars(SnippetSheetIvars {
            globals: globals as *const Globals,
            parent_window: parent as *const NSWindow,
            name_field: name_field.clone(),
            keyword_field: keyword_field.clone(),
            content_view: content_view.clone(),
            sheet_window: window.clone(),
            edit_id: Cell::new(edit_id),
        });
        let controller: Retained<SnippetSheetController> = msg_send![super(controller_alloc), init];
        let target: &objc2::runtime::AnyObject = &controller;
        insert_btn.setTarget(Some(target));
        insert_btn.setAction(Some(objc2::sel!(showVarMenu:)));
        save_btn.setTarget(Some(target));
        save_btn.setAction(Some(objc2::sel!(saveSnippet:)));
        cancel_btn.setTarget(Some(target));
        cancel_btn.setAction(Some(objc2::sel!(cancelSnippet:)));

        // 编辑模式回填三个字段
        if let Some(snippet) = &edit {
            name_field.setStringValue(&NSString::from_str(&snippet.name));
            keyword_field.setStringValue(&NSString::from_str(&snippet.keyword));
            content_view.setString(&NSString::from_str(&snippet.content));
        }

        // 挂 sheet（completionHandler 传 None：结束动作由保存/取消显式 endSheet:）
        parent.beginSheet_completionHandler(&window, None);
        window.makeFirstResponder(Some(&name_field));

        // 刻意泄漏控制器（连带 ivars 里的窗口/字段句柄；见模块注释）
        std::mem::forget(controller);
    }
}

/// UTF-16 偏移 → char 偏移（超出文本长度时取末尾）。
fn utf16_offset_to_char_index(s: &str, utf16_offset: usize) -> usize {
    let mut seen = 0usize;
    for (ci, ch) in s.chars().enumerate() {
        if seen >= utf16_offset {
            return ci;
        }
        seen += ch.len_utf16();
    }
    s.chars().count()
}

/// char 偏移 → UTF-16 偏移。
fn char_index_to_utf16_offset(s: &str, char_index: usize) -> usize {
    s.chars().take(char_index).map(char::len_utf16).sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::expand::expand;

    #[test]
    fn var_token_format_matches_expand_parser() {
        // token 拼装结果必须能被 expand 命中（同源验证）
        let now = crate::model::now_ms();
        assert_eq!(
            expand(&var_token(vars::CLIPBOARD), now, Some("v")).text,
            "v"
        );
        assert_eq!(
            expand(&date_var_token(vars::DATE_FMT_YMD), now, None).text,
            expand("{{date:YYYY-MM-DD}}", now, None).text
        );
        assert_eq!(
            expand(&date_var_token(vars::DATE_FMT_TS_MS), now, None).text,
            now.to_string()
        );
        assert_eq!(
            expand(&var_token(vars::CURSOR), now, None).cursor_char,
            Some(0)
        );
    }

    #[test]
    fn utf16_to_char_index_ascii_and_astral() {
        assert_eq!(utf16_offset_to_char_index("hello", 2), 2);
        // 😀 占 2 个 UTF-16 单元：偏移 2 落在其后 → char 1
        assert_eq!(utf16_offset_to_char_index("😀ab", 2), 1);
        assert_eq!(utf16_offset_to_char_index("😀ab", 3), 2);
        // 超界取末尾
        assert_eq!(utf16_offset_to_char_index("ab", 99), 2);
    }

    #[test]
    fn char_index_to_utf16_offset_roundtrip() {
        let s = "a中😀b"; // UTF-16 单元数：1 + 1 + 2 + 1 = 5
        for ci in 0..=s.chars().count() {
            let u16_off = char_index_to_utf16_offset(s, ci);
            assert_eq!(utf16_offset_to_char_index(s, u16_off), ci);
        }
        assert_eq!(char_index_to_utf16_offset(s, 2), 2); // a(1) + 中(1)
        assert_eq!(char_index_to_utf16_offset(s, 3), 4); // + 😀(2)
        assert_eq!(char_index_to_utf16_offset(s, 4), 5); // + b(1)
    }
}
