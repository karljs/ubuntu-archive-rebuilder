-- Repairs build_findings FKs pointing at the dropped builds_old left by
-- the buggy migration 004. See 004 for the legacy_alter_table trap.

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
