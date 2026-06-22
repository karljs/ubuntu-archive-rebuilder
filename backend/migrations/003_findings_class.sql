-- Add finding class column to build_findings.
--
-- 'toolchain'     — a genuine compiler/toolchain incompatibility (GCC vs Clang).
-- 'environmental' — an infrastructure / environmental artifact unrelated to the
--                   compiler under test (e.g. parallel `make install` races,
--                   source-fetch failures). These must not count against a
--                   compiler in success-rate comparisons.
--
-- DEFAULT 'toolchain' preserves the meaning of all existing rows; a re-scan
-- repopulates the column from the analyzer pattern definitions.

ALTER TABLE build_findings ADD COLUMN finding_class TEXT NOT NULL DEFAULT 'toolchain';

CREATE INDEX IF NOT EXISTS idx_findings_class ON build_findings(finding_class);
