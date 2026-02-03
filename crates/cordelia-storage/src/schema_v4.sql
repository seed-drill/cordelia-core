-- Cordelia schema v4 -- matches TypeScript implementation exactly.
-- Used by SqliteStorage::create_new() for testing.

CREATE TABLE IF NOT EXISTS l1_hot (
  user_id TEXT PRIMARY KEY,
  data BLOB NOT NULL,
  updated_at TEXT NOT NULL DEFAULT (datetime('now')),
  created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS l2_items (
  id TEXT PRIMARY KEY,
  type TEXT NOT NULL CHECK(type IN ('entity', 'session', 'learning')),
  owner_id TEXT,
  visibility TEXT NOT NULL DEFAULT 'private'
    CHECK(visibility IN ('private', 'group', 'public')),
  data BLOB NOT NULL,
  last_accessed_at TEXT,
  access_count INTEGER NOT NULL DEFAULT 0,
  checksum TEXT,
  group_id TEXT,
  author_id TEXT,
  key_version INTEGER DEFAULT 1,
  parent_id TEXT,
  is_copy INTEGER DEFAULT 0,
  created_at TEXT NOT NULL DEFAULT (datetime('now')),
  updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS l2_index (
  id INTEGER PRIMARY KEY CHECK(id = 1),
  data BLOB NOT NULL,
  updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS audit (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  ts TEXT NOT NULL DEFAULT (datetime('now')),
  entry TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS schema_version (
  version INTEGER NOT NULL,
  migrated_at TEXT NOT NULL DEFAULT (datetime('now'))
);

INSERT INTO schema_version (version) VALUES (4);

-- FTS5
CREATE VIRTUAL TABLE IF NOT EXISTS l2_fts USING fts5(
  item_id UNINDEXED,
  name,
  content,
  tags,
  tokenize = 'porter unicode61'
);

CREATE TABLE IF NOT EXISTS embedding_cache (
  content_hash TEXT NOT NULL,
  provider TEXT NOT NULL,
  model TEXT NOT NULL,
  dimensions INTEGER NOT NULL,
  vector BLOB NOT NULL,
  created_at TEXT NOT NULL DEFAULT (datetime('now')),
  PRIMARY KEY (content_hash, provider, model)
);

-- Integrity canary
CREATE TABLE IF NOT EXISTS integrity_canary (
  id INTEGER PRIMARY KEY CHECK(id = 1),
  value TEXT NOT NULL,
  written_at TEXT NOT NULL DEFAULT (datetime('now'))
);

-- Groups
CREATE TABLE IF NOT EXISTS groups (
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  culture TEXT NOT NULL DEFAULT '{}',
  security_policy TEXT NOT NULL DEFAULT '{}',
  created_at TEXT NOT NULL DEFAULT (datetime('now')),
  updated_at TEXT NOT NULL DEFAULT (datetime('now')),
  owner_id TEXT,
  owner_pubkey TEXT,
  signature TEXT
);

CREATE TABLE IF NOT EXISTS group_members (
  group_id TEXT NOT NULL,
  entity_id TEXT NOT NULL,
  role TEXT NOT NULL DEFAULT 'member'
    CHECK(role IN ('owner', 'admin', 'member', 'viewer')),
  posture TEXT DEFAULT 'active'
    CHECK(posture IN ('active', 'silent', 'emcon')),
  joined_at TEXT NOT NULL DEFAULT (datetime('now')),
  PRIMARY KEY (group_id, entity_id),
  FOREIGN KEY (group_id) REFERENCES groups(id),
  FOREIGN KEY (entity_id) REFERENCES l1_hot(user_id)
);

CREATE TABLE IF NOT EXISTS access_log (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  ts TEXT NOT NULL DEFAULT (datetime('now')),
  entity_id TEXT NOT NULL,
  action TEXT NOT NULL,
  resource_type TEXT NOT NULL,
  resource_id TEXT,
  group_id TEXT,
  detail TEXT
);

-- Indexes
CREATE INDEX IF NOT EXISTS idx_l2_items_group ON l2_items(group_id) WHERE group_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_l2_items_parent ON l2_items(parent_id) WHERE parent_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_l2_items_author ON l2_items(author_id) WHERE author_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_access_log_entity ON access_log(entity_id);
CREATE INDEX IF NOT EXISTS idx_access_log_group ON access_log(group_id) WHERE group_id IS NOT NULL;
