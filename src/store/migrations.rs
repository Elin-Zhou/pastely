//! SQLite 迁移链：以 `PRAGMA user_version` 为版本号，逐版本推进。
//! Schema 的唯一来源；只增不删，向后兼容。

use rusqlite::Connection;

/// 迁移链：第 i 项是将 user_version 从 i 升级到 i+1 的 DDL。
const MIGRATIONS: &[&str] = &[
    // v1：初版建表
    "\
    CREATE TABLE clip_items (
      id           INTEGER PRIMARY KEY AUTOINCREMENT,
      kind         TEXT    NOT NULL CHECK(kind IN ('text','image')),
      text_content TEXT,
      image_data   BLOB,
      img_w        INTEGER,
      img_h        INTEGER,
      content_hash TEXT    NOT NULL,
      created_at   INTEGER NOT NULL,
      pinned       INTEGER NOT NULL DEFAULT 0
    );
    CREATE UNIQUE INDEX idx_clip_items_hash ON clip_items(content_hash);
    CREATE INDEX idx_clip_items_time ON clip_items(created_at DESC);
    ",
    // v2：快捷片段表（V0.3）
    "\
    CREATE TABLE snippets (
      id         INTEGER PRIMARY KEY AUTOINCREMENT,
      name       TEXT    NOT NULL,
      keyword    TEXT    NOT NULL,
      content    TEXT    NOT NULL,
      created_at INTEGER NOT NULL,
      updated_at INTEGER NOT NULL
    );
    CREATE INDEX idx_snippets_keyword ON snippets(keyword);
    ",
];

/// 将数据库迁移到最新版本（每个版本一步，事务内推进 user_version）。
pub(crate) fn migrate(conn: &Connection) -> rusqlite::Result<()> {
    let mut version: i64 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    for (i, ddl) in MIGRATIONS.iter().enumerate() {
        let target = (i + 1) as i64;
        if version < target {
            conn.execute_batch(&format!(
                "BEGIN;\n{ddl}\nPRAGMA user_version = {target};\nCOMMIT;"
            ))?;
            version = target;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::params;

    fn user_version(conn: &Connection) -> i64 {
        conn.query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap()
    }

    #[test]
    fn migrate_fresh_db_reaches_v2_and_both_tables_are_writable() {
        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();
        assert_eq!(user_version(&conn), 2);
        conn.execute(
            "INSERT INTO clip_items (kind, text_content, content_hash, created_at)
             VALUES (?1, ?2, ?3, ?4)",
            params!["text", "hello", "a".repeat(32), 1_i64],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO snippets (name, keyword, content, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params!["签名", "sig", "张三", 1_i64, 2_i64],
        )
        .unwrap();
    }

    #[test]
    fn migrate_is_idempotent() {
        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();
        migrate(&conn).unwrap();
        assert_eq!(user_version(&conn), 2);
    }

    #[test]
    fn migrate_v1_db_upgrades_to_v2_and_preserves_clip_items() {
        // 模拟 v1 老库：手工执行 v1 DDL + user_version = 1
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(&format!(
            "BEGIN;\n{}\nPRAGMA user_version = 1;\nCOMMIT;",
            MIGRATIONS[0]
        ))
        .unwrap();
        assert_eq!(user_version(&conn), 1);
        conn.execute(
            "INSERT INTO clip_items (kind, text_content, content_hash, created_at)
             VALUES (?1, ?2, ?3, ?4)",
            params!["text", "old row", "a".repeat(32), 7_i64],
        )
        .unwrap();

        migrate(&conn).unwrap();
        assert_eq!(user_version(&conn), 2);
        // 原 clip_items 数据保留
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM clip_items", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1);
        let text: Option<String> = conn
            .query_row("SELECT text_content FROM clip_items", [], |r| r.get(0))
            .unwrap();
        assert_eq!(text.as_deref(), Some("old row"));
        // v2 新表可写
        conn.execute(
            "INSERT INTO snippets (name, keyword, content, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params!["地址", "addr", "北京市", 1_i64, 2_i64],
        )
        .unwrap();
    }

    #[test]
    fn v1_schema_rejects_unknown_kind() {
        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();
        let err = conn.execute(
            "INSERT INTO clip_items (kind, content_hash, created_at) VALUES ('video', 'h', 1)",
            [],
        );
        assert!(
            err.is_err(),
            "CHECK(kind IN ('text','image')) should reject"
        );
    }

    #[test]
    fn v1_schema_enforces_unique_content_hash() {
        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();
        conn.execute(
            "INSERT INTO clip_items (kind, content_hash, created_at) VALUES ('text', 'h', 1)",
            [],
        )
        .unwrap();
        let dup = conn.execute(
            "INSERT INTO clip_items (kind, content_hash, created_at) VALUES ('text', 'h', 2)",
            [],
        );
        assert!(dup.is_err(), "unique index on content_hash should reject");
    }
}
