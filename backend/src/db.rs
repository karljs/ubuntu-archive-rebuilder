//! Database operations — SQLite storage for batches, builds, and findings.

use crate::models::{BuildStatus, BuilderBackend, FindingClass, FindingSeverity};
pub use crate::models::{Batch, Build, BuildFinding};
use crate::profile::Profile;
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use flate2::read::GzDecoder;
use sqlx::sqlite::{SqlitePool, SqlitePoolOptions};
use sqlx::Row;
use std::io::Read;
use std::path::Path;
use uuid::Uuid;

const SCHEMA: &str = include_str!("../migrations/001_initial.sql");
const MIGRATION_002: &str = include_str!("../migrations/002_findings_severity.sql");
const MIGRATION_003: &str = include_str!("../migrations/003_findings_class.sql");
const MIGRATION_004: &str = include_str!("../migrations/004_build_log_blob.sql");
const MIGRATION_005: &str = include_str!("../migrations/005_arch_and_component.sql");
const MIGRATION_006: &str = include_str!("../migrations/006_repair_findings_fk.sql");
const MIGRATION_007: &str = include_str!("../migrations/007_oom_retry_metadata.sql");

/// Aggregate build counts for a batch.
///
/// `environmental` holds builds that failed *only* because of environmental /
/// infrastructure findings (e.g. a parallel-install race), not the toolchain.
/// These are split out of `failed` so they don't count against the compiler in
/// success-rate comparisons. `total` still counts every build, but
/// `comparable_total()` / `toolchain_success_rate()` exclude environmental
/// failures.
#[derive(Debug, Default)]
pub struct BatchStats {
    pub total: i64,
    pub pending: i64,
    pub building: i64,
    pub succeeded: i64,
    pub failed: i64,
    pub dep_wait: i64,
    pub timeout: i64,
    pub oom_killed: i64,
    /// Failed builds whose findings were all environmental (excluded from
    /// compiler comparison). Subset carved out of what would otherwise be `failed`.
    pub environmental: i64,
}

impl BatchStats {
    /// Percentage of `part` relative to `self.total`, or 0 if total is 0.
    pub fn percent(&self, part: i64) -> f64 {
        if self.total == 0 {
            0.0
        } else {
            (part as f64 / self.total as f64) * 100.0
        }
    }

    /// Total builds excluding environmental-only failures — the denominator for
    /// a fair compiler comparison.
    pub fn comparable_total(&self) -> i64 {
        self.total - self.environmental
    }
}

/// Parameters for inserting a new build record.
///
/// Uses borrowed strings (`&'a str`) on the insert path to avoid cloning data
/// that is already in a local buffer. The owned `Build` type returned by query
/// functions is a separate struct for the same reason: sqlx rows yield owned
/// `String` values, so the two types serve different lifetimes and cannot
/// easily be unified without unnecessary allocations.
///
/// `build_log` is gzip-compressed bytes.  `None` means the log was not stored
/// (dropped by the store-logs policy).  The encoding is always gzip; there is
/// no plain-text path for new rows.
pub struct NewBuild<'a> {
    pub batch_id: Uuid,
    pub source_package: &'a str,
    pub version: &'a str,
    pub status: BuildStatus,
    pub build_duration_seconds: Option<f64>,
    pub peak_memory_mb: Option<i64>,
    pub build_log: Option<Vec<u8>>,
    pub compiler_detected: Option<&'a str>,
    pub submitted_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
    /// Archive component (main / universe / restricted / multiverse).
    /// `None` for legacy rows or bare-name package lists.
    pub component: Option<&'a str>,
    pub attempt_number: i64,
    pub jobs: Option<i64>,
    pub memory_limit_mb: Option<i64>,
}

// ---------------------------------------------------------------------------
// Initialization
// ---------------------------------------------------------------------------

/// Open (or create) the database and ensure the schema exists.
pub async fn init(db_path: &Path) -> Result<SqlitePool> {
    let db_url = format!("sqlite:{}?mode=rwc", db_path.display());

    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .connect(&db_url)
        .await
        .context("Failed to connect to database")?;

    sqlx::query(SCHEMA)
        .execute(&pool)
        .await
        .context("Failed to initialize schema")?;

    // Run incremental migrations idempotently.
    // ALTER TABLE … ADD COLUMN fails if the column already exists, so we
    // check first.  This keeps the migration simple without a migrations table.
    let has_severity: bool = sqlx::query_scalar(
        "SELECT COUNT(*) > 0 FROM pragma_table_info('build_findings') WHERE name = 'severity'",
    )
    .fetch_one(&pool)
    .await
    .unwrap_or(false);

    if !has_severity {
        sqlx::query(MIGRATION_002)
            .execute(&pool)
            .await
            .context("Failed to apply migration 002")?;
    }

    let has_class: bool = sqlx::query_scalar(
        "SELECT COUNT(*) > 0 FROM pragma_table_info('build_findings') WHERE name = 'finding_class'",
    )
    .fetch_one(&pool)
    .await
    .unwrap_or(false);

    if !has_class {
        sqlx::query(MIGRATION_003)
            .execute(&pool)
            .await
            .context("Failed to apply migration 003")?;
    }

    // Migration 004: reshape builds.build_log from TEXT to BLOB (gzip).
    // Detect by checking whether the column type has changed; we use the
    // presence of the new BLOB affinity as a proxy — if the column type is
    // still 'TEXT' the migration has not been applied yet.
    let log_col_type: Option<String> = sqlx::query_scalar(
        "SELECT type FROM pragma_table_info('builds') WHERE name = 'build_log'",
    )
    .fetch_optional(&pool)
    .await
    .unwrap_or(None);

    if log_col_type.as_deref() != Some("BLOB") {
        sqlx::query(MIGRATION_004)
            .execute(&pool)
            .await
            .context("Failed to apply migration 004")?;
    }

    // Migration 005: add batches.arch and builds.component.  Use `arch` on
    // batches as the sentinel — both columns are added by the same migration
    // script, so if one exists the other does too.
    let has_arch: bool = sqlx::query_scalar(
        "SELECT COUNT(*) > 0 FROM pragma_table_info('batches') WHERE name = 'arch'",
    )
    .fetch_one(&pool)
    .await
    .unwrap_or(false);

    if !has_arch {
        sqlx::query(MIGRATION_005)
            .execute(&pool)
            .await
            .context("Failed to apply migration 005")?;
    }

    // Migration 006: repair build_findings.build_id foreign keys that were
    // repointed to the (now-dropped) builds_old table by the original
    // migration 004.  Detected by checking where build_findings' FK
    // actually resolves: "builds_old" means the DB was migrated by the
    // buggy 004 and needs the repair; "builds" (or no FK row at all) means
    // the DB is already correct.
    let findings_fk_target: Option<String> = sqlx::query_scalar(
        "SELECT \"table\" FROM pragma_foreign_key_list('build_findings') LIMIT 1",
    )
    .fetch_optional(&pool)
    .await
    .unwrap_or(None);

    if findings_fk_target.as_deref() == Some("builds_old") {
        sqlx::query(MIGRATION_006)
            .execute(&pool)
            .await
            .context("Failed to apply migration 006")?;
    }

    // Migration 007: add attempt_number, jobs, memory_limit_mb to builds
    // and relax UNIQUE constraint to allow retry attempts.  Detected by
    // checking for the attempt_number column.
    let has_attempt_number: bool = sqlx::query_scalar(
        "SELECT COUNT(*) > 0 FROM pragma_table_info('builds') WHERE name = 'attempt_number'",
    )
    .fetch_one(&pool)
    .await
    .unwrap_or(false);

    if !has_attempt_number {
        sqlx::query(MIGRATION_007)
            .execute(&pool)
            .await
            .context("Failed to apply migration 007")?;
    }

    Ok(pool)
}

// ---------------------------------------------------------------------------
// Batches
// ---------------------------------------------------------------------------

/// Create a new batch from a build profile.
pub async fn create_batch(
    pool: &SqlitePool,
    profile: &Profile,
    builder_backend: BuilderBackend,
    arch: &str,
) -> Result<Batch> {
    let id = Uuid::new_v4();
    let name = profile.batch_name();
    let now = Utc::now();

    sqlx::query(
        "INSERT INTO batches (id, name, compiler_type, compiler_version, series, arch,
                              profile_name, profile_content, builder_backend, started_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(id.to_string())
    .bind(&name)
    .bind(profile.compiler.compiler_type.as_str())
    .bind(&profile.compiler.version)
    .bind(&profile.target.series)
    .bind(arch)
    .bind(profile.name.as_str())
    .bind(profile.raw_content.as_str())
    .bind(builder_backend.as_str())
    .bind(now.to_rfc3339())
    .execute(pool)
    .await
    .context("Failed to insert batch")?;

    Ok(Batch {
        id,
        name,
        compiler_type: profile.compiler.compiler_type.as_str().to_string(),
        compiler_version: profile.compiler.version.clone(),
        series: profile.target.series.clone(),
        arch: arch.to_string(),
        profile_name: profile.name.clone(),
        profile_content: profile.raw_content.clone(),
        builder_backend,
        started_at: now,
        finished_at: None,
    })
}

/// Mark a batch as finished.
pub async fn finish_batch(pool: &SqlitePool, batch_id: Uuid) -> Result<()> {
    sqlx::query("UPDATE batches SET finished_at = ? WHERE id = ?")
        .bind(Utc::now().to_rfc3339())
        .bind(batch_id.to_string())
        .execute(pool)
        .await
        .context("Failed to update batch")?;
    Ok(())
}

/// Look up a batch by UUID.
pub async fn get_batch(pool: &SqlitePool, id: Uuid) -> Result<Option<Batch>> {
    sqlx::query(BATCH_SELECT_BY_ID)
        .bind(id.to_string())
        .fetch_optional(pool)
        .await
        .context("Failed to fetch batch")?
        .map(|r| batch_from_row(&r))
        .transpose()
}

/// Look up a batch by name.
pub async fn get_batch_by_name(pool: &SqlitePool, name: &str) -> Result<Option<Batch>> {
    sqlx::query(
        "SELECT id, name, compiler_type, compiler_version, series, arch,
                profile_name, profile_content, builder_backend, started_at, finished_at
         FROM batches WHERE name = ?",
    )
    .bind(name)
    .fetch_optional(pool)
    .await
    .context("Failed to fetch batch")?
    .map(|r| batch_from_row(&r))
    .transpose()
}

/// Get the most recently started batch.
pub async fn get_latest_batch(pool: &SqlitePool) -> Result<Option<Batch>> {
    sqlx::query(
        "SELECT id, name, compiler_type, compiler_version, series, arch,
                profile_name, profile_content, builder_backend, started_at, finished_at
         FROM batches ORDER BY started_at DESC LIMIT 1",
    )
    .fetch_optional(pool)
    .await
    .context("Failed to fetch latest batch")?
    .map(|r| batch_from_row(&r))
    .transpose()
}

/// List all batches, most recent first.
pub async fn list_batches(pool: &SqlitePool) -> Result<Vec<Batch>> {
    sqlx::query(
        "SELECT id, name, compiler_type, compiler_version, series, arch,
                profile_name, profile_content, builder_backend, started_at, finished_at
         FROM batches ORDER BY started_at DESC",
    )
    .fetch_all(pool)
    .await
    .context("Failed to list batches")?
    .iter()
    .map(batch_from_row)
    .collect()
}

const BATCH_SELECT_BY_ID: &str =
    "SELECT id, name, compiler_type, compiler_version, series, arch,
            profile_name, profile_content, builder_backend, started_at, finished_at
     FROM batches WHERE id = ?";

fn batch_from_row(row: &sqlx::sqlite::SqliteRow) -> Result<Batch> {
    let id_str: String = row.get("id");
    let backend_str: String = row.get("builder_backend");
    let started_str: String = row.get("started_at");
    let finished_str: Option<String> = row.get("finished_at");

    Ok(Batch {
        id: Uuid::parse_str(&id_str)?,
        name: row.get("name"),
        compiler_type: row.get("compiler_type"),
        compiler_version: row.get("compiler_version"),
        series: row.get("series"),
        arch: row.get("arch"),
        profile_name: row.get("profile_name"),
        profile_content: row.get("profile_content"),
        builder_backend: backend_str
            .parse()
            .map_err(|e: String| anyhow::anyhow!(e))?,
        started_at: DateTime::parse_from_rfc3339(&started_str)?.with_timezone(&Utc),
        finished_at: finished_str
            .map(|s| DateTime::parse_from_rfc3339(&s).map(|d| d.with_timezone(&Utc)))
            .transpose()?,
    })
}

// ---------------------------------------------------------------------------
// Builds
// ---------------------------------------------------------------------------

/// Insert a new build record.
pub async fn insert_build(pool: &SqlitePool, b: &NewBuild<'_>) -> Result<Build> {
    let id = Uuid::new_v4();

    sqlx::query(
        "INSERT INTO builds (
            id, batch_id, source_package, version, status,
            build_duration_seconds, peak_memory_mb,
            build_log, compiler_detected, submitted_at, completed_at, component,
            attempt_number, jobs, memory_limit_mb
         ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(id.to_string())
    .bind(b.batch_id.to_string())
    .bind(b.source_package)
    .bind(b.version)
    .bind(b.status.as_str())
    .bind(b.build_duration_seconds)
    .bind(b.peak_memory_mb)
    .bind(b.build_log.as_deref())
    .bind(b.compiler_detected)
    .bind(b.submitted_at.to_rfc3339())
    .bind(b.completed_at.map(|d| d.to_rfc3339()))
    .bind(b.component)
    .bind(b.attempt_number)
    .bind(b.jobs)
    .bind(b.memory_limit_mb)
    .execute(pool)
    .await
    .context("Failed to insert build")?;

    Ok(Build {
        id,
        batch_id: b.batch_id,
        source_package: b.source_package.to_string(),
        version: b.version.to_string(),
        status: b.status,
        build_duration_seconds: b.build_duration_seconds,
        peak_memory_mb: b.peak_memory_mb,
        build_log: None,
        compiler_detected: b.compiler_detected.map(|s| s.to_string()),
        submitted_at: b.submitted_at,
        completed_at: b.completed_at,
        component: b.component.map(|s| s.to_string()),
        attempt_number: b.attempt_number,
        jobs: b.jobs,
        memory_limit_mb: b.memory_limit_mb,
    })
}

/// Get all builds for a batch.  The `build_log` field is always `None` here;
/// use `get_build_log()` when the log content is actually needed.
pub async fn get_builds_for_batch(pool: &SqlitePool, batch_id: Uuid) -> Result<Vec<Build>> {
    sqlx::query(
        "SELECT id, batch_id, source_package, version, status,
                build_duration_seconds, peak_memory_mb,
                compiler_detected, submitted_at, completed_at, component,
                attempt_number, jobs, memory_limit_mb
         FROM builds WHERE batch_id = ? ORDER BY source_package, attempt_number",
    )
    .bind(batch_id.to_string())
    .fetch_all(pool)
    .await
    .context("Failed to get builds for batch")?
    .iter()
    .map(build_from_row)
    .collect()
}

/// Fetch all builds across all batches, ordered by submission time.
/// The `build_log` field is always `None`; use `get_build_log()` when needed.
pub async fn list_all_builds(pool: &SqlitePool) -> Result<Vec<Build>> {
    sqlx::query(
        "SELECT id, batch_id, source_package, version, status,
                build_duration_seconds, peak_memory_mb,
                compiler_detected, submitted_at, completed_at, component,
                attempt_number, jobs, memory_limit_mb
         FROM builds ORDER BY submitted_at",
    )
    .fetch_all(pool)
    .await
    .context("Failed to list all builds")?
    .iter()
    .map(build_from_row)
    .collect()
}

/// Fetch and decompress the build log for a single build.
///
/// Returns `None` if no log was stored (dropped by store policy).
/// The stored blob is always gzip-compressed; legacy plain-text rows written
/// before migration 004 are handled by falling back to UTF-8 interpretation
/// if gzip decompression fails.
pub async fn get_build_log(pool: &SqlitePool, build_id: Uuid) -> Result<Option<String>> {
    let row = sqlx::query(
        "SELECT build_log FROM builds WHERE id = ?",
    )
    .bind(build_id.to_string())
    .fetch_optional(pool)
    .await
    .context("Failed to fetch build log")?;

    let Some(row) = row else { return Ok(None) };
    let blob: Option<Vec<u8>> = row.get("build_log");
    let Some(bytes) = blob else { return Ok(None) };

    // Try gzip first; fall back to plain UTF-8 for any pre-migration rows that
    // were not compressed by the one-time migration script.
    let mut gz = GzDecoder::new(&bytes[..]);
    let mut s = String::new();
    if gz.read_to_string(&mut s).is_ok() {
        Ok(Some(s))
    } else {
        Ok(Some(String::from_utf8_lossy(&bytes).into_owned()))
    }
}

fn build_from_row(row: &sqlx::sqlite::SqliteRow) -> Result<Build> {
    let id_str: String = row.get("id");
    let batch_id_str: String = row.get("batch_id");
    let status_str: String = row.get("status");
    let submitted_str: String = row.get("submitted_at");
    let completed_str: Option<String> = row.get("completed_at");

    Ok(Build {
        id: Uuid::parse_str(&id_str)?,
        batch_id: Uuid::parse_str(&batch_id_str)?,
        source_package: row.get("source_package"),
        version: row.get("version"),
        status: status_str
            .parse()
            .map_err(|e: String| anyhow::anyhow!(e))?,
        build_duration_seconds: row.get("build_duration_seconds"),
        peak_memory_mb: row.get("peak_memory_mb"),
        build_log: None,   // not selected; use get_build_log() when needed
        compiler_detected: row.get("compiler_detected"),
        submitted_at: DateTime::parse_from_rfc3339(&submitted_str)?.with_timezone(&Utc),
        completed_at: completed_str
            .map(|s| DateTime::parse_from_rfc3339(&s).map(|d| d.with_timezone(&Utc)))
            .transpose()?,
        component: row.get("component"),
        attempt_number: row.get("attempt_number"),
        jobs: row.get("jobs"),
        memory_limit_mb: row.get("memory_limit_mb"),
    })
}

// ---------------------------------------------------------------------------
// Findings
// ---------------------------------------------------------------------------

/// Insert a build finding.
pub async fn insert_finding(
    pool: &SqlitePool,
    build_id: Uuid,
    category: &str,
    description: &str,
    excerpt: &str,
    line_number: Option<i64>,
    severity: FindingSeverity,
    class: FindingClass,
) -> Result<BuildFinding> {
    let id = Uuid::new_v4();

    sqlx::query(
        "INSERT INTO build_findings (id, build_id, category, description, excerpt, line_number, severity, finding_class)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(id.to_string())
    .bind(build_id.to_string())
    .bind(category)
    .bind(description)
    .bind(excerpt)
    .bind(line_number)
    .bind(severity.as_str())
    .bind(class.as_str())
    .execute(pool)
    .await
    .context("Failed to insert finding")?;

    Ok(BuildFinding {
        id,
        build_id,
        category: category.to_string(),
        description: description.to_string(),
        excerpt: excerpt.to_string(),
        line_number,
        severity,
        class,
    })
}

/// Get all findings for a build.
pub async fn get_findings_for_build(
    pool: &SqlitePool,
    build_id: Uuid,
) -> Result<Vec<BuildFinding>> {
    sqlx::query(
        "SELECT id, build_id, category, description, excerpt, line_number, severity, finding_class
         FROM build_findings WHERE build_id = ? ORDER BY line_number",
    )
    .bind(build_id.to_string())
    .fetch_all(pool)
    .await
    .context("Failed to get findings")?
    .iter()
    .map(finding_from_row)
    .collect()
}

/// Get finding count for a build.
pub async fn get_finding_count_for_build(pool: &SqlitePool, build_id: Uuid) -> Result<i64> {
    let row = sqlx::query("SELECT COUNT(*) as count FROM build_findings WHERE build_id = ?")
        .bind(build_id.to_string())
        .fetch_one(pool)
        .await
        .context("Failed to get finding count")?;
    Ok(row.get("count"))
}

/// Delete all findings for a build. Returns the number of rows removed.
pub async fn delete_findings_for_build(pool: &SqlitePool, build_id: Uuid) -> Result<u64> {
    let result = sqlx::query("DELETE FROM build_findings WHERE build_id = ?")
        .bind(build_id.to_string())
        .execute(pool)
        .await
        .context("Failed to delete findings")?;
    Ok(result.rows_affected())
}

fn finding_from_row(row: &sqlx::sqlite::SqliteRow) -> Result<BuildFinding> {
    let id_str: String = row.get("id");
    let build_id_str: String = row.get("build_id");
    let severity_str: String = row.get("severity");
    let class_str: String = row.get("finding_class");

    Ok(BuildFinding {
        id: Uuid::parse_str(&id_str)?,
        build_id: Uuid::parse_str(&build_id_str)?,
        category: row.get("category"),
        description: row.get("description"),
        excerpt: row.get("excerpt"),
        line_number: row.get("line_number"),
        severity: severity_str
            .parse()
            .map_err(|e: String| anyhow::anyhow!(e))?,
        class: class_str
            .parse()
            .map_err(|e: String| anyhow::anyhow!(e))?,
    })
}

// ---------------------------------------------------------------------------
// Statistics
// ---------------------------------------------------------------------------

/// Get aggregate build counts by status for a batch.
pub async fn get_batch_stats(pool: &SqlitePool, batch_id: Uuid) -> Result<BatchStats> {
    let rows = sqlx::query(
        "SELECT status, COUNT(*) as count FROM builds WHERE batch_id = ? GROUP BY status",
    )
    .bind(batch_id.to_string())
    .fetch_all(pool)
    .await
    .context("Failed to get batch stats")?;

    let mut stats = BatchStats::default();
    for row in rows {
        let status: String = row.get("status");
        let count: i64 = row.get("count");
        match status.as_str() {
            "pending" => stats.pending = count,
            "building" => stats.building = count,
            "succeeded" => stats.succeeded = count,
            "failed" => stats.failed = count,
            "dep_wait" => stats.dep_wait = count,
            "timeout" => stats.timeout = count,
            "oom_killed" => stats.oom_killed = count,
            _ => {}
        }
    }
    stats.total = stats.pending + stats.building + stats.succeeded
        + stats.failed + stats.dep_wait + stats.timeout + stats.oom_killed;

    // Carve out "environmental" failures: failed builds that have at least one
    // finding and whose findings are *all* environmental. These are infra
    // artifacts (e.g. parallel-install races), not toolchain failures, so they
    // are split out of `failed` and excluded from compiler comparison.
    let env_failures: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM builds b
         WHERE b.batch_id = ? AND b.status = 'failed'
           AND EXISTS (SELECT 1 FROM build_findings f WHERE f.build_id = b.id)
           AND NOT EXISTS (
               SELECT 1 FROM build_findings f
               WHERE f.build_id = b.id AND f.finding_class <> 'environmental'
           )",
    )
    .bind(batch_id.to_string())
    .fetch_one(pool)
    .await
    .context("Failed to count environmental failures")?;

    stats.environmental = env_failures;
    stats.failed -= env_failures;

    Ok(stats)
}
/// Get findings grouped by category for a batch.
pub async fn get_finding_stats(pool: &SqlitePool, batch_id: Uuid) -> Result<Vec<(String, i64)>> {
    let rows = sqlx::query(
        "SELECT bf.category, COUNT(*) as count
         FROM build_findings bf
         JOIN builds b ON bf.build_id = b.id
         WHERE b.batch_id = ?
         GROUP BY bf.category
         ORDER BY count DESC",
    )
    .bind(batch_id.to_string())
    .fetch_all(pool)
    .await
    .context("Failed to get finding stats")?;

    Ok(rows
        .iter()
        .map(|row| (row.get("category"), row.get("count")))
        .collect())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::{Compiler, CompilerType, Profile, Target};
    use sqlx::sqlite::SqlitePoolOptions;

    /// Build an in-memory SQLite pool with the full schema + migrations applied.
    ///
    /// Uses `max_connections(1)` because sqlite's `:memory:` is per-connection;
    /// a single connection keeps the schema visible across all queries in the
    /// test.
    async fn mem_pool() -> SqlitePool {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("connect in-memory");
        sqlx::query(SCHEMA).execute(&pool).await.expect("schema");
        sqlx::query(MIGRATION_002).execute(&pool).await.expect("mig 002");
        sqlx::query(MIGRATION_003).execute(&pool).await.expect("mig 003");
        sqlx::query(MIGRATION_004).execute(&pool).await.expect("mig 004");
        sqlx::query(MIGRATION_005).execute(&pool).await.expect("mig 005");
        sqlx::query(MIGRATION_006).execute(&pool).await.expect("mig 006");
        sqlx::query(MIGRATION_007).execute(&pool).await.expect("mig 007");
        pool
    }

    fn sample_profile() -> Profile {
        Profile {
            compiler: Compiler {
                compiler_type: CompilerType::Clang,
                version: "18".to_string(),
            },
            target: Target { series: "noble".to_string() },
            flags: vec![],
            name: "clang-18-noble".to_string(),
            raw_content: String::new(),
        }
    }

    #[tokio::test]
    async fn migration_005_adds_arch_and_component_columns() {
        let pool = mem_pool().await;

        let batch_cols: Vec<String> = sqlx::query_scalar(
            "SELECT name FROM pragma_table_info('batches') ORDER BY name",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert!(batch_cols.contains(&"arch".to_string()), "batches.arch missing");

        let build_cols: Vec<String> = sqlx::query_scalar(
            "SELECT name FROM pragma_table_info('builds') ORDER BY name",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert!(build_cols.contains(&"component".to_string()), "builds.component missing");
    }

    #[tokio::test]
    async fn migration_005_is_idempotent() {
        // Re-running the migration's ALTER TABLE would fail if the columns
        // already exist; the init() guard prevents that. Verify by running
        // the migration twice manually — the second should error, confirming
        // the guard in init() is actually necessary.
        let pool = mem_pool().await;
        let result = sqlx::query(MIGRATION_005).execute(&pool).await;
        assert!(result.is_err(), "second migration 005 run should have failed");
    }

    #[tokio::test]
    async fn create_batch_persists_arch() {
        let pool = mem_pool().await;
        let profile = sample_profile();
        let batch = create_batch(&pool, &profile, BuilderBackend::Sbuild, "arm64")
            .await
            .unwrap();

        assert_eq!(batch.arch, "arm64");

        // Round-trip via every read path.
        let by_id = get_batch(&pool, batch.id).await.unwrap().unwrap();
        assert_eq!(by_id.arch, "arm64");

        let by_name = get_batch_by_name(&pool, &batch.name).await.unwrap().unwrap();
        assert_eq!(by_name.arch, "arm64");

        let latest = get_latest_batch(&pool).await.unwrap().unwrap();
        assert_eq!(latest.arch, "arm64");

        let listed = list_batches(&pool).await.unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].arch, "arm64");
    }

    #[tokio::test]
    async fn create_batch_defaults_arch_to_amd64_at_caller() {
        // The DB column has DEFAULT 'amd64', but create_batch always binds
        // the caller's value explicitly. Verify the caller's "amd64" lands.
        let pool = mem_pool().await;
        let profile = sample_profile();
        let batch = create_batch(&pool, &profile, BuilderBackend::Sbuild, "amd64")
            .await
            .unwrap();
        assert_eq!(batch.arch, "amd64");
    }

    #[tokio::test]
    async fn insert_build_persists_component() {
        let pool = mem_pool().await;
        let profile = sample_profile();
        let batch = create_batch(&pool, &profile, BuilderBackend::Sbuild, "amd64")
            .await
            .unwrap();

        let now = Utc::now();
        let build = insert_build(
            &pool,
            &NewBuild {
                batch_id: batch.id,
                source_package: "foo",
                version: "1.0",
                status: BuildStatus::Succeeded,
                build_duration_seconds: Some(42.0),
                peak_memory_mb: Some(128),
                build_log: None,
                compiler_detected: Some("clang confirmed: 18"),
                submitted_at: now,
                completed_at: Some(now),
                component: Some("universe"),
                attempt_number: 1,
                jobs: None,
                memory_limit_mb: None,
            },
        )
        .await
        .unwrap();

        assert_eq!(build.component.as_deref(), Some("universe"));

        let builds = get_builds_for_batch(&pool, batch.id).await.unwrap();
        assert_eq!(builds.len(), 1);
        assert_eq!(builds[0].component.as_deref(), Some("universe"));

        let all = list_all_builds(&pool).await.unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].component.as_deref(), Some("universe"));
    }

    #[tokio::test]
    async fn insert_build_accepts_null_component() {
        let pool = mem_pool().await;
        let profile = sample_profile();
        let batch = create_batch(&pool, &profile, BuilderBackend::Sbuild, "amd64")
            .await
            .unwrap();

        let now = Utc::now();
        insert_build(
            &pool,
            &NewBuild {
                batch_id: batch.id,
                source_package: "foo",
                version: "1.0",
                status: BuildStatus::Failed,
                build_duration_seconds: None,
                peak_memory_mb: None,
                build_log: None,
                compiler_detected: None,
                submitted_at: now,
                completed_at: Some(now),
                component: None,
                attempt_number: 1,
                jobs: None,
                memory_limit_mb: None,
            },
        )
        .await
        .unwrap();

        let builds = get_builds_for_batch(&pool, batch.id).await.unwrap();
        assert_eq!(builds[0].component, None);
    }

    /// Regression: migration 004 renamed builds → builds_old and created a
    /// new builds table.  SQLite's ALTER TABLE RENAME rewrites FK references
    /// in other tables to follow the rename, so build_findings.build_id
    /// ended up pointing at the (subsequently dropped) builds_old.  Any
    /// finding insert then failed with "no such table: main.builds_old".
    /// Migration 004 now sets legacy_alter_table=ON to prevent the rewrite,
    /// and migration 006 repairs databases already bitten by it.  This test
    /// reproduces the original failure mode by inserting a finding right
    /// after the full migration chain runs.
    #[tokio::test]
    async fn insert_finding_after_migrations_succeeds() {
        let pool = mem_pool().await;
        let profile = sample_profile();
        let batch = create_batch(&pool, &profile, BuilderBackend::Sbuild, "amd64")
            .await
            .unwrap();

        let now = Utc::now();
        let build = insert_build(
            &pool,
            &NewBuild {
                batch_id: batch.id,
                source_package: "foo",
                version: "1.0",
                status: BuildStatus::Failed,
                build_duration_seconds: None,
                peak_memory_mb: None,
                build_log: None,
                compiler_detected: None,
                submitted_at: now,
                completed_at: Some(now),
                component: None,
                attempt_number: 1,
                jobs: None,
                memory_limit_mb: None,
            },
        )
        .await
        .unwrap();

        let finding = insert_finding(
            &pool,
            build.id,
            "missing_header",
            "fatal error: foo.h: No such file or directory",
            "fatal error: foo.h: No such file or directory",
            Some(42),
            FindingSeverity::Error,
            FindingClass::Toolchain,
        )
        .await
        .expect("finding insert should succeed with FK pointing at builds, not builds_old");

        assert_eq!(finding.build_id, build.id);

        let fetched = get_findings_for_build(&pool, build.id).await.unwrap();
        assert_eq!(fetched.len(), 1);
        assert_eq!(fetched[0].category, "missing_header");

        // The FK target must resolve to `builds`, never `builds_old`.
        let fk_target: String = sqlx::query_scalar(
            "SELECT \"table\" FROM pragma_foreign_key_list('build_findings') LIMIT 1",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(fk_target, "builds");
    }

    /// Regression for the user-facing failure: a database migrated by the
    /// original (buggy) migration 004 has `build_findings.build_id` pointing
    /// at the dropped `builds_old` table, so finding inserts fail with
    /// "no such table: main.builds_old".  `init()` must detect and repair
    /// that state via migration 006.  This test stages a broken on-disk
    /// database (applying the buggy 004 sequence by hand) and then runs
    /// `init()` against it.
    #[tokio::test]
    async fn init_repairs_buggy_migration_004_database() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db_path = dir.path().join("rebuilder.db");

        // Stage: schema + migrations 002/003/005, plus the *buggy* 004
        // (no legacy_alter_table, so the rename repoints build_findings'
        // FK at builds_old, which is then dropped).
        let staging = SqlitePoolOptions::new()
            .max_connections(1)
            .connect(&format!("sqlite:{}?mode=rwc", db_path.display()))
            .await
            .unwrap();
        pool_exec(&staging, SCHEMA).await;
        pool_exec(&staging, MIGRATION_002).await;
        pool_exec(&staging, MIGRATION_003).await;
        pool_exec(&staging, MIGRATION_005).await;
        // Buggy 004: same as the fixed migration but WITHOUT
        // legacy_alter_table, so the RENAME rewrites build_findings' FK.
        pool_exec(
            &staging,
            r#"
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
                UNIQUE(batch_id, source_package)
            );
            INSERT INTO builds
                SELECT id, batch_id, source_package, version, status,
                       build_duration_seconds, peak_memory_mb,
                       build_log, compiler_detected, submitted_at, completed_at, component
                FROM builds_old;
            DROP TABLE builds_old;
            PRAGMA foreign_keys = ON;
            "#,
        )
        .await;
        // Sanity: the staged DB is genuinely broken.
        let staged_fk: String = sqlx::query_scalar(
            "SELECT \"table\" FROM pragma_foreign_key_list('build_findings') LIMIT 1",
        )
        .fetch_one(&staging)
        .await
        .unwrap();
        assert_eq!(staged_fk, "builds_old", "test staging is broken");
        staging.close().await;

        // init() should detect the dangling FK and apply migration 006.
        let pool = init(&db_path).await.expect("init repairs broken db");

        let repaired_fk: String = sqlx::query_scalar(
            "SELECT \"table\" FROM pragma_foreign_key_list('build_findings') LIMIT 1",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(repaired_fk, "builds");

        // End-to-end: insert a batch → build → finding must succeed.
        let profile = sample_profile();
        let batch = create_batch(&pool, &profile, BuilderBackend::Sbuild, "amd64")
            .await
            .unwrap();
        let now = Utc::now();
        let build = insert_build(
            &pool,
            &NewBuild {
                batch_id: batch.id,
                source_package: "foo",
                version: "1.0",
                status: BuildStatus::Failed,
                build_duration_seconds: None,
                peak_memory_mb: None,
                build_log: None,
                compiler_detected: None,
                submitted_at: now,
                completed_at: Some(now),
                component: None,
                attempt_number: 1,
                jobs: None,
                memory_limit_mb: None,
            },
        )
        .await
        .unwrap();
        insert_finding(
            &pool,
            build.id,
            "missing_header",
            "fatal error: foo.h",
            "fatal error: foo.h",
            Some(1),
            FindingSeverity::Error,
            FindingClass::Toolchain,
        )
        .await
        .expect("finding insert succeeds after repair");
    }

    #[tokio::test]
    async fn migration_007_adds_oom_retry_columns() {
        let pool = mem_pool().await;

        let build_cols: Vec<String> = sqlx::query_scalar(
            "SELECT name FROM pragma_table_info('builds') ORDER BY name",
        )
        .fetch_all(&pool)
        .await
        .unwrap();

        assert!(build_cols.contains(&"attempt_number".to_string()), "builds.attempt_number missing");
        assert!(build_cols.contains(&"jobs".to_string()), "builds.jobs missing");
        assert!(build_cols.contains(&"memory_limit_mb".to_string()), "builds.memory_limit_mb missing");
    }

    #[tokio::test]
    async fn migration_007_allows_multiple_attempts_for_same_package() {
        let pool = mem_pool().await;
        let profile = sample_profile();
        let batch = create_batch(&pool, &profile, BuilderBackend::Sbuild, "amd64")
            .await
            .unwrap();

        let now = Utc::now();

        // Attempt 1: OOM-killed
        insert_build(
            &pool,
            &NewBuild {
                batch_id: batch.id,
                source_package: "foo",
                version: "1.0",
                status: BuildStatus::OomKilled,
                build_duration_seconds: None,
                peak_memory_mb: Some(14000),
                build_log: None,
                compiler_detected: None,
                submitted_at: now,
                completed_at: Some(now),
                component: Some("main"),
                attempt_number: 1,
                jobs: Some(8),
                memory_limit_mb: Some(14336),
            },
        )
        .await
        .unwrap();

        // Attempt 2: succeeded at jobs=1 — must not violate UNIQUE
        insert_build(
            &pool,
            &NewBuild {
                batch_id: batch.id,
                source_package: "foo",
                version: "1.0",
                status: BuildStatus::Succeeded,
                build_duration_seconds: Some(120.0),
                peak_memory_mb: Some(500),
                build_log: None,
                compiler_detected: Some("clang confirmed: 21"),
                submitted_at: now,
                completed_at: Some(now),
                component: Some("main"),
                attempt_number: 2,
                jobs: Some(1),
                memory_limit_mb: Some(14336),
            },
        )
        .await
        .unwrap();

        let builds = get_builds_for_batch(&pool, batch.id).await.unwrap();
        assert_eq!(builds.len(), 2);
        assert_eq!(builds[0].attempt_number, 1);
        assert_eq!(builds[0].status, BuildStatus::OomKilled);
        assert_eq!(builds[1].attempt_number, 2);
        assert_eq!(builds[1].status, BuildStatus::Succeeded);
    }

    #[tokio::test]
    async fn migration_007_legacy_rows_get_defaults() {
        // Stage a DB with the old schema (no attempt_number/jobs/memory_limit_mb),
        // then run init() to apply migration 007, and verify defaults.
        let dir = tempfile::tempdir().expect("tempdir");
        let db_path = dir.path().join("rebuilder.db");

        let staging = SqlitePoolOptions::new()
            .max_connections(1)
            .connect(&format!("sqlite:{}?mode=rwc", db_path.display()))
            .await
            .unwrap();

        // Stage with the pre-007 schema (no attempt_number/jobs/memory_limit_mb,
        // old UNIQUE constraint).
        pool_exec(
            &staging,
            r#"
            CREATE TABLE batches (
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
                UNIQUE(batch_id, source_package)
            );
            CREATE INDEX IF NOT EXISTS idx_builds_batch ON builds(batch_id);
            CREATE INDEX IF NOT EXISTS idx_builds_status ON builds(status);
            CREATE INDEX IF NOT EXISTS idx_builds_package ON builds(source_package);

            CREATE TABLE build_findings (
                id TEXT PRIMARY KEY,
                build_id TEXT NOT NULL REFERENCES builds(id),
                category TEXT NOT NULL,
                description TEXT NOT NULL,
                excerpt TEXT NOT NULL,
                line_number INTEGER
            );
            CREATE INDEX IF NOT EXISTS idx_findings_build ON build_findings(build_id);
            CREATE INDEX IF NOT EXISTS idx_findings_category ON build_findings(category);
            "#,
        )
        .await;
        pool_exec(&staging, MIGRATION_002).await;
        pool_exec(&staging, MIGRATION_003).await;
        pool_exec(&staging, MIGRATION_004).await;
        pool_exec(&staging, MIGRATION_005).await;
        pool_exec(&staging, MIGRATION_006).await;

        // Insert a legacy build row (old schema, no new columns).
        let profile = sample_profile();
        let batch = create_batch(&staging, &profile, BuilderBackend::Sbuild, "amd64")
            .await
            .unwrap();
        let now = Utc::now();
        sqlx::query(
            "INSERT INTO builds (id, batch_id, source_package, version, status,
             build_duration_seconds, peak_memory_mb, compiler_detected,
             submitted_at, completed_at, component)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(Uuid::new_v4().to_string())
        .bind(batch.id.to_string())
        .bind("legacy-pkg")
        .bind("1.0")
        .bind("succeeded")
        .bind(42.0)
        .bind(128)
        .bind(None::<String>)
        .bind(now.to_rfc3339())
        .bind(now.to_rfc3339())
        .bind::<Option<String>>(None)
        .execute(&staging)
        .await
        .unwrap();
        staging.close().await;

        // Run init() — should apply migration 007.
        let pool = init(&db_path).await.expect("init applies migration 007");

        // Verify the legacy row got defaults.
        let row = sqlx::query(
            "SELECT attempt_number, jobs, memory_limit_mb FROM builds WHERE source_package = 'legacy-pkg'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        let attempt: i64 = row.get("attempt_number");
        let jobs: Option<i64> = row.get("jobs");
        let mem_limit: Option<i64> = row.get("memory_limit_mb");
        assert_eq!(attempt, 1);
        assert_eq!(jobs, None);
        assert_eq!(mem_limit, None);
    }

    async fn pool_exec(pool: &SqlitePool, sql: &str) {
        sqlx::query(sql).execute(pool).await.expect("exec");
    }
}
