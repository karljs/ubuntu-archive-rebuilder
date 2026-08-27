-- DEFAULT preserves existing rows.

ALTER TABLE build_findings ADD COLUMN severity TEXT NOT NULL DEFAULT 'error';
