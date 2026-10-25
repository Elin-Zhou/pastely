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
pub fn tick<F: FnMut(NewClipItem)>(state: &mut WatcherState, mut sink: F) {
    let current = pasteboard::change_count();
    if current == state.last_count {
        return;
    }
    state.last_count = current;
    if state.paused {
        return;
    }
    let raw_text = pasteboard::read_text().filter(|s| !s.is_empty());
    let raw_image = if raw_text.is_some() {
        None
    } else {
        pasteboard::read_image()
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
}
