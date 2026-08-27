-- batches.arch and builds.component (nullable for legacy rows).

ALTER TABLE batches ADD COLUMN arch TEXT NOT NULL DEFAULT 'amd64';
ALTER TABLE builds ADD COLUMN component TEXT;

CREATE INDEX IF NOT EXISTS idx_batches_arch      ON batches(arch);
CREATE INDEX IF NOT EXISTS idx_builds_component  ON builds(component);
