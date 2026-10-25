//! 一次性清理：TTL 过期清除 + 容量淘汰的组合入口。定时调度属装配层。

use crate::store::ClipStore;

/// 执行一轮清理，返回 `(expired 删除数, capacity 淘汰数)`。
///
/// 先 TTL 后容量：过期清除作用于全量，容量淘汰只处理幸存者，
/// 因此 capacity 计数不含本就被 TTL 清除的条目。`pinned` 条目对两者均豁免。
pub fn reap_once(
    store: &ClipStore,
    ttl_days: u32,
    max_items: usize,
) -> anyhow::Result<(usize, usize)> {
    let expired = store.purge_expired(ttl_days)?;
    let evicted = store.enforce_capacity(max_items)?;
    Ok((expired, evicted))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ClipKind, NewClipItem};
    use std::time::{SystemTime, UNIX_EPOCH};

    const DAY_MS: i64 = 86_400_000;

    fn now_ms() -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64
    }

    fn text_item(hash: &str, created_at: i64) -> NewClipItem {
        NewClipItem {
            kind: ClipKind::Text,
            text: Some(format!("t-{hash}")),
            image_png: None,
            img_size: None,
            hash: hash.to_string(),
            created_at,
        }
    }

    #[test]
    fn reap_once_returns_expired_and_evicted_counts() {
        let store = ClipStore::open_in_memory().unwrap();
        let now = now_ms();
        // 过期非置顶：TTL 清 1
        store
            .upsert(&text_item("expired", now - 2 * DAY_MS))
            .unwrap();
        // 4 条新鲜记录，max_items=3：TTL 后剩 4 → 容量再淘汰最旧 1 条
        for i in 0..4 {
            store.upsert(&text_item(&format!("f{i}"), now + i)).unwrap();
        }
        let (expired, evicted) = reap_once(&store, 1, 3).unwrap();
        assert_eq!((expired, evicted), (1, 1));
        assert_eq!(store.count().unwrap(), 3);
        // "expired" 已清，最旧的 f0 被容量淘汰
        let remaining = store.all_searchable().unwrap();
        let texts: Vec<&str> = remaining.iter().map(|i| i.full_text.as_str()).collect();
        assert_eq!(texts, vec!["t-f3", "t-f2", "t-f1"]);
    }

    #[test]
    fn pinned_is_exempt_from_both_ttl_and_capacity() {
        let store = ClipStore::open_in_memory().unwrap();
        let now = now_ms();
        let pinned_old = store
            .upsert(&text_item("pin-old", now - 2 * DAY_MS))
            .unwrap();
        store.set_pinned(pinned_old, true).unwrap();
        store.upsert(&text_item("u1", now + 1)).unwrap();
        store.upsert(&text_item("u2", now + 2)).unwrap();

        let (expired, evicted) = reap_once(&store, 1, 1).unwrap();
        // 旧置顶免 TTL；容量压到 1 时淘汰 2 条非置顶，仅剩置顶
        assert_eq!((expired, evicted), (0, 2));
        assert_eq!(store.count().unwrap(), 1);
        let left = &store.all_searchable().unwrap()[0];
        assert_eq!(left.full_text, "t-pin-old");
        assert!(left.pinned);
    }

    #[test]
    fn ttl_zero_and_no_capacity_pressure_is_noop() {
        let store = ClipStore::open_in_memory().unwrap();
        let ancient = now_ms() - 400 * DAY_MS;
        store.upsert(&text_item("ancient", ancient)).unwrap();
        store.upsert(&text_item("fresh", now_ms())).unwrap();
        assert_eq!(reap_once(&store, 0, 1000).unwrap(), (0, 0));
        assert_eq!(store.count().unwrap(), 2);
    }

    #[test]
    fn capacity_only_when_nothing_expired() {
        let store = ClipStore::open_in_memory().unwrap();
        let now = now_ms();
        for i in 0..5 {
            store.upsert(&text_item(&format!("n{i}"), now + i)).unwrap();
        }
        let (expired, evicted) = reap_once(&store, 30, 3).unwrap();
        assert_eq!((expired, evicted), (0, 2));
        assert_eq!(store.count().unwrap(), 3);
    }
}
