-- Add severity column to build_findings.
--
-- 'error'       — finding on a failed build; represents a cause of the failure.
-- 'observation' — finding on a succeeded build; the build completed despite the
--                 issue, but the issue is worth noting (e.g. ignored compiler
--                 flags, GCC-specific warnings Clang doesn't recognise).
--
-- DEFAULT 'error' preserves the meaning of all existing rows.

ALTER TABLE build_findings ADD COLUMN severity TEXT NOT NULL DEFAULT 'error';
