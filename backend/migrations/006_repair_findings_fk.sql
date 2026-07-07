-- Repair build_findings.build_id so its foreign key references `builds`
-- rather than the dangling `builds_old` left behind by migration 004.
--
-- Background: migration 004 renamed builds → builds_old, created a new
-- builds table, then dropped builds_old.  SQLite's ALTER TABLE RENAME
-- rewrites foreign-key references in *other* tables to follow the rename,
-- so build_findings.build_id ended up referencing the now-dropped
-- builds_old.  Any attempt to insert a finding with foreign_keys enabled
-- fails with "no such table: main.builds_old".  Migration 004 has since
-- been fixed to set legacy_alter_table=ON before the rename, which
-- prevents the rewrite; this migration repairs databases that were
-- migrated by the buggy version.
--
-- The repair rebuilds build_findings from scratch (with the same columns
-- added by migrations 002 and 003) so its FK definition once again points
-- to `builds`.  legacy_alter_table=ON ensures the rebuild's own RENAME
-- doesn't trip the same trap.

PRAGMA legacy_alter_table = ON;
PRAGMA foreign_keys = OFF;

ALTER TABLE build_findings RENAME TO build_findings_old;

CREATE TABLE build_findings (
    id TEXT PRIMARY KEY,
    build_id TEXT NOT NULL REFERENCES builds(id),
    category TEXT NOT NULL,
    description TEXT NOT NULL,
    excerpt TEXT NOT NULL,
    line_number INTEGER,
    severity TEXT NOT NULL DEFAULT 'error',
    finding_class TEXT NOT NULL DEFAULT 'toolchain'
);

INSERT INTO build_findings
    SELECT id, build_id, category, description, excerpt, line_number, severity, finding_class
    FROM build_findings_old;

DROP TABLE build_findings_old;

CREATE INDEX IF NOT EXISTS idx_findings_build    ON build_findings(build_id);
CREATE INDEX IF NOT EXISTS idx_findings_category ON build_findings(category);
CREATE INDEX IF NOT EXISTS idx_findings_class    ON build_findings(finding_class);

PRAGMA legacy_alter_table = OFF;
PRAGMA foreign_keys = ON;
