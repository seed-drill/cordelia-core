//! Cordelia Storage -- rusqlite wrapper for schema v4.
//!
//! Shares the SAME cordelia.db file as the TypeScript MCP server.
//! WAL mode + busy_timeout for concurrent access.
//!
//! The P2P layer does NOT need L1, FTS, or embedding ops.
//! This crate exposes only what the P2P node requires.

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("schema version mismatch: expected {expected}, found {found}")]
    SchemaVersionMismatch { expected: u32, found: u32 },
    #[error("lock poisoned")]
    LockPoisoned,
}

pub type Result<T> = std::result::Result<T, StorageError>;

/// Row types matching the SQLite schema v4.

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct L2ItemRow {
    pub id: String,
    pub item_type: String,
    pub owner_id: Option<String>,
    pub visibility: String,
    pub data: Vec<u8>,
    pub checksum: Option<String>,
    pub group_id: Option<String>,
    pub author_id: Option<String>,
    pub key_version: i32,
    pub parent_id: Option<String>,
    pub is_copy: bool,
    pub access_count: i64,
    pub last_accessed_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct L2ItemMeta {
    pub owner_id: Option<String>,
    pub visibility: String,
    pub group_id: Option<String>,
    pub author_id: Option<String>,
    pub key_version: i32,
    pub parent_id: Option<String>,
    pub is_copy: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct L2ItemWrite {
    pub id: String,
    pub item_type: String,
    pub data: Vec<u8>,
    pub owner_id: Option<String>,
    pub visibility: String,
    pub group_id: Option<String>,
    pub author_id: Option<String>,
    pub key_version: i32,
    pub parent_id: Option<String>,
    pub is_copy: bool,
}

/// Lightweight header for sync protocol.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ItemHeader {
    pub item_id: String,
    pub item_type: String,
    pub checksum: String,
    pub updated_at: String,
    pub author_id: String,
    pub is_deletion: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupRow {
    pub id: String,
    pub name: String,
    pub culture: String,
    pub security_policy: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupMemberRow {
    pub group_id: String,
    pub entity_id: String,
    pub role: String,
    pub posture: Option<String>,
    pub joined_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccessLogEntry {
    pub entity_id: String,
    pub action: String,
    pub resource_type: String,
    pub resource_id: Option<String>,
    pub group_id: Option<String>,
    pub detail: Option<String>,
}

/// Storage trait for the P2P layer.
pub trait Storage: Send + Sync {
    fn read_l1(&self, user_id: &str) -> Result<Option<Vec<u8>>>;
    fn write_l1(&self, user_id: &str, data: &[u8]) -> Result<()>;

    fn read_l2_item(&self, id: &str) -> Result<Option<L2ItemRow>>;
    fn write_l2_item(&self, item: &L2ItemWrite) -> Result<()>;
    fn delete_l2_item(&self, id: &str) -> Result<bool>;
    fn read_l2_item_meta(&self, id: &str) -> Result<Option<L2ItemMeta>>;
    fn list_group_items(
        &self,
        group_id: &str,
        since: Option<&str>,
        limit: u32,
    ) -> Result<Vec<ItemHeader>>;

    fn write_group(&self, id: &str, name: &str, culture: &str, security_policy: &str) -> Result<()>;
    fn read_group(&self, id: &str) -> Result<Option<GroupRow>>;
    fn list_groups(&self) -> Result<Vec<GroupRow>>;
    fn list_members(&self, group_id: &str) -> Result<Vec<GroupMemberRow>>;
    fn get_membership(&self, group_id: &str, entity_id: &str) -> Result<Option<GroupMemberRow>>;

    fn log_access(&self, entry: &AccessLogEntry) -> Result<()>;

    // L2 index (encrypted blob, singleton)
    fn read_l2_index(&self) -> Result<Option<Vec<u8>>>;
    fn write_l2_index(&self, data: &[u8]) -> Result<()>;

    // FTS search (used by API search endpoint)
    fn fts_search(&self, query: &str, limit: u32) -> Result<Vec<String>>;
}

/// SQLite-backed storage.
/// Connection wrapped in Mutex for Send + Sync (rusqlite Connection is !Sync).
pub struct SqliteStorage {
    conn: Mutex<Connection>,
    #[allow(dead_code)]
    db_path: PathBuf,
}

impl SqliteStorage {
    fn db(&self) -> Result<std::sync::MutexGuard<'_, Connection>> {
        self.conn.lock().map_err(|_| StorageError::LockPoisoned)
    }

    /// Open (or create) the database at `db_path`.
    /// Sets WAL mode and busy_timeout for concurrent access with the TS process.
    pub fn open(db_path: &Path) -> Result<Self> {
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let conn = Connection::open(db_path)?;
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA busy_timeout = 5000;
             PRAGMA foreign_keys = ON;",
        )?;

        let storage = Self {
            conn: Mutex::new(conn),
            db_path: db_path.to_path_buf(),
        };

        storage.ensure_schema()?;
        Ok(storage)
    }

    /// Open in read-only mode (for testing with existing DBs).
    pub fn open_readonly(db_path: &Path) -> Result<Self> {
        let conn = Connection::open_with_flags(
            db_path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        conn.execute_batch("PRAGMA busy_timeout = 5000;")?;

        Ok(Self {
            conn: Mutex::new(conn),
            db_path: db_path.to_path_buf(),
        })
    }

    /// Create a new database with schema v4 (for testing).
    pub fn create_new(db_path: &Path) -> Result<Self> {
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let conn = Connection::open(db_path)?;
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA busy_timeout = 5000;
             PRAGMA foreign_keys = ON;",
        )?;
        conn.execute_batch(include_str!("schema_v4.sql"))?;

        Ok(Self {
            conn: Mutex::new(conn),
            db_path: db_path.to_path_buf(),
        })
    }

    fn ensure_schema(&self) -> Result<()> {
        let conn = self.db()?;
        let table_exists: bool = conn.query_row(
            "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='schema_version'",
            [],
            |row| row.get(0),
        )?;

        if !table_exists {
            // Empty database -- initialize schema v4
            conn.execute_batch(include_str!("schema_v4.sql"))?;
            return Ok(());
        }

        let version: u32 = conn
            .query_row("SELECT version FROM schema_version LIMIT 1", [], |row| {
                row.get(0)
            })
            .optional()?
            .unwrap_or(0);

        if version < 4 {
            return Err(StorageError::SchemaVersionMismatch {
                expected: 4,
                found: version,
            });
        }

        Ok(())
    }

    /// Compute SHA-256 checksum for data.
    fn checksum(data: &[u8]) -> String {
        let mut hasher = Sha256::new();
        hasher.update(data);
        hex::encode(hasher.finalize())
    }
}

impl Storage for SqliteStorage {
    fn read_l1(&self, user_id: &str) -> Result<Option<Vec<u8>>> {
        let conn = self.db()?;
        let result = conn
            .query_row(
                "SELECT data FROM l1_hot WHERE user_id = ?1",
                params![user_id],
                |row| row.get::<_, Vec<u8>>(0),
            )
            .optional()?;
        Ok(result)
    }

    fn write_l1(&self, user_id: &str, data: &[u8]) -> Result<()> {
        let conn = self.db()?;
        conn.execute(
            "INSERT INTO l1_hot (user_id, data, updated_at)
             VALUES (?1, ?2, datetime('now'))
             ON CONFLICT(user_id) DO UPDATE SET
               data = excluded.data,
               updated_at = datetime('now')",
            params![user_id, data],
        )?;
        Ok(())
    }

    fn read_l2_item(&self, id: &str) -> Result<Option<L2ItemRow>> {
        let conn = self.db()?;
        let result = conn
            .query_row(
                "SELECT id, type, owner_id, visibility, data, checksum,
                        group_id, author_id, key_version, parent_id, is_copy,
                        access_count, last_accessed_at, created_at, updated_at
                 FROM l2_items WHERE id = ?1",
                params![id],
                |row| {
                    Ok(L2ItemRow {
                        id: row.get(0)?,
                        item_type: row.get(1)?,
                        owner_id: row.get(2)?,
                        visibility: row.get(3)?,
                        data: row.get(4)?,
                        checksum: row.get(5)?,
                        group_id: row.get(6)?,
                        author_id: row.get(7)?,
                        key_version: row.get::<_, Option<i32>>(8)?.unwrap_or(1),
                        parent_id: row.get(9)?,
                        is_copy: row.get::<_, Option<i32>>(10)?.unwrap_or(0) != 0,
                        access_count: row.get::<_, Option<i64>>(11)?.unwrap_or(0),
                        last_accessed_at: row.get(12)?,
                        created_at: row.get(13)?,
                        updated_at: row.get(14)?,
                    })
                },
            )
            .optional()?;
        Ok(result)
    }

    fn write_l2_item(&self, item: &L2ItemWrite) -> Result<()> {
        let checksum = Self::checksum(&item.data);
        let conn = self.db()?;
        conn.execute(
            "INSERT INTO l2_items (id, type, owner_id, visibility, data, checksum,
                                   group_id, author_id, key_version, parent_id, is_copy, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, datetime('now'))
             ON CONFLICT(id) DO UPDATE SET
               type = excluded.type,
               owner_id = excluded.owner_id,
               visibility = excluded.visibility,
               data = excluded.data,
               checksum = excluded.checksum,
               group_id = excluded.group_id,
               author_id = excluded.author_id,
               key_version = excluded.key_version,
               parent_id = excluded.parent_id,
               is_copy = excluded.is_copy,
               updated_at = datetime('now')",
            params![
                item.id,
                item.item_type,
                item.owner_id,
                item.visibility,
                item.data,
                checksum,
                item.group_id,
                item.author_id,
                item.key_version,
                item.parent_id,
                if item.is_copy { 1 } else { 0 },
            ],
        )?;
        Ok(())
    }

    fn delete_l2_item(&self, id: &str) -> Result<bool> {
        let conn = self.db()?;
        let changes = conn.execute("DELETE FROM l2_items WHERE id = ?1", params![id])?;
        Ok(changes > 0)
    }

    fn read_l2_item_meta(&self, id: &str) -> Result<Option<L2ItemMeta>> {
        let conn = self.db()?;
        let result = conn
            .query_row(
                "SELECT owner_id, visibility, group_id, author_id, key_version, parent_id, is_copy
                 FROM l2_items WHERE id = ?1",
                params![id],
                |row| {
                    Ok(L2ItemMeta {
                        owner_id: row.get(0)?,
                        visibility: row.get(1)?,
                        group_id: row.get(2)?,
                        author_id: row.get(3)?,
                        key_version: row.get::<_, Option<i32>>(4)?.unwrap_or(1),
                        parent_id: row.get(5)?,
                        is_copy: row.get::<_, Option<i32>>(6)?.unwrap_or(0) != 0,
                    })
                },
            )
            .optional()?;
        Ok(result)
    }

    fn list_group_items(
        &self,
        group_id: &str,
        since: Option<&str>,
        limit: u32,
    ) -> Result<Vec<ItemHeader>> {
        let conn = self.db()?;
        let map_row = |row: &rusqlite::Row| {
            Ok(ItemHeader {
                item_id: row.get(0)?,
                item_type: row.get(1)?,
                checksum: row.get::<_, Option<String>>(2)?.unwrap_or_default(),
                updated_at: row.get(3)?,
                author_id: row.get::<_, Option<String>>(4)?.unwrap_or_default(),
                is_deletion: row.get::<_, i32>(5)? != 0,
            })
        };

        if let Some(since_ts) = since {
            let mut stmt = conn.prepare(
                "SELECT id, type, checksum, updated_at, author_id, 0
                 FROM l2_items
                 WHERE group_id = ?1 AND updated_at > ?2
                 ORDER BY updated_at ASC
                 LIMIT ?3",
            )?;
            let rows = stmt
                .query_map(params![group_id, since_ts, limit], map_row)?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            Ok(rows)
        } else {
            let mut stmt = conn.prepare(
                "SELECT id, type, checksum, updated_at, author_id, 0
                 FROM l2_items
                 WHERE group_id = ?1
                 ORDER BY updated_at ASC
                 LIMIT ?2",
            )?;
            let rows = stmt
                .query_map(params![group_id, limit], map_row)?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            Ok(rows)
        }
    }

    fn write_group(&self, id: &str, name: &str, culture: &str, security_policy: &str) -> Result<()> {
        let conn = self.db()?;
        conn.execute(
            "INSERT INTO groups (id, name, culture, security_policy, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, datetime('now'), datetime('now'))
             ON CONFLICT(id) DO UPDATE SET
               name = excluded.name,
               culture = excluded.culture,
               security_policy = excluded.security_policy,
               updated_at = datetime('now')",
            params![id, name, culture, security_policy],
        )?;
        Ok(())
    }

    fn read_group(&self, id: &str) -> Result<Option<GroupRow>> {
        let conn = self.db()?;
        let result = conn
            .query_row(
                "SELECT id, name, culture, security_policy, created_at, updated_at
                 FROM groups WHERE id = ?1",
                params![id],
                |row| {
                    Ok(GroupRow {
                        id: row.get(0)?,
                        name: row.get(1)?,
                        culture: row.get(2)?,
                        security_policy: row.get(3)?,
                        created_at: row.get(4)?,
                        updated_at: row.get(5)?,
                    })
                },
            )
            .optional()?;
        Ok(result)
    }

    fn list_groups(&self) -> Result<Vec<GroupRow>> {
        let conn = self.db()?;
        let mut stmt = conn.prepare(
            "SELECT id, name, culture, security_policy, created_at, updated_at
             FROM groups ORDER BY name",
        )?;
        let rows = stmt
            .query_map([], |row| {
                Ok(GroupRow {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    culture: row.get(2)?,
                    security_policy: row.get(3)?,
                    created_at: row.get(4)?,
                    updated_at: row.get(5)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    fn list_members(&self, group_id: &str) -> Result<Vec<GroupMemberRow>> {
        let conn = self.db()?;
        let mut stmt = conn.prepare(
            "SELECT group_id, entity_id, role, posture, joined_at
             FROM group_members WHERE group_id = ?1 ORDER BY entity_id",
        )?;
        let rows = stmt
            .query_map(params![group_id], |row| {
                Ok(GroupMemberRow {
                    group_id: row.get(0)?,
                    entity_id: row.get(1)?,
                    role: row.get(2)?,
                    posture: row.get(3)?,
                    joined_at: row.get(4)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    fn get_membership(&self, group_id: &str, entity_id: &str) -> Result<Option<GroupMemberRow>> {
        let conn = self.db()?;
        let result = conn
            .query_row(
                "SELECT group_id, entity_id, role, posture, joined_at
                 FROM group_members WHERE group_id = ?1 AND entity_id = ?2",
                params![group_id, entity_id],
                |row| {
                    Ok(GroupMemberRow {
                        group_id: row.get(0)?,
                        entity_id: row.get(1)?,
                        role: row.get(2)?,
                        posture: row.get(3)?,
                        joined_at: row.get(4)?,
                    })
                },
            )
            .optional()?;
        Ok(result)
    }

    fn log_access(&self, entry: &AccessLogEntry) -> Result<()> {
        let conn = self.db()?;
        conn.execute(
            "INSERT INTO access_log (entity_id, action, resource_type, resource_id, group_id, detail)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                entry.entity_id,
                entry.action,
                entry.resource_type,
                entry.resource_id,
                entry.group_id,
                entry.detail,
            ],
        )?;
        Ok(())
    }

    fn read_l2_index(&self) -> Result<Option<Vec<u8>>> {
        let conn = self.db()?;
        let result = conn
            .query_row(
                "SELECT data FROM l2_index WHERE id = 1",
                [],
                |row| row.get::<_, Vec<u8>>(0),
            )
            .optional()?;
        Ok(result)
    }

    fn write_l2_index(&self, data: &[u8]) -> Result<()> {
        let conn = self.db()?;
        conn.execute(
            "INSERT INTO l2_index (id, data, updated_at)
             VALUES (1, ?1, datetime('now'))
             ON CONFLICT(id) DO UPDATE SET
               data = excluded.data,
               updated_at = datetime('now')",
            params![data],
        )?;
        Ok(())
    }

    fn fts_search(&self, query: &str, limit: u32) -> Result<Vec<String>> {
        let trimmed = query.trim();
        if trimmed.is_empty() {
            return Ok(vec![]);
        }

        let safe_query: String = trimmed
            .split_whitespace()
            .filter(|t| !t.is_empty())
            .map(|t| format!("\"{}\"", t.replace('"', "")))
            .collect::<Vec<_>>()
            .join(" ");

        if safe_query.is_empty() {
            return Ok(vec![]);
        }

        let conn = self.db()?;
        let mut stmt = conn.prepare(
            "SELECT item_id FROM l2_fts WHERE l2_fts MATCH ?1 ORDER BY rank LIMIT ?2",
        )?;

        let ids = stmt
            .query_map(params![safe_query, limit], |row| row.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(ids)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_db() -> (tempfile::TempDir, SqliteStorage) {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let storage = SqliteStorage::create_new(&db_path).unwrap();
        (dir, storage)
    }

    #[test]
    fn test_l1_read_write() {
        let (_dir, storage) = test_db();

        assert!(storage.read_l1("test-user").unwrap().is_none());

        storage.write_l1("test-user", b"hello").unwrap();
        let data = storage.read_l1("test-user").unwrap().unwrap();
        assert_eq!(data, b"hello");

        storage.write_l1("test-user", b"updated").unwrap();
        let data = storage.read_l1("test-user").unwrap().unwrap();
        assert_eq!(data, b"updated");
    }

    #[test]
    fn test_l2_item_crud() {
        let (_dir, storage) = test_db();

        let item = L2ItemWrite {
            id: "test-item-1".into(),
            item_type: "entity".into(),
            data: b"encrypted-blob".to_vec(),
            owner_id: Some("russell".into()),
            visibility: "private".into(),
            group_id: None,
            author_id: Some("russell".into()),
            key_version: 1,
            parent_id: None,
            is_copy: false,
        };

        storage.write_l2_item(&item).unwrap();
        let row = storage.read_l2_item("test-item-1").unwrap().unwrap();
        assert_eq!(row.id, "test-item-1");
        assert_eq!(row.item_type, "entity");
        assert_eq!(row.data, b"encrypted-blob");
        assert!(row.checksum.is_some());

        assert!(storage.delete_l2_item("test-item-1").unwrap());
        assert!(storage.read_l2_item("test-item-1").unwrap().is_none());
    }

    #[test]
    fn test_groups_and_members() {
        let (_dir, storage) = test_db();

        storage.write_l1("russell", b"{}").unwrap();
        storage.write_l1("martin", b"{}").unwrap();

        {
            let conn = storage.db().unwrap();
            conn.execute(
                "INSERT INTO groups (id, name, culture, security_policy) VALUES (?1, ?2, ?3, ?4)",
                params!["team-1", "Team One", "{}", "{}"],
            ).unwrap();
            conn.execute(
                "INSERT INTO group_members (group_id, entity_id, role) VALUES (?1, ?2, ?3)",
                params!["team-1", "russell", "owner"],
            ).unwrap();
            conn.execute(
                "INSERT INTO group_members (group_id, entity_id, role) VALUES (?1, ?2, ?3)",
                params!["team-1", "martin", "member"],
            ).unwrap();
        }

        let group = storage.read_group("team-1").unwrap().unwrap();
        assert_eq!(group.name, "Team One");

        let members = storage.list_members("team-1").unwrap();
        assert_eq!(members.len(), 2);

        let membership = storage.get_membership("team-1", "russell").unwrap().unwrap();
        assert_eq!(membership.role, "owner");
    }

    #[test]
    fn test_list_group_items() {
        let (_dir, storage) = test_db();

        let item = L2ItemWrite {
            id: "grp-item-1".into(),
            item_type: "learning".into(),
            data: b"blob".to_vec(),
            owner_id: Some("russell".into()),
            visibility: "group".into(),
            group_id: Some("seed-drill".into()),
            author_id: Some("russell".into()),
            key_version: 1,
            parent_id: None,
            is_copy: false,
        };

        storage.write_l2_item(&item).unwrap();

        let headers = storage.list_group_items("seed-drill", None, 100).unwrap();
        assert_eq!(headers.len(), 1);
        assert_eq!(headers[0].item_id, "grp-item-1");
    }

    #[test]
    fn test_access_log() {
        let (_dir, storage) = test_db();

        storage
            .log_access(&AccessLogEntry {
                entity_id: "russell".into(),
                action: "read".into(),
                resource_type: "l2_item".into(),
                resource_id: Some("item-1".into()),
                group_id: None,
                detail: None,
            })
            .unwrap();

        let conn = storage.db().unwrap();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM access_log", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn test_write_group() {
        let (_dir, storage) = test_db();

        storage
            .write_group("grp-1", "Test Group", r#"{"broadcast_eagerness":"chatty"}"#, "{}")
            .unwrap();

        let group = storage.read_group("grp-1").unwrap().unwrap();
        assert_eq!(group.name, "Test Group");
        assert!(group.culture.contains("chatty"));

        // Upsert: update name
        storage
            .write_group("grp-1", "Updated Name", r#"{"broadcast_eagerness":"moderate"}"#, "{}")
            .unwrap();
        let group2 = storage.read_group("grp-1").unwrap().unwrap();
        assert_eq!(group2.name, "Updated Name");

        let groups = storage.list_groups().unwrap();
        assert_eq!(groups.len(), 1);
    }

    #[test]
    fn test_checksum_computed_on_write() {
        let (_dir, storage) = test_db();

        let data = b"test data for checksum";
        let item = L2ItemWrite {
            id: "chk-1".into(),
            item_type: "entity".into(),
            data: data.to_vec(),
            owner_id: None,
            visibility: "private".into(),
            group_id: None,
            author_id: None,
            key_version: 1,
            parent_id: None,
            is_copy: false,
        };

        storage.write_l2_item(&item).unwrap();
        let row = storage.read_l2_item("chk-1").unwrap().unwrap();

        let expected = SqliteStorage::checksum(data);
        assert_eq!(row.checksum.unwrap(), expected);
    }
}
