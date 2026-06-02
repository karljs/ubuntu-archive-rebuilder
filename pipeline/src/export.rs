//! Export module — generates JSON data files for the report viewer.

use crate::db;
use anyhow::Result;
use serde::Serialize;
use sqlx::SqlitePool;
use std::path::Path;
use tokio::fs;
use tracing::info;
use uuid::Uuid;

/// Summary of a batch for the index.
#[derive(Serialize)]
pub struct BatchSummary {
    pub id: String,
    pub name: String,
    pub compiler_type: String,
    pub compiler_version: String,
    pub series: String,
    pub profile_name: String,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub stats: BatchStats,
}

/// Statistics for a batch.
#[derive(Serialize)]
pub struct BatchStats {
    pub total: i64,
    pub succeeded: i64,
    pub failed: i64,
    pub dep_wait: i64,
    pub timeout: i64,
}

/// Detailed batch data including builds.
#[derive(Serialize)]
pub struct BatchDetail {
    pub id: String,
    pub name: String,
    pub compiler_type: String,
    pub compiler_version: String,
    pub series: String,
    pub profile_name: String,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub builds: Vec<BuildSummary>,
    pub finding_summary: Vec<FindingCount>,
}

/// Summary of a build for the batch detail.
#[derive(Serialize)]
pub struct BuildSummary {
    pub id: String,
    pub package: String,
    pub version: String,
    pub status: String,
    pub duration_seconds: Option<f64>,
    pub peak_memory_mb: Option<i64>,
    pub finding_count: i64,
}

/// Count of findings by category.
#[derive(Serialize)]
pub struct FindingCount {
    pub category: String,
    pub count: i64,
}

/// Detailed build data including findings.
#[derive(Serialize)]
pub struct BuildDetail {
    pub id: String,
    pub batch_id: String,
    pub package: String,
    pub version: String,
    pub status: String,
    pub duration_seconds: Option<f64>,
    pub peak_memory_mb: Option<i64>,
    pub findings: Vec<Finding>,
}

/// A single finding/error from a build.
#[derive(Serialize)]
pub struct Finding {
    pub category: String,
    pub description: String,
    pub excerpt: String,
    pub line_number: Option<i64>,
}

/// Export all data to the output directory.
pub async fn export_data(
    pool: &SqlitePool,
    output_dir: &Path,
    batch_filter: Option<&[Uuid]>,
) -> Result<()> {
    // Create directory structure
    fs::create_dir_all(output_dir).await?;
    fs::create_dir_all(output_dir.join("batches")).await?;
    fs::create_dir_all(output_dir.join("builds")).await?;
    fs::create_dir_all(output_dir.join("logs")).await?;

    // Get all batches (for index.json, always include all for navigation)
    let all_batches = db::list_batches(pool).await?;

    // Build index with summaries
    let mut batch_summaries = Vec::new();
    for batch in &all_batches {
        let stats = db::get_batch_stats(pool, batch.id).await?;
        batch_summaries.push(BatchSummary {
            id: batch.id.to_string(),
            name: batch.name.clone(),
            compiler_type: batch.compiler_type.clone(),
            compiler_version: batch.compiler_version.clone(),
            series: batch.series.clone(),
            profile_name: batch.profile_name.clone(),
            started_at: batch.started_at.to_rfc3339(),
            finished_at: batch.finished_at.map(|t| t.to_rfc3339()),
            stats: BatchStats {
                total: stats.total,
                succeeded: stats.succeeded,
                failed: stats.failed,
                dep_wait: stats.dep_wait,
                timeout: stats.timeout,
            },
        });
    }

    // Write index.json
    let index_json = serde_json::to_string_pretty(&batch_summaries)?;
    fs::write(output_dir.join("index.json"), index_json).await?;
    info!("Wrote index.json with {} batches", batch_summaries.len());

    // Determine which batches to fully export
    let batches_to_export: Vec<_> = if let Some(filter) = batch_filter {
        all_batches
            .into_iter()
            .filter(|b| filter.contains(&b.id))
            .collect()
    } else {
        all_batches
    };

    // Export each batch
    for batch in batches_to_export {
        export_batch(pool, output_dir, &batch).await?;
    }

    Ok(())
}

/// Export a single batch and all its builds.
async fn export_batch(
    pool: &SqlitePool,
    output_dir: &Path,
    batch: &db::Batch,
) -> Result<()> {
    info!(batch_name = %batch.name, "Exporting batch");

    // Get builds for this batch
    let builds = db::get_builds_for_batch(pool, batch.id).await?;

    // Get finding summary
    let finding_stats = db::get_finding_stats(pool, batch.id).await?;

    // Build the batch detail
    let mut build_summaries = Vec::new();
    for build in &builds {
        let finding_count = db::get_finding_count_for_build(pool, build.id).await?;
        build_summaries.push(BuildSummary {
            id: build.id.to_string(),
            package: build.source_package.clone(),
            version: build.version.clone(),
            status: build.status.as_str().to_string(),
            duration_seconds: build.build_duration_seconds,
            peak_memory_mb: build.peak_memory_mb,
            finding_count,
        });
    }

    let batch_detail = BatchDetail {
        id: batch.id.to_string(),
        name: batch.name.clone(),
        compiler_type: batch.compiler_type.clone(),
        compiler_version: batch.compiler_version.clone(),
        series: batch.series.clone(),
        profile_name: batch.profile_name.clone(),
        started_at: batch.started_at.to_rfc3339(),
        finished_at: batch.finished_at.map(|t| t.to_rfc3339()),
        builds: build_summaries,
        finding_summary: finding_stats
            .into_iter()
            .map(|(category, count)| FindingCount { category, count })
            .collect(),
    };

    // Write batch detail
    let batch_json = serde_json::to_string_pretty(&batch_detail)?;
    fs::write(
        output_dir.join("batches").join(format!("{}.json", batch.id)),
        batch_json,
    )
    .await?;

    // Export each build's details and log
    for build in builds {
        export_build(pool, output_dir, &build).await?;
    }

    Ok(())
}

/// Export a single build's details and log.
async fn export_build(
    pool: &SqlitePool,
    output_dir: &Path,
    build: &db::Build,
) -> Result<()> {
    // Get findings for this build
    let findings = db::get_findings_for_build(pool, build.id).await?;

    let build_detail = BuildDetail {
        id: build.id.to_string(),
        batch_id: build.batch_id.to_string(),
        package: build.source_package.clone(),
        version: build.version.clone(),
        status: build.status.as_str().to_string(),
        duration_seconds: build.build_duration_seconds,
        peak_memory_mb: build.peak_memory_mb,
        findings: findings
            .into_iter()
            .map(|f| Finding {
                category: f.category,
                description: f.description,
                excerpt: f.excerpt,
                line_number: f.line_number,
            })
            .collect(),
    };

    // Write build detail
    let build_json = serde_json::to_string_pretty(&build_detail)?;
    fs::write(
        output_dir.join("builds").join(format!("{}.json", build.id)),
        build_json,
    )
    .await?;

    // Write log file if present
    if let Some(ref log) = build.build_log {
        fs::write(
            output_dir.join("logs").join(format!("{}.log", build.id)),
            log,
        )
        .await?;
    }

    Ok(())
}
