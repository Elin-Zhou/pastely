//! 设置窗口「快捷片段」Tab：片段列表（名称 / 关键词 / 内容单行预览 +
//! 行尾「编辑」「删除」）与右上角「＋ 新增片段」入口。
//!
//! 行按钮经 `#[ivars = ...]` 携带 snippet id（参考 settings.rs 的
//! `HotkeyRecorderIvars` 模式），target 统一指向 AppController 的
//! `editSnippet:` / `deleteSnippet:`；业务实现集中在模块级函数
//! （[`add`] / [`edit`] / [`delete`] / [`refresh`]），AppController 只做薄转发。

use std::cell::RefCell;

use objc2::define_class;
use objc2::msg_send;
use objc2::rc::Retained;
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSAlert, NSAlertSecondButtonReturn, NSBezelStyle, NSButton, NSColor, NSFont, NSLineBreakMode,
    NSScrollView, NSScrollerStyle, NSTextAlignment, NSView,
};
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use objc2_foundation::NSString;

use super::AppController;
use super::Globals;
use crate::model::SnippetItem;

/// Tab 内容区左右内边距（pt）。
const TAB_PADDING: f64 = 12.0;
/// 顶部说明 / 新增按钮行高（pt）。
const TOP_BAR_HEIGHT: f64 = 30.0;
/// 片段行高（pt）。
const SNIPPET_ROW_HEIGHT: f64 = 36.0;
/// 行内「编辑」「删除」按钮宽 / 高（pt）。
const ROW_BUTTON_WIDTH: f64 = 44.0;
const ROW_BUTTON_HEIGHT: f64 = 22.0;
/// 名称 / 关键词列宽（pt）。
const NAME_WIDTH: f64 = 92.0;
const KEYWORD_WIDTH: f64 = 76.0;

fn rect(x: f64, y: f64, w: f64, h: f64) -> CGRect {
    CGRect::new(CGPoint::new(x, y), CGSize::new(w, h))
}

/// 次级小标签工厂（Tab 内多处使用：说明小字 / 名称 / 关键词 / 预览）。
fn label(
    text: &NSString,
    size: f64,
    mtm: MainThreadMarker,
) -> Retained<objc2_app_kit::NSTextField> {
    let l = objc2_app_kit::NSTextField::labelWithString(text, mtm);
    l.setFont(Some(&NSFont::systemFontOfSize(size)));
    l
}

/// 行按钮的 ivars：携带目标 snippet id。
pub struct SnippetRowButtonIvars {
    snippet_id: i64,
}

define_class!(
    // SAFETY: NSButton 无子类化约束；本类型不实现 Drop。
    #[unsafe(super(NSButton))]
    #[thread_kind = MainThreadOnly]
    #[name = "PastelySnippetRowButton"]
    #[ivars = SnippetRowButtonIvars]
    pub struct SnippetRowButton;
);

/// Tab 根视图的 ivars：行容器（documentView）与 AppController 句柄，
/// 供 [`refresh`] 回读重建行。
struct SnippetTabIvars {
    rows_container: RefCell<Option<Retained<NSView>>>,
    controller: *const AppController,
}

define_class!(
    // SAFETY: NSView 无子类化约束；本类型不实现 Drop。
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "PastelySnippetTabView"]
    #[ivars = SnippetTabIvars]
    struct SnippetTabView;
);

impl SnippetTabView {
    /// 行容器（documentView）句柄。
    fn rows_container(&self) -> Option<Retained<NSView>> {
        self.ivars().rows_container.borrow().clone()
    }

    /// 装配层传入的 AppController（进程生命周期：delegate 被 NSApplication
    /// 持久 retain；与 mod.rs ivars 存 `*const Globals` 同一借用纪律）。
    fn controller(&self) -> &'static AppController {
        // SAFETY: 指针来自 build() 的装配参数，指向 NSApplication 持有的
        // delegate（进程生命周期），与本模块其余 ivars 指针同纪律
        unsafe { &*self.ivars().controller }
    }
}

/// 构建片段 Tab 根视图（供 settings.rs 装入 snippet_container；
/// 尺寸与 `tab_height()` 对齐，内部布局按该值计算）。
///
/// # Safety
///
/// 必须在主线程调用（mtm 保证）。
pub unsafe fn build(
    mtm: MainThreadMarker,
    _globals: &'static Globals,
    controller: &AppController,
) -> Retained<NSView> {
    unsafe {
        let width = super::settings::SETTINGS_WIDTH;
        let height = super::settings::tab_height();
        let root_alloc = SnippetTabView::alloc(mtm).set_ivars(SnippetTabIvars {
            rows_container: RefCell::new(None),
            controller,
        });
        let root: Retained<SnippetTabView> =
            msg_send![super(root_alloc), initWithFrame: rect(0.0, 0.0, width, height)];

        // 顶部：左侧说明小字 + 右侧「＋ 新增片段」
        let hint = label(&NSString::from_str("粘贴时输入关键词快速插入"), 11.0, mtm);
        hint.setTextColor(Some(&NSColor::secondaryLabelColor()));
        hint.setFrame(rect(TAB_PADDING, height - TOP_BAR_HEIGHT, 180.0, 16.0));
        root.addSubview(&hint);

        let add_btn = NSButton::initWithFrame(
            NSButton::alloc(mtm),
            rect(
                width - TAB_PADDING - 96.0,
                height - TOP_BAR_HEIGHT - 2.0,
                96.0,
                24.0,
            ),
        );
        add_btn.setTitle(&NSString::from_str("＋ 新增片段"));
        add_btn.setBezelStyle(NSBezelStyle::Push);
        add_btn.setFont(Some(&NSFont::systemFontOfSize(11.0)));
        let target: &objc2::runtime::AnyObject = controller;
        add_btn.setTarget(Some(target));
        add_btn.setAction(Some(objc2::sel!(addSnippet:)));
        root.addSubview(&add_btn);

        // 片段列表滚动区（与设置页现有风格一致：Overlay 滚动条、透明背景）
        let scroll_w = width - TAB_PADDING * 2.0;
        let scroll_h = height - TOP_BAR_HEIGHT - 12.0;
        let scroll = NSScrollView::initWithFrame(
            NSScrollView::alloc(mtm),
            rect(TAB_PADDING, 8.0, scroll_w, scroll_h),
        );
        scroll.setHasVerticalScroller(true);
        scroll.setScrollerStyle(NSScrollerStyle::Overlay);
        scroll.setDrawsBackground(false);
        let rows = NSView::initWithFrame(NSView::alloc(mtm), rect(0.0, 0.0, scroll_w, scroll_h));
        scroll.setDocumentView(Some(&rows));
        root.ivars().rows_container.borrow_mut().replace(rows);
        root.addSubview(&scroll);

        root.into_super()
    }
}

/// 重查 `list_snippets` 全量重建行（照抄 `update_list` 的 drain + 重建模式）；
/// 空列表显示占位提示。
pub fn refresh(globals: &'static Globals) {
    let Some(w) = *globals.settings.borrow() else {
        return;
    };
    let mtm = MainThreadMarker::new().expect("main thread");
    let snippets = {
        let store = globals.store.lock().expect("store poisoned");
        store.list_snippets().unwrap_or_default()
    };
    // 根视图是 snippet_container 的唯一子视图（build 装入）
    let Some(root) = w.snippet_container.subviews().firstObject() else {
        return;
    };
    let Some(tab) = root.downcast_ref::<SnippetTabView>() else {
        return;
    };
    let controller = tab.controller();
    let Some(rows_view) = tab.rows_container() else {
        return;
    };
    unsafe {
        let area_h = rows_view.frame().size.height;
        let row_w = rows_view.frame().size.width;
        for old in rows_view.subviews().iter() {
            old.removeFromSuperview();
        }
        if snippets.is_empty() {
            // 空列表占位提示（居中）
            let hint = label(&NSString::from_str("暂无片段，点击右上角新增"), 12.0, mtm);
            hint.setTextColor(Some(&NSColor::secondaryLabelColor()));
            hint.setAlignment(NSTextAlignment::Center);
            hint.setFrame(rect(0.0, area_h / 2.0 - 10.0, row_w, 20.0));
            rows_view.addSubview(&hint);
            return;
        }
        // documentView 高度 = max(实际行数×行高, 滚动区高度)，
        // 行少时短于视口也能从顶部排（同 update_list）
        let container_h = (snippets.len() as f64 * SNIPPET_ROW_HEIGHT).max(area_h);
        rows_view.setFrameSize(CGSize::new(row_w, container_h));
        for (i, snippet) in snippets.iter().enumerate() {
            let row = build_snippet_row(mtm, row_w, snippet, controller);
            // 行 y 从顶部排（非翻转坐标系）
            let y = container_h - (i as f64 + 1.0) * SNIPPET_ROW_HEIGHT;
            row.setFrameOrigin(CGPoint::new(0.0, y));
            rows_view.addSubview(&row);
        }
    }
}

/// 「＋ 新增片段」：在设置窗口上打开空白 sheet。
pub fn add(globals: &'static Globals) {
    if let Some(w) = *globals.settings.borrow() {
        super::snippet_sheet::show(globals, &w.window, None);
    }
}

/// 行「编辑」：sender ivars 携带 id → 查库回填 sheet。
pub fn edit(globals: &'static Globals, sender: Option<&objc2::runtime::AnyObject>) {
    let Some(button) = sender.and_then(|s| s.downcast_ref::<SnippetRowButton>()) else {
        return;
    };
    let id = button.ivars().snippet_id;
    let Some(w) = *globals.settings.borrow() else {
        return;
    };
    let snippet: Option<SnippetItem> = {
        let store = globals.store.lock().expect("store poisoned");
        store
            .list_snippets()
            .unwrap_or_default()
            .into_iter()
            .find(|s| s.id == id)
    };
    let Some(snippet) = snippet else {
        tracing::warn!("编辑片段：id={id} 不存在（可能已被删除）");
        return;
    };
    super::snippet_sheet::show(globals, &w.window, Some(snippet));
}

/// 行「删除」：NSAlert 确认（文案含片段名称）→ `delete_snippet` → 刷新列表。
///
/// Borrow 纪律：任何 RefCell / store 锁都不跨 `runModal` 的嵌套事件循环。
pub fn delete(globals: &'static Globals, sender: Option<&objc2::runtime::AnyObject>) {
    let Some(button) = sender.and_then(|s| s.downcast_ref::<SnippetRowButton>()) else {
        return;
    };
    let id = button.ivars().snippet_id;
    let name: Option<String> = {
        let store = globals.store.lock().expect("store poisoned");
        store
            .list_snippets()
            .unwrap_or_default()
            .into_iter()
            .find(|s| s.id == id)
            .map(|s| s.name)
    };
    let Some(name) = name else {
        tracing::warn!("删除片段：id={id} 不存在（可能已被删除）");
        return;
    };
    let mtm = MainThreadMarker::new().expect("main thread");
    unsafe {
        // 复用 settings::show_error 同款 NSAlert 构建方式（runModal 确认）
        let alert: Retained<NSAlert> = msg_send![NSAlert::alloc(mtm), init];
        alert.setMessageText(&NSString::from_str("删除片段"));
        alert.setInformativeText(&NSString::from_str(&format!(
            "确定删除「{name}」？此操作不可撤销。"
        )));
        alert.addButtonWithTitle(&NSString::from_str("取消"));
        alert.addButtonWithTitle(&NSString::from_str("删除"));
        if alert.runModal() == NSAlertSecondButtonReturn {
            if let Err(e) = globals
                .store
                .lock()
                .expect("store poisoned")
                .delete_snippet(id)
            {
                super::settings::show_error("删除片段失败", &format!("{e:#}"));
                return;
            }
            tracing::info!("片段已删除 id={id} name={name:?}");
            refresh(globals);
        }
    }
}

/// 构建一行：名称（主文本）· 关键词（accent 次级）· 内容单行预览 +
/// 行尾「编辑」「删除」小按钮（ivars 携带 id，target 统一 AppController）。
///
/// # Safety
///
/// 必须在主线程调用（mtm 保证）。
unsafe fn build_snippet_row(
    mtm: MainThreadMarker,
    row_width: f64,
    snippet: &SnippetItem,
    controller: &'static AppController,
) -> Retained<NSView> {
    unsafe {
        let row = NSView::initWithFrame(
            NSView::alloc(mtm),
            rect(0.0, 0.0, row_width, SNIPPET_ROW_HEIGHT),
        );

        // 名称（主文本）
        let name = label(&NSString::from_str(&snippet.name), 12.0, mtm);
        name.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
        name.setFrame(rect(0.0, 9.0, NAME_WIDTH, 18.0));
        row.addSubview(&name);

        // 关键词（accent 次级色；等宽字体需 10.15+ 版本门槛 API，按规约不引入，
        // 以 accent 色 + 次级字号区分）
        let keyword = label(&NSString::from_str(&snippet.keyword), 11.0, mtm);
        keyword.setTextColor(Some(&crate::compat::accent_color()));
        keyword.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
        keyword.setFrame(rect(NAME_WIDTH + 4.0, 10.0, KEYWORD_WIDTH, 16.0));
        row.addSubview(&keyword);

        // 内容单行预览：自行 60 char 截断（未复用 model::make_preview——
        // 其 200 char 上限对 Tab 行太长，60 char 足够单行展示；换行折叠见
        // [`content_preview`]）
        let preview = label(
            &NSString::from_str(&content_preview(&snippet.content)),
            10.0,
            mtm,
        );
        preview.setTextColor(Some(&NSColor::secondaryLabelColor()));
        preview.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
        let preview_x = NAME_WIDTH + 4.0 + KEYWORD_WIDTH + 4.0;
        let text_right = row_width - 8.0 - ROW_BUTTON_WIDTH * 2.0 - 4.0 - 8.0;
        preview.setFrame(rect(
            preview_x,
            11.0,
            (text_right - preview_x).max(40.0),
            16.0,
        ));
        row.addSubview(&preview);

        // 行尾「编辑」「删除」小按钮
        let del_x = row_width - 8.0 - ROW_BUTTON_WIDTH;
        let edit_x = del_x - 4.0 - ROW_BUTTON_WIDTH;
        let button_y = (SNIPPET_ROW_HEIGHT - ROW_BUTTON_HEIGHT) / 2.0;
        let edit_btn = row_button(mtm, "编辑", snippet.id, objc2::sel!(editSnippet:));
        edit_btn.setFrame(rect(edit_x, button_y, ROW_BUTTON_WIDTH, ROW_BUTTON_HEIGHT));
        let target: &objc2::runtime::AnyObject = controller;
        edit_btn.setTarget(Some(target));
        row.addSubview(&edit_btn);
        let del_btn = row_button(mtm, "删除", snippet.id, objc2::sel!(deleteSnippet:));
        del_btn.setFrame(rect(del_x, button_y, ROW_BUTTON_WIDTH, ROW_BUTTON_HEIGHT));
        del_btn.setTarget(Some(target));
        row.addSubview(&del_btn);

        row
    }
}

/// 行小按钮工厂：ivars 携带 snippet id；action 由调用方按编辑/删除指定。
unsafe fn row_button(
    mtm: MainThreadMarker,
    title: &str,
    snippet_id: i64,
    action: objc2::runtime::Sel,
) -> Retained<SnippetRowButton> {
    unsafe {
        let alloc = SnippetRowButton::alloc(mtm).set_ivars(SnippetRowButtonIvars { snippet_id });
        let button: Retained<SnippetRowButton> = msg_send![super(alloc), initWithFrame: rect(
            0.0,
            0.0,
            ROW_BUTTON_WIDTH,
            ROW_BUTTON_HEIGHT
        )];
        button.setTitle(&NSString::from_str(title));
        button.setBezelStyle(NSBezelStyle::Push);
        button.setFont(Some(&NSFont::systemFontOfSize(10.0)));
        button.setAction(Some(action));
        button
    }
}

/// 内容单行预览：取前 60 个 char（换行折叠为空格），超长追加省略号。
fn content_preview(content: &str) -> String {
    const PREVIEW_CHARS: usize = 60;
    let mut out = String::new();
    let mut truncated = false;
    for (i, ch) in content.chars().enumerate() {
        if i == PREVIEW_CHARS {
            truncated = true;
            break;
        }
        out.push(if ch == '\n' || ch == '\r' { ' ' } else { ch });
    }
    if truncated {
        out.push('…');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_preview_short_text_unchanged() {
        assert_eq!(content_preview("hello"), "hello");
        assert_eq!(content_preview(""), "");
    }

    #[test]
    fn content_preview_folds_newlines() {
        assert_eq!(content_preview("a\nb\rc\r\nd"), "a b c  d");
    }

    #[test]
    fn content_preview_truncates_at_60_chars_with_ellipsis() {
        let s = "x".repeat(61);
        let p = content_preview(&s);
        assert_eq!(p.chars().count(), 61);
        assert!(p.ends_with('…'));
        // 恰好 60 char 不截断
        assert_eq!(content_preview(&"x".repeat(60)), "x".repeat(60));
    }

    #[test]
    fn content_preview_counts_chars_not_bytes() {
        let s = "中".repeat(61);
        let p = content_preview(&s);
        assert_eq!(p.chars().count(), 61);
        assert!(p.ends_with('…'));
    }
}
