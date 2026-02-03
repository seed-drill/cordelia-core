-- Cordelia schema v5 migration from v4.
-- Adds devices table for portal enrollment flow.

CREATE TABLE IF NOT EXISTS devices (
    device_id TEXT PRIMARY KEY,
    entity_id TEXT NOT NULL,
    device_name TEXT,
    device_type TEXT NOT NULL DEFAULT 'node'
        CHECK(device_type IN ('node', 'browser', 'mobile')),
    auth_token_hash TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    last_seen_at TEXT,
    revoked_at TEXT
);

CREATE INDEX IF NOT EXISTS idx_devices_entity ON devices(entity_id);

UPDATE schema_version SET version = 5, migrated_at = datetime('now') WHERE version = 4;
