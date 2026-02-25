-- Cordelia schema v7 migration from v6.
-- Adds 'removed' posture to group_members CHECK constraint (CoW soft-delete).
-- SQLite cannot ALTER CHECK constraints, so we recreate the table.

CREATE TABLE IF NOT EXISTS group_members_new (
  group_id TEXT NOT NULL,
  entity_id TEXT NOT NULL,
  role TEXT NOT NULL DEFAULT 'member'
    CHECK(role IN ('owner', 'admin', 'member', 'viewer')),
  posture TEXT DEFAULT 'active'
    CHECK(posture IN ('active', 'silent', 'emcon', 'removed')),
  joined_at TEXT NOT NULL DEFAULT (datetime('now')),
  PRIMARY KEY (group_id, entity_id),
  FOREIGN KEY (group_id) REFERENCES groups(id),
  FOREIGN KEY (entity_id) REFERENCES l1_hot(user_id)
);

INSERT OR IGNORE INTO group_members_new
  SELECT group_id, entity_id, role, posture, joined_at FROM group_members;

DROP TABLE group_members;
ALTER TABLE group_members_new RENAME TO group_members;

UPDATE schema_version SET version = 7, migrated_at = datetime('now') WHERE version = 6;
