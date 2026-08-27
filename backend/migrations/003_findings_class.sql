-- DEFAULT preserves existing rows; a re-scan repopulates from the patterns.

ALTER TABLE build_findings ADD COLUMN finding_class TEXT NOT NULL DEFAULT 'toolchain';

CREATE INDEX IF NOT EXISTS idx_findings_class ON build_findings(finding_class);
