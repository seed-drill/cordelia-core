-- Cordelia schema v6 migration from v5.
-- Adds owner signing fields to groups table (R4-030 group descriptor signing).

ALTER TABLE groups ADD COLUMN owner_id TEXT;
ALTER TABLE groups ADD COLUMN owner_pubkey TEXT;
ALTER TABLE groups ADD COLUMN signature TEXT;

UPDATE schema_version SET version = 6, migrated_at = datetime('now') WHERE version = 5;
