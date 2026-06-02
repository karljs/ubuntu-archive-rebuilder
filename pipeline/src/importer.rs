//! Log importer — imports build logs from external sources.
//!
//! Creates a batch from a profile and imports pre-existing build logs
//! with associated metadata.

use crate::analyzer::{infer_status_from_log, scan_log};
use crate::db;
use crate::models::{BuildStatus, BuilderBackend};
use crate::profile::Profile;
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use sqlx::SqlitePool;
use std::collections::HashMap;
use std::path::Path;
use tracing::{info, warn};
use uuid::Uuid;

/// Metadata for a single build, provided externally.
#[derive(Debug, Clone, Deserialize)]
pub struct BuildMetadata {
    pub source_package: String,
    pub version: String,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub build_duration_seconds: Option<f64>,
    #[serde(default)]
    pub peak_memory_mb: Option<i64>,
    #[serde(default)]
    pub disk_usage_mb: Option<i64>,
    #[serde(default)]
    pub submitted_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub completed_at: Option<DateTime<Utc>>,
}

/// Metadata file format — maps log filenames to build metadata.
#[derive(Debug, Deserialize)]
pub struct MetadataFile {
    /// Map from log filename (without path) to metadata.
    pub builds: HashMap<String, BuildMetadata>,
}

/// Import logs from a directory.
///
/// Creates a new batch from the given profile and imports all logs into it.
/// Requires a metadata file to provide package names, versions, and metrics.
pub async fn import_from_directory(
    pool: &SqlitePool,
    profile: &Profile,
    log_dir: &Path,
    metadata_path: &Path,
) -> Result<(Uuid, ImportStats)> {
    // Load metadata file
    let content = std::fs::read_to_string(metadata_path)
        .context("Failed to read metadata file")?;
    let metadata: MetadataFile =
        serde_json::from_str(&content).context("Failed to parse metadata file")?;

    // Create batch
    let batch = db::create_batch(pool, profile, BuilderBackend::External).await?;

    info!(
        batch_id = %batch.id,
        batch_name = %batch.name,
        "Created batch for import"
    );

    let mut stats = ImportStats::default();

    // Find all log files
    let entries = std::fs::read_dir(log_dir).context("Failed to read log directory")?;

    for entry in entries {
        let entry = entry?;
        let path = entry.path();

        // Skip non-files
        if !path.is_file() {
            continue;
        }

        let filename = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("");

        // Accept .log and .buildlog files
        if !filename.ends_with(".log") && !filename.ends_with(".buildlog") {
            continue;
        }

        match import_single_log(pool, batch.id, &path, filename, &metadata).await {
            Ok(status) => {
                stats.imported += 1;
                match status {
                    BuildStatus::Succeeded => stats.succeeded += 1,
                    BuildStatus::Failed => stats.failed += 1,
                    BuildStatus::DepWait => stats.dep_wait += 1,
                    BuildStatus::Timeout => stats.timeout += 1,
                    _ => {}
                }
            }
            Err(e) => {
                warn!("Failed to import {}: {}", filename, e);
                stats.errors += 1;
            }
        }
    }

    // Mark batch as finished
    db::finish_batch(pool, batch.id).await?;

    info!(
        batch_id = %batch.id,
        imported = stats.imported,
        "Import completed"
    );

    Ok((batch.id, stats))
}

async fn import_single_log(
    pool: &SqlitePool,
    batch_id: Uuid,
    log_path: &Path,
    filename: &str,
    metadata: &MetadataFile,
) -> Result<BuildStatus> {
    // Read log content
    let log_content =
        std::fs::read_to_string(log_path).context("Failed to read log file")?;

    // Get metadata for this log
    let build_meta = metadata
        .builds
        .get(filename)
        .with_context(|| format!("No metadata found for log file: {}", filename))?;

    let explicit_status = build_meta
        .status
        .as_ref()
        .and_then(|s| s.parse().ok());

    // Determine status: use explicit if provided, otherwise infer from log
    let status = explicit_status.unwrap_or_else(|| infer_status_from_log(&log_content));

    let now = Utc::now();
    let submitted_at = build_meta.submitted_at.unwrap_or(now);
    let completed_at = build_meta.completed_at.or(Some(now));

    // Insert build record
    let build = db::insert_build(
        pool,
        &db::NewBuild {
            batch_id,
            source_package: &build_meta.source_package,
            version: &build_meta.version,
            status,
            build_duration_seconds: build_meta.build_duration_seconds,
            peak_memory_mb: build_meta.peak_memory_mb,
            disk_usage_mb: build_meta.disk_usage_mb,
            build_log: Some(&log_content),
            compiler_detected: None,
            submitted_at,
            completed_at,
        },
    )
    .await?;

    info!(
        package = %build_meta.source_package,
        status = %status.as_str(),
        "Imported build"
    );

    // Scan log for error patterns if build failed
    if matches!(status, BuildStatus::Failed | BuildStatus::Timeout) {
        let findings = scan_log(&log_content);
        for finding in findings {
            db::insert_finding(
                pool,
                build.id,
                &finding.category,
                &finding.description,
                &finding.excerpt,
                Some(finding.line_number as i64),
            )
            .await?;
        }
    }

    Ok(status)
}

#[derive(Debug, Default)]
pub struct ImportStats {
    pub imported: usize,
    pub succeeded: usize,
    pub failed: usize,
    pub dep_wait: usize,
    pub timeout: usize,
    pub errors: usize,
}

impl std::fmt::Display for ImportStats {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Imported {} builds: {} succeeded, {} failed, {} dep-wait, {} timeout ({} errors)",
            self.imported, self.succeeded, self.failed, self.dep_wait, self.timeout, self.errors
        )
    }
}
