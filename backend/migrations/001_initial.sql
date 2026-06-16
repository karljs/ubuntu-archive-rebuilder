-- Schema for rebuild experiments database.
--
-- Each batch records which compiler profile was used, so results are
-- fully reproducible.  The profile TOML is snapshotted at build time.

CREATE TABLE IF NOT EXISTS batches (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    compiler_type TEXT NOT NULL,
    compiler_version TEXT NOT NULL,
    series TEXT NOT NULL,
    profile_name TEXT NOT NULL,
    profile_content TEXT NOT NULL,
    builder_backend TEXT NOT NULL,
    started_at TEXT NOT NULL,
    finished_at TEXT
);

CREATE INDEX IF NOT EXISTS idx_batches_compiler ON batches(compiler_type, compiler_version);
CREATE INDEX IF NOT EXISTS idx_batches_series ON batches(series);
CREATE INDEX IF NOT EXISTS idx_batches_started ON batches(started_at);

CREATE TABLE IF NOT EXISTS builds (
    id TEXT PRIMARY KEY,
    batch_id TEXT NOT NULL REFERENCES batches(id),
    source_package TEXT NOT NULL,
    version TEXT NOT NULL,
    status TEXT NOT NULL,
    build_duration_seconds REAL,
    peak_memory_mb INTEGER,
    build_log TEXT,
    compiler_detected TEXT,
    submitted_at TEXT NOT NULL,
    completed_at TEXT,

    UNIQUE(batch_id, source_package)
);

CREATE INDEX IF NOT EXISTS idx_builds_batch ON builds(batch_id);
CREATE INDEX IF NOT EXISTS idx_builds_status ON builds(status);
CREATE INDEX IF NOT EXISTS idx_builds_package ON builds(source_package);

CREATE TABLE IF NOT EXISTS build_findings (
    id TEXT PRIMARY KEY,
    build_id TEXT NOT NULL REFERENCES builds(id),
    category TEXT NOT NULL,
    description TEXT NOT NULL,
    excerpt TEXT NOT NULL,
    line_number INTEGER
);

CREATE INDEX IF NOT EXISTS idx_findings_build ON build_findings(build_id);
CREATE INDEX IF NOT EXISTS idx_findings_category ON build_findings(category);

-- binary_metrics is reserved for a future feature that will compare binary
-- sizes and symbol counts across compiler profiles. No Rust code reads or
-- writes this table yet.
CREATE TABLE IF NOT EXISTS binary_metrics (
    id TEXT PRIMARY KEY,
    build_id TEXT NOT NULL REFERENCES builds(id),
    binary_name TEXT NOT NULL,
    deb_package TEXT NOT NULL,
    installed_size_kb INTEGER NOT NULL,
    text_section_bytes INTEGER,
    total_stripped_bytes INTEGER,
    symbol_count INTEGER
);

CREATE INDEX IF NOT EXISTS idx_metrics_build ON binary_metrics(build_id);
