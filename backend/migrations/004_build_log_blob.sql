-- Migrate build_log from TEXT to BLOB (gzip-compressed).
--
-- NULL  = no log stored (dropped by store policy or not yet available).
-- BLOB  = gzip-compressed UTF-8 log bytes.  Always gzip; no encoding column
--         needed because the format is uniform for all rows post-migration.
--
-- After applying this migration, compress existing plain-text rows with:
--
--   python3 -c "
--   import sqlite3, gzip
--   db = sqlite3.connect('rebuilder.db')
--   for row_id, log in db.execute(
--       'SELECT id, CAST(build_log AS TEXT) FROM builds WHERE build_log IS NOT NULL'):
--       db.execute('UPDATE builds SET build_log=? WHERE id=?',
--                  (gzip.compress(log.encode()), row_id))
--   db.execute('VACUUM')
--   db.commit()
--   "

-- Use legacy_alter_table so the RENAME does NOT rewrite foreign-key
-- references in other tables (build_findings.build_id).  Without this,
-- SQLite would repoint build_findings to builds_old, and once builds_old is
-- dropped the FK would dangle — causing "no such table: main.builds_old" on
-- every finding insert.  legacy_alter_table is restored to OFF at the end.
PRAGMA legacy_alter_table = ON;
PRAGMA foreign_keys = OFF;

ALTER TABLE builds RENAME TO builds_old;

CREATE TABLE builds (
    id TEXT PRIMARY KEY,
    batch_id TEXT NOT NULL REFERENCES batches(id),
    source_package TEXT NOT NULL,
    version TEXT NOT NULL,
    status TEXT NOT NULL,
    build_duration_seconds REAL,
    peak_memory_mb INTEGER,
    build_log BLOB,
    compiler_detected TEXT,
    submitted_at TEXT NOT NULL,
    completed_at TEXT,
    UNIQUE(batch_id, source_package)
);

INSERT INTO builds
    SELECT id, batch_id, source_package, version, status,
           build_duration_seconds, peak_memory_mb,
           build_log,
           compiler_detected, submitted_at, completed_at
    FROM builds_old;

DROP TABLE builds_old;

CREATE INDEX IF NOT EXISTS idx_builds_batch   ON builds(batch_id);
CREATE INDEX IF NOT EXISTS idx_builds_status  ON builds(status);
CREATE INDEX IF NOT EXISTS idx_builds_package ON builds(source_package);

PRAGMA legacy_alter_table = OFF;
PRAGMA foreign_keys = ON;
