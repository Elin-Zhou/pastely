//! 搜索框 field editor：拦截 ↑↓/⏎/Esc 键盘事件。
//!
//! 焦点在 NSSearchField 时，方向键/回车会被 NSTextView（field editor）消费，
//! 到不了 window——因此用自定义 NSTextView 子类做 field editor，在
//! `windowWillReturnFieldEditor:toObject:` 中交给面板。

use objc2::define_class;
use objc2::msg_send;
use objc2::rc::Retained;
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{NSEvent, NSTextInputClient, NSTextView};

use super::PanelRuntime;

// 虚拟键码（HID）：Up=126 Down=125 Return=36 Enter=76 Escape=53；顶部数字 1-9=18..=26
const K_UP: u16 = 126;
const K_DOWN: u16 = 125;
const K_RETURN: u16 = 36;
const K_ENTER: u16 = 76;
const K_ESCAPE: u16 = 53;
const K_NUM_1: u16 = 18;
const K_NUM_9: u16 = 26;

define_class!(
    // SAFETY: NSTextView 无子类化约束；本类型不实现 Drop。
    #[unsafe(super(NSTextView))]
    #[thread_kind = MainThreadOnly]
    #[name = "ClipHubPanelFieldEditor"]
    #[ivars = *mut PanelRuntime]
    pub struct PanelFieldEditor;

    impl PanelFieldEditor {
        #[unsafe(method(keyDown:))]
        unsafe fn key_down(&self, event: &NSEvent) {
            let key = event.keyCode();
            // IME 组合中（存在 marked text）：全部按键原样交回系统 field editor，
            // 否则会吞掉候选窗的确认/翻页等组合键
            if self.hasMarkedText() {
                let _: () = unsafe { msg_send![super(self), keyDown: event] };
                return;
            }
            // SAFETY: 共享引用即可（方法全部走 &self），避免与 Globals::runtime()
            // 重新派生的 &mut PanelRuntime 构成双重可变别名
            let runtime: &PanelRuntime = unsafe { &*(*self.ivars()) };
            // 数字键直达：顶部数字 1-9 确认对应行；越界/空列表时透传系统
            if (K_NUM_1..=K_NUM_9).contains(&key) && runtime.confirm_at((key - K_NUM_1) as usize)
            {
                return;
            }
            match key {
                K_UP => runtime.move_selection(-1),
                K_DOWN => runtime.move_selection(1),
                K_RETURN | K_ENTER => runtime.confirm_current(),
                K_ESCAPE => runtime.hide(),
                _ => {
                    // 其余按键（含 IME 组合输入）交回系统 field editor 行为
                    let _: () = unsafe { msg_send![super(self), keyDown: event] };
                }
            }
        }
    }
);

impl PanelFieldEditor {
    /// 创建并以 `initWithFrame:` 初始化（NSTextView 惯例零帧，由 window 管理）。
    ///
    /// # Safety
    ///
    /// `runtime` 必须指向进程生命周期内存（泄漏的 `Box<PanelRuntime>`）。
    pub unsafe fn new(mtm: MainThreadMarker, runtime: *mut PanelRuntime) -> Retained<Self> {
        unsafe {
            let zero = objc2_core_foundation::CGRect::ZERO;
            let this = Self::alloc(mtm).set_ivars(runtime);
            msg_send![super(this), initWithFrame: zero]
        }
    }
}
