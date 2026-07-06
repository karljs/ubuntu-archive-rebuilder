-- Track build architecture and archive component so results can be sliced
-- by either dimension.  Architecture is a batch-level property (every build
-- in a batch targets the same arch); component is a per-build property
-- (each source package lives in main / universe / restricted / multiverse).

-- Backfill existing batches with the implicit default they were built with.
ALTER TABLE batches ADD COLUMN arch TEXT NOT NULL DEFAULT 'amd64';

-- Nullable: existing rows and bare-name package lists have no component info.
ALTER TABLE builds ADD COLUMN component TEXT;

CREATE INDEX IF NOT EXISTS idx_batches_arch   ON batches(arch);
CREATE INDEX IF NOT EXISTS idx_builds_component ON builds(component);
