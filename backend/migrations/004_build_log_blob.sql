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

PRAGMA foreign_keys = ON;
