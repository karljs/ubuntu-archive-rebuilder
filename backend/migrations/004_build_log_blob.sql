-- build_log TEXT to BLOB (gzip). NULL = no log stored.
--
-- legacy_alter_table=ON is essential: without it the RENAME rewrites FK
-- references in other tables (build_findings.build_id to builds_old), and
-- once builds_old is dropped every finding insert fails with
-- "no such table: main.builds_old".
--
-- Pre-migration plain-text rows can be compressed with:
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
