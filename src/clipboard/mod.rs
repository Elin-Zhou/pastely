//! 剪切板监听：changeCount 轮询 + 内容限额决策 + 去重回调。
//! NSTimer 调度与回调分发在装配层（main.rs）。

pub mod pasteboard;

use crate::model::{content_hash, now_ms, ClipKind, NewClipItem};

/// watcher 的可变状态（全部主线程访问）。
pub struct WatcherState {
    pub last_count: i64,
    pub paused: bool,
    pub max_text_chars: usize,
    pub max_image_mb: u32,
}

impl WatcherState {
    pub fn new(max_text_chars: usize, max_image_mb: u32) -> Self {
        Self {
            last_count: pasteboard::change_count(),
            paused: false,
            max_text_chars,
            max_image_mb,
        }
    }
}

/// 一次内容读取的决策结果。
#[derive(Debug, Clone, PartialEq)]
pub enum WatchOutcome {
    /// 不入库（无内容/超限）。
    Skip,
    /// 入库的新条目。
    Store(NewClipItem),
}

/// 纯决策函数：给定读到的新内容与限额，产出入库条目或跳过。
///
/// - 文本优先；文本按 `max_text_chars` 截断（按 char）后入库
/// - 无文本才读图片；PNG 超过 `max_image_mb` 跳过
/// - hash 基于（截断后）内容计算
pub fn decide(
    raw_text: Option<String>,
    raw_image: Option<(Vec<u8>, u32, u32)>,
    max_text_chars: usize,
    max_image_mb: u32,
) -> WatchOutcome {
    if let Some(text) = raw_text {
        if text.is_empty() {
            return WatchOutcome::Skip;
        }
        let truncated: String = text.chars().take(max_text_chars).collect();
        let hash = content_hash(truncated.as_bytes());
        return WatchOutcome::Store(NewClipItem {
            kind: ClipKind::Text,
            text: Some(truncated),
            image_png: None,
            img_size: None,
            hash,
            created_at: now_ms(),
        });
    }
    if let Some((png, w, h)) = raw_image {
        let limit = u64::from(max_image_mb) * 1024 * 1024;
        if png.len() as u64 > limit {
            return WatchOutcome::Skip;
        }
        let hash = content_hash(&png);
        return WatchOutcome::Store(NewClipItem {
            kind: ClipKind::Image,
            text: None,
            image_png: Some(png),
            img_size: Some((w, h)),
            hash,
            created_at: now_ms(),
        });
    }
    WatchOutcome::Skip
}

/// 轮询一步（NSTimer 每 300ms 调用）：变化才读取，产出条目回调给 sink。
///
/// 直连系统剪切板（`pasteboard::change_count`/`read_text`/`read_image`）。
pub fn tick<F: FnMut(NewClipItem)>(state: &mut WatcherState, sink: F) {
    tick_with(
        state,
        pasteboard::change_count,
        pasteboard::read_text,
        pasteboard::read_image,
        sink,
    );
}

/// [`tick`] 的可注入版本：读取原语全部由调用方提供，基线推进/暂停/去重状态机
/// 因此可脱离 NSPasteboard 做单元测试（防自回环语义见下方测试）。
pub fn tick_with<F: FnMut(NewClipItem)>(
    state: &mut WatcherState,
    read_count: impl Fn() -> i64,
    read_text: impl Fn() -> Option<String>,
    read_image: impl Fn() -> Option<(Vec<u8>, u32, u32)>,
    mut sink: F,
) {
    let current = read_count();
    if current == state.last_count {
        return;
    }
    state.last_count = current;
    if state.paused {
        return;
    }
    let raw_text = read_text().filter(|s| !s.is_empty());
    let raw_image = if raw_text.is_some() {
        None
    } else {
        read_image()
    };
    match decide(
        raw_text,
        raw_image,
        state.max_text_chars,
        state.max_image_mb,
    ) {
        WatchOutcome::Store(item) => sink(item),
        WatchOutcome::Skip => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image(png: &[u8]) -> Option<(Vec<u8>, u32, u32)> {
        Some((png.to_vec(), 4, 4))
    }

    #[test]
    fn text_is_truncated_by_chars_and_hashed() {
        let out = decide(Some("你好世界".to_string()), None, 2, 10);
        let WatchOutcome::Store(item) = out else {
            panic!("expected store");
        };
        assert_eq!(item.text.as_deref(), Some("你好"));
        assert_eq!(item.kind, ClipKind::Text);
        assert_eq!(item.hash, content_hash("你好".as_bytes()));
    }

    #[test]
    fn empty_text_falls_through_to_skip() {
        assert_eq!(
            decide(Some(String::new()), None, 10, 10),
            WatchOutcome::Skip
        );
    }

    #[test]
    fn image_over_limit_skipped() {
        let big = vec![0u8; 11 * 1024 * 1024];
        assert_eq!(decide(None, image(&big), 10, 10), WatchOutcome::Skip);
    }

    #[test]
    fn image_within_limit_stored_with_size() {
        let out = decide(None, image(&[1, 2, 3]), 10, 10);
        let WatchOutcome::Store(item) = out else {
            panic!("expected store");
        };
        assert_eq!(item.kind, ClipKind::Image);
        assert_eq!(item.img_size, Some((4, 4)));
        assert_eq!(item.hash, content_hash(&[1, 2, 3]));
    }

    #[test]
    fn nothing_readable_skips() {
        assert_eq!(decide(None, None, 10, 10), WatchOutcome::Skip);
    }

    #[test]
    fn text_takes_priority_over_image() {
        let out = decide(Some("t".to_string()), image(&[9]), 10, 10);
        let WatchOutcome::Store(item) = out else {
            panic!("expected store");
        };
        assert_eq!(item.kind, ClipKind::Text);
    }

    // —— 基线状态机（tick_with 注入读取原语，防自回环语义回归）——

    fn watcher_at(last_count: i64, paused: bool) -> WatcherState {
        WatcherState {
            last_count,
            paused,
            max_text_chars: 100,
            max_image_mb: 1,
        }
    }

    #[test]
    fn unchanged_count_reads_nothing_and_fires_nothing() {
        let mut state = watcher_at(7, false);
        let reads = std::cell::Cell::new(0u32);
        let fired = std::cell::Cell::new(0u32);
        tick_with(
            &mut state,
            || 7,
            || {
                reads.set(reads.get() + 1);
                Some("x".to_string())
            },
            || {
                reads.set(reads.get() + 1);
                None
            },
            |_| fired.set(fired.get() + 1),
        );
        assert_eq!(reads.get(), 0, "count 未变不得读取剪切板内容");
        assert_eq!(fired.get(), 0, "count 未变不得产出条目");
        assert_eq!(state.last_count, 7);
    }

    #[test]
    fn paused_tick_advances_baseline_and_never_backfills() {
        let mut state = watcher_at(7, true);
        let fired = std::cell::Cell::new(0u32);
        tick_with(
            &mut state,
            || 9,
            || panic!("暂停中不得读取文本"),
            || panic!("暂停中不得读取图片"),
            |_| fired.set(fired.get() + 1),
        );
        assert_eq!(fired.get(), 0, "暂停中不得入库");
        assert_eq!(state.last_count, 9, "暂停中基线仍须推进（防自回环）");

        // 恢复记录后，同一 changeCount 不得补发暂停期间的内容
        state.paused = false;
        tick_with(
            &mut state,
            || 9,
            || Some("迟到的内容".to_string()),
            || None,
            |_| fired.set(fired.get() + 1),
        );
        assert_eq!(fired.get(), 0, "同一 changeCount 恢复后不得补发");
    }

    #[test]
    fn count_jump_fires_once_then_dedups_same_count() {
        let mut state = watcher_at(1, false);
        let seen: std::cell::RefCell<Vec<NewClipItem>> = std::cell::RefCell::new(Vec::new());
        tick_with(
            &mut state,
            || 3,
            || Some("hello".to_string()),
            || None,
            |it| seen.borrow_mut().push(it),
        );
        {
            let seen = seen.borrow();
            assert_eq!(seen.len(), 1, "跳变一次只应产出一条");
            assert_eq!(seen[0].kind, ClipKind::Text);
            assert_eq!(seen[0].text.as_deref(), Some("hello"));
            assert_eq!(seen[0].hash, content_hash("hello".as_bytes()));
        }
        assert_eq!(state.last_count, 3);

        // 第二次同 count：去重，不再触发
        tick_with(
            &mut state,
            || 3,
            || Some("hello".to_string()),
            || None,
            |it| seen.borrow_mut().push(it),
        );
        assert_eq!(seen.borrow().len(), 1, "同 count 重复轮询不得重复入库");
    }
}
