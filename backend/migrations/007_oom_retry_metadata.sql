-- Add OOM retry metadata to builds: attempt_number, jobs, memory_limit_mb.
--
-- Relaxes UNIQUE(batch_id, source_package) to UNIQUE(batch_id, source_package,
-- attempt_number) so a package can have multiple build attempts in the same
-- batch (e.g. attempt 1 OOM-killed at 8 jobs, attempt 2 succeeded at 1 job).
--
-- Existing rows get attempt_number=1, jobs=NULL, memory_limit_mb=NULL (legacy
-- builds had no cgroup limit and no retry).
--
-- SQLite cannot ALTER a UNIQUE constraint in place, so this uses the same
-- table-rebuild pattern as migration 004.  legacy_alter_table prevents FK
-- rewrites in build_findings.

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
    component TEXT,
    attempt_number INTEGER NOT NULL DEFAULT 1,
    jobs INTEGER,
    memory_limit_mb INTEGER,
    UNIQUE(batch_id, source_package, attempt_number)
);

INSERT INTO builds
    SELECT id, batch_id, source_package, version, status,
           build_duration_seconds, peak_memory_mb,
           build_log, compiler_detected, submitted_at, completed_at, component,
           1, NULL, NULL
    FROM builds_old;

DROP TABLE builds_old;

CREATE INDEX IF NOT EXISTS idx_builds_batch   ON builds(batch_id);
CREATE INDEX IF NOT EXISTS idx_builds_status  ON builds(status);
CREATE INDEX IF NOT EXISTS idx_builds_package ON builds(source_package);

PRAGMA legacy_alter_table = OFF;
PRAGMA foreign_keys = ON;
