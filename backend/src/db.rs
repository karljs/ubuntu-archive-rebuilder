//! Database operations — SQLite storage for batches, builds, and findings.

use crate::models::{BuildStatus, BuilderBackend, FindingSeverity};
pub use crate::models::{Batch, Build, BuildFinding};
use crate::profile::Profile;
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use sqlx::sqlite::{SqlitePool, SqlitePoolOptions};
use sqlx::Row;
use std::path::Path;
use uuid::Uuid;

const SCHEMA: &str = include_str!("../migrations/001_initial.sql");
const MIGRATION_002: &str = include_str!("../migrations/002_findings_severity.sql");

/// Aggregate build counts for a batch.
#[derive(Debug, Default)]
pub struct BatchStats {
    pub total: i64,
    pub pending: i64,
    pub building: i64,
    pub succeeded: i64,
    pub failed: i64,
    pub dep_wait: i64,
    pub timeout: i64,
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
}

/// Parameters for inserting a new build record.
///
/// Uses borrowed strings (`&'a str`) on the insert path to avoid cloning data
/// that is already in a local buffer. The owned `Build` type returned by query
/// functions is a separate struct for the same reason: sqlx rows yield owned
/// `String` values, so the two types serve different lifetimes and cannot
/// easily be unified without unnecessary allocations.
pub struct NewBuild<'a> {
    pub batch_id: Uuid,
    pub source_package: &'a str,
    pub version: &'a str,
    pub status: BuildStatus,
    pub build_duration_seconds: Option<f64>,
    pub peak_memory_mb: Option<i64>,
    pub build_log: Option<&'a str>,
    pub compiler_detected: Option<&'a str>,
    pub submitted_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
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
) -> Result<Batch> {
    let id = Uuid::new_v4();
    let name = profile.batch_name();
    let now = Utc::now();

    sqlx::query(
        "INSERT INTO batches (id, name, compiler_type, compiler_version, series,
                              profile_name, profile_content, builder_backend, started_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(id.to_string())
    .bind(&name)
    .bind(profile.compiler.compiler_type.as_str())
    .bind(&profile.compiler.version)
    .bind(&profile.target.series)
    .bind(&profile.name)
    .bind(&profile.raw_content)
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
        "SELECT id, name, compiler_type, compiler_version, series,
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
        "SELECT id, name, compiler_type, compiler_version, series,
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
        "SELECT id, name, compiler_type, compiler_version, series,
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
    "SELECT id, name, compiler_type, compiler_version, series,
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
            build_log, compiler_detected, submitted_at, completed_at
         ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(id.to_string())
    .bind(b.batch_id.to_string())
    .bind(b.source_package)
    .bind(b.version)
    .bind(b.status.as_str())
    .bind(b.build_duration_seconds)
    .bind(b.peak_memory_mb)
    .bind(b.build_log)
    .bind(b.compiler_detected)
    .bind(b.submitted_at.to_rfc3339())
    .bind(b.completed_at.map(|d| d.to_rfc3339()))
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
        build_log: b.build_log.map(|s| s.to_string()),
        compiler_detected: b.compiler_detected.map(|s| s.to_string()),
        submitted_at: b.submitted_at,
        completed_at: b.completed_at,
    })
}

/// Get all builds for a batch.
pub async fn get_builds_for_batch(pool: &SqlitePool, batch_id: Uuid) -> Result<Vec<Build>> {
    sqlx::query(
        "SELECT id, batch_id, source_package, version, status,
                build_duration_seconds, peak_memory_mb,
                build_log, compiler_detected, submitted_at, completed_at
         FROM builds WHERE batch_id = ? ORDER BY submitted_at",
    )
    .bind(batch_id.to_string())
    .fetch_all(pool)
    .await
    .context("Failed to get builds for batch")?
    .iter()
    .map(build_from_row)
    .collect()
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
        build_log: row.get("build_log"),
        compiler_detected: row.get("compiler_detected"),
        submitted_at: DateTime::parse_from_rfc3339(&submitted_str)?.with_timezone(&Utc),
        completed_at: completed_str
            .map(|s| DateTime::parse_from_rfc3339(&s).map(|d| d.with_timezone(&Utc)))
            .transpose()?,
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
) -> Result<BuildFinding> {
    let id = Uuid::new_v4();

    sqlx::query(
        "INSERT INTO build_findings (id, build_id, category, description, excerpt, line_number, severity)
         VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(id.to_string())
    .bind(build_id.to_string())
    .bind(category)
    .bind(description)
    .bind(excerpt)
    .bind(line_number)
    .bind(severity.as_str())
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
    })
}

/// Get all findings for a build.
pub async fn get_findings_for_build(
    pool: &SqlitePool,
    build_id: Uuid,
) -> Result<Vec<BuildFinding>> {
    sqlx::query(
        "SELECT id, build_id, category, description, excerpt, line_number, severity
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

fn finding_from_row(row: &sqlx::sqlite::SqliteRow) -> Result<BuildFinding> {
    let id_str: String = row.get("id");
    let build_id_str: String = row.get("build_id");
    let severity_str: String = row.get("severity");

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
            _ => {}
        }
    }
    stats.total = stats.pending + stats.building + stats.succeeded
        + stats.failed + stats.dep_wait + stats.timeout;

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
