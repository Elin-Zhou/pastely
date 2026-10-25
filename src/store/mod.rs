//! SQLite 持久化：`ClipStore` 封装 clip_items 的全部读写。
//! 打开即建库 + 迁移 + WAL；库文件损坏的恢复（改名 `.bak` 重建）属装配层。

mod migrations;

use std::path::Path;

use anyhow::Context;
use rusqlite::{Connection, OptionalExtension};

use crate::model::{text_preview, ClipKind, NewClipItem, SearchableItem};

/// 一天的毫秒数（TTL 计算用）。
const DAY_MS: i64 = 86_400_000;

/// clip_items 的 SQLite 句柄。
pub struct ClipStore {
    conn: Connection,
}

impl ClipStore {
    /// 打开（或创建）文件库：迁移 + WAL。
    pub fn open(path: &Path) -> anyhow::Result<Self> {
        let conn = Connection::open(path)
            .with_context(|| format!("failed to open clip db at {}", path.display()))?;
        conn.pragma_update(None, "journal_mode", "wal")
            .context("failed to enable WAL journal mode")?;
        Self::open_with(conn)
    }

    /// 内存库（测试用）。
    pub fn open_in_memory() -> anyhow::Result<Self> {
        Self::open_with(Connection::open_in_memory()?)
    }

    fn open_with(conn: Connection) -> anyhow::Result<Self> {
        migrations::migrate(&conn)?;
        Ok(Self { conn })
    }

    /// 写入条目；`hash` 冲突时仅刷新 `created_at`（置顶），返回行 id（冲突时为既有行 id）。
    pub fn upsert(&self, item: &NewClipItem) -> rusqlite::Result<i64> {
        let (img_w, img_h) = match item.img_size {
            Some((w, h)) => (Some(w as i64), Some(h as i64)),
            None => (None, None),
        };
        self.conn.query_row(
            "INSERT INTO clip_items
                 (kind, text_content, image_data, img_w, img_h, content_hash, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(content_hash) DO UPDATE SET created_at = excluded.created_at
             RETURNING id",
            rusqlite::params![
                item.kind.as_str(),
                item.text,
                item.image_png,
                img_w,
                img_h,
                item.hash,
                item.created_at,
            ],
            |row| row.get(0),
        )
    }

    /// 全量加载（不含 image_data BLOB），created_at 降序。
    pub fn all_searchable(&self) -> rusqlite::Result<Vec<SearchableItem>> {
        let mut stmt = self.conn.prepare(&searchable_sql(None))?;
        let rows = stmt.query_map([], searchable_from_row)?;
        rows.collect()
    }

    /// 同 [`Self::all_searchable`]，但限条数（UI 空查询首屏用）。
    pub fn recent(&self, limit: usize) -> rusqlite::Result<Vec<SearchableItem>> {
        let mut stmt = self.conn.prepare(&searchable_sql(Some("?1")))?;
        let rows = stmt.query_map([limit as i64], searchable_from_row)?;
        rows.collect()
    }

    /// 按需取图片 BLOB。行不存在或 BLOB 为 NULL 均返回 `None`。
    pub fn image_bytes(&self, id: i64) -> rusqlite::Result<Option<Vec<u8>>> {
        Ok(self
            .conn
            .query_row(
                "SELECT image_data FROM clip_items WHERE id = ?1",
                [id],
                |row| row.get::<_, Option<Vec<u8>>>(0),
            )
            .optional()?
            .flatten())
    }

    /// 删除单条（幂等：id 不存在也返回 Ok）。
    pub fn delete(&self, id: i64) -> rusqlite::Result<()> {
        self.conn
            .execute("DELETE FROM clip_items WHERE id = ?1", [id])?;
        Ok(())
    }

    /// 清空全部。
    pub fn clear(&self) -> rusqlite::Result<()> {
        self.conn.execute("DELETE FROM clip_items", [])?;
        Ok(())
    }

    /// 置顶标记。
    pub fn set_pinned(&self, id: i64, pinned: bool) -> rusqlite::Result<()> {
        self.conn.execute(
            "UPDATE clip_items SET pinned = ?2 WHERE id = ?1",
            rusqlite::params![id, pinned],
        )?;
        Ok(())
    }

    /// 总条数。
    pub fn count(&self) -> rusqlite::Result<usize> {
        let n: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM clip_items", [], |row| row.get(0))?;
        Ok(n as usize)
    }

    /// TTL 清理：`ttl_days == 0` 永久豁免；否则删除过期的非置顶条目，返回删除数。
    pub fn purge_expired(&self, ttl_days: u32) -> rusqlite::Result<usize> {
        if ttl_days == 0 {
            return Ok(0);
        }
        let cutoff = now_ms() - ttl_days as i64 * DAY_MS;
        let n = self.conn.execute(
            "DELETE FROM clip_items WHERE pinned = 0 AND created_at < ?1",
            [cutoff],
        )?;
        Ok(n)
    }

    /// 容量淘汰：超限时按 created_at 升序删最旧非 pinned 至 count == max_items，返回删除数。
    pub fn enforce_capacity(&self, max_items: usize) -> rusqlite::Result<usize> {
        let count = self.count()?;
        if count <= max_items {
            return Ok(0);
        }
        let excess = (count - max_items) as i64;
        let n = self.conn.execute(
            "DELETE FROM clip_items WHERE id IN (
                 SELECT id FROM clip_items WHERE pinned = 0
                 ORDER BY created_at ASC LIMIT ?1
             )",
            [excess],
        )?;
        Ok(n)
    }
}

/// 列表查询 SQL（不含 image_data BLOB）。
fn searchable_sql(limit: Option<&str>) -> String {
    let limit_clause = match limit {
        Some(placeholder) => format!(" LIMIT {placeholder}"),
        None => String::new(),
    };
    format!(
        "SELECT id, kind, text_content, img_w, img_h, created_at, pinned \
         FROM clip_items ORDER BY created_at DESC{limit_clause}"
    )
}

/// 行 → [`SearchableItem`]。preview 截断保留前缀，故高亮索引可直接沿用。
fn searchable_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<SearchableItem> {
    let id: i64 = row.get(0)?;
    let kind = ClipKind::parse(&row.get::<_, String>(1)?).ok_or_else(|| invalid_kind_error(row))?;
    let text_content: Option<String> = row.get(2)?;
    let img_w: Option<u32> = row.get(3)?;
    let img_h: Option<u32> = row.get(4)?;
    let created_at: i64 = row.get(5)?;
    let pinned: bool = row.get(6)?;
    let full_text = match kind {
        ClipKind::Text => text_content.unwrap_or_default(),
        ClipKind::Image => String::new(),
    };
    Ok(SearchableItem {
        id,
        kind,
        text_preview: text_preview(&full_text),
        full_text,
        img_size: match (img_w, img_h) {
            (Some(w), Some(h)) => Some((w, h)),
            _ => None,
        },
        created_at,
        pinned,
    })
}

/// DB 中出现未知 kind（违反 CHECK 约束的外部写入）时的转换错误。
fn invalid_kind_error(row: &rusqlite::Row<'_>) -> rusqlite::Error {
    let value = row
        .get::<_, String>(1)
        .unwrap_or_else(|_| "<unreadable>".to_string());
    rusqlite::Error::FromSqlConversionFailure(
        1,
        rusqlite::types::Type::Text,
        format!("invalid clip kind: {value}").into(),
    )
}

/// 当前 unix epoch 毫秒。
fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn now_ms() -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64
    }

    fn text_item(hash: &str, text: &str, created_at: i64) -> NewClipItem {
        NewClipItem {
            kind: ClipKind::Text,
            text: Some(text.to_string()),
            image_png: None,
            img_size: None,
            hash: hash.to_string(),
            created_at,
        }
    }

    fn image_item(hash: &str, created_at: i64) -> NewClipItem {
        NewClipItem {
            kind: ClipKind::Image,
            text: None,
            image_png: Some(vec![1, 2, 3, 4]),
            img_size: Some((640, 480)),
            hash: hash.to_string(),
            created_at,
        }
    }

    fn full_texts(items: &[SearchableItem]) -> Vec<String> {
        items.iter().map(|i| i.full_text.clone()).collect()
    }

    #[test]
    fn open_in_memory_runs_migrations() {
        let store = ClipStore::open_in_memory().unwrap();
        assert_eq!(store.count().unwrap(), 0);
        assert!(store.all_searchable().unwrap().is_empty());
    }

    #[test]
    fn open_file_db_enables_wal_and_persists() {
        let db_path =
            std::env::temp_dir().join(format!("cliphub-store-test-{}.db", std::process::id()));
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{}", db_path.display(), suffix));
        }
        {
            let store = ClipStore::open(&db_path).unwrap();
            store.upsert(&text_item("h-file", "file row", 1)).unwrap();
        }
        // 裸连接验证 WAL 与迁移版本
        let probe = Connection::open(&db_path).unwrap();
        let mode: String = probe
            .query_row("PRAGMA journal_mode", [], |r| r.get(0))
            .unwrap();
        assert_eq!(mode, "wal");
        let version: i64 = probe
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, 1);
        drop(probe);
        // 重新打开：数据仍在
        let store = ClipStore::open(&db_path).unwrap();
        assert_eq!(store.count().unwrap(), 1);
        drop(store);
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{}", db_path.display(), suffix));
        }
    }

    #[test]
    fn upsert_inserts_new_row() {
        let store = ClipStore::open_in_memory().unwrap();
        let id = store.upsert(&text_item("h1", "hello", 100)).unwrap();
        assert!(id > 0);
        assert_eq!(store.count().unwrap(), 1);
    }

    #[test]
    fn upsert_same_hash_refreshes_created_at_and_keeps_count() {
        let store = ClipStore::open_in_memory().unwrap();
        let id1 = store.upsert(&text_item("h-dup", "hello", 1_000)).unwrap();
        let id2 = store.upsert(&text_item("h-dup", "hello", 2_000)).unwrap();
        assert_eq!(id1, id2, "conflict upsert should return existing row id");
        assert_eq!(store.count().unwrap(), 1);
        let items = store.all_searchable().unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].created_at, 2_000, "created_at should be refreshed");
    }

    #[test]
    fn all_searchable_orders_desc_and_maps_fields() {
        let store = ClipStore::open_in_memory().unwrap();
        let text_old = store.upsert(&text_item("h1", "first", 100)).unwrap();
        let image_id = store.upsert(&image_item("h2", 200)).unwrap();
        let text_new = store.upsert(&text_item("h3", "second", 300)).unwrap();
        let items = store.all_searchable().unwrap();
        assert_eq!(items.len(), 3);
        assert_eq!(
            full_texts(&items),
            vec!["second".to_string(), String::new(), "first".to_string()]
        );
        assert_eq!(items[0].id, text_new);
        assert_eq!(items[0].kind, ClipKind::Text);
        assert_eq!(items[0].text_preview, "second");
        assert!(!items[0].pinned);
        assert_eq!(items[1].id, image_id);
        assert_eq!(items[1].kind, ClipKind::Image);
        assert_eq!(items[1].full_text, "");
        assert_eq!(items[1].text_preview, "");
        assert_eq!(items[1].img_size, Some((640, 480)));
        assert_eq!(items[2].id, text_old);
    }

    #[test]
    fn recent_limits_result_count() {
        let store = ClipStore::open_in_memory().unwrap();
        for i in 0..5 {
            store.upsert(&text_item(&format!("h{i}"), "t", i)).unwrap();
        }
        let top2 = store.recent(2).unwrap();
        assert_eq!(full_texts(&top2), vec!["t".to_string(), "t".to_string()]);
        assert_eq!(top2[0].created_at, 4);
        assert_eq!(top2[1].created_at, 3);
        assert_eq!(store.recent(10).unwrap().len(), 5);
        assert!(store.recent(0).unwrap().is_empty());
    }

    #[test]
    fn image_bytes_loads_blob_on_demand() {
        let store = ClipStore::open_in_memory().unwrap();
        let img_id = store.upsert(&image_item("h-img", 1)).unwrap();
        let text_id = store.upsert(&text_item("h-txt", "plain", 2)).unwrap();
        assert_eq!(store.image_bytes(img_id).unwrap(), Some(vec![1, 2, 3, 4]));
        assert_eq!(
            store.image_bytes(text_id).unwrap(),
            None,
            "text row has NULL blob"
        );
        assert_eq!(
            store.image_bytes(9_999).unwrap(),
            None,
            "missing row -> None"
        );
    }

    #[test]
    fn set_pinned_toggles_flag() {
        let store = ClipStore::open_in_memory().unwrap();
        let id = store.upsert(&text_item("h-pin", "p", 1)).unwrap();
        assert!(!store.all_searchable().unwrap()[0].pinned);
        store.set_pinned(id, true).unwrap();
        assert!(store.all_searchable().unwrap()[0].pinned);
        store.set_pinned(id, false).unwrap();
        assert!(!store.all_searchable().unwrap()[0].pinned);
    }

    #[test]
    fn delete_and_clear() {
        let store = ClipStore::open_in_memory().unwrap();
        let id1 = store.upsert(&text_item("h1", "a", 1)).unwrap();
        let id2 = store.upsert(&text_item("h2", "b", 2)).unwrap();
        store.delete(id1).unwrap();
        assert_eq!(store.count().unwrap(), 1);
        assert_eq!(store.all_searchable().unwrap()[0].id, id2);
        store.delete(4_242).unwrap(); // 幂等：不存在的 id 不报错
        store.clear().unwrap();
        assert_eq!(store.count().unwrap(), 0);
        assert!(store.all_searchable().unwrap().is_empty());
    }

    #[test]
    fn purge_expired_zero_means_forever() {
        let store = ClipStore::open_in_memory().unwrap();
        let ancient = now_ms() - 400 * DAY_MS;
        store
            .upsert(&text_item("h-old", "ancient", ancient))
            .unwrap();
        assert_eq!(store.purge_expired(0).unwrap(), 0);
        assert_eq!(store.count().unwrap(), 1, "ttl_days=0 must never purge");
    }

    #[test]
    fn purge_expired_removes_old_unpinned_only() {
        let store = ClipStore::open_in_memory().unwrap();
        let now = now_ms();
        let old = now - 2 * DAY_MS;
        store.upsert(&text_item("h-old", "old", old)).unwrap();
        let pinned_id = store
            .upsert(&text_item("h-old-pin", "pinned old", old))
            .unwrap();
        store.upsert(&text_item("h-new", "fresh", now)).unwrap();
        store.set_pinned(pinned_id, true).unwrap();

        assert_eq!(store.purge_expired(1).unwrap(), 1);
        assert_eq!(store.count().unwrap(), 2);
        assert_eq!(
            full_texts(&store.all_searchable().unwrap()),
            vec!["fresh".to_string(), "pinned old".to_string()]
        );
    }

    #[test]
    fn enforce_capacity_evicts_oldest_unpinned() {
        let store = ClipStore::open_in_memory().unwrap();
        for (i, name) in ["a", "b", "c", "d", "e"].iter().enumerate() {
            store
                .upsert(&text_item(&format!("h{i}"), name, (i + 1) as i64))
                .unwrap();
        }
        assert_eq!(store.enforce_capacity(3).unwrap(), 2);
        assert_eq!(
            full_texts(&store.all_searchable().unwrap()),
            vec!["e".to_string(), "d".to_string(), "c".to_string()]
        );
    }

    #[test]
    fn enforce_capacity_skips_pinned_when_evicting() {
        let store = ClipStore::open_in_memory().unwrap();
        let pinned_id = store.upsert(&text_item("h0", "a", 1)).unwrap();
        store.upsert(&text_item("h1", "b", 2)).unwrap();
        store.upsert(&text_item("h2", "c", 3)).unwrap();
        store.upsert(&text_item("h3", "d", 4)).unwrap();
        store.upsert(&text_item("h4", "e", 5)).unwrap();
        store.set_pinned(pinned_id, true).unwrap();

        assert_eq!(store.enforce_capacity(3).unwrap(), 2);
        assert_eq!(
            full_texts(&store.all_searchable().unwrap()),
            vec!["e".to_string(), "d".to_string(), "a".to_string()]
        );
    }

    #[test]
    fn enforce_capacity_noop_when_not_over_limit() {
        let store = ClipStore::open_in_memory().unwrap();
        store.upsert(&text_item("h1", "a", 1)).unwrap();
        store.upsert(&text_item("h2", "b", 2)).unwrap();
        assert_eq!(store.enforce_capacity(5).unwrap(), 0);
        assert_eq!(
            store.enforce_capacity(2).unwrap(),
            0,
            "count == max is not over"
        );
        assert_eq!(store.count().unwrap(), 2);
    }
}
