//! Build orchestration — runs a batch of package builds sequentially,
//! recording results to the database and handling Ctrl+C gracefully.

mod sbuild;
mod source;
mod time_parser;

pub use sbuild::{run_sbuild, SbuildConfig};
pub use source::{fetch_source, SourcePackage};
pub use time_parser::parse_time_output;

use crate::analyzer::scan_log;
use crate::db::{self, BatchStats};
use crate::models::{BuildResult, BuildStatus, BuilderBackend};
use crate::profile::Profile;
use anyhow::{Context, Result};
use sqlx::SqlitePool;
use tokio::signal::unix::{signal, SignalKind};
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};
use uuid::Uuid;

/// Parameters for a full batch build run (multiple packages).
pub struct BuildConfig {
    pub profile: Profile,
    pub packages: Vec<String>,
    pub timeout_seconds: u64,
    pub verbose: bool,
    pub run_tests: bool,
    pub jobs: usize,
}

/// Run a batch of builds, recording each result to the database.
///
/// Builds are executed sequentially.  Returns the batch ID and aggregate
/// statistics.  A Ctrl+C during any build cancels the current build and
/// skips all remaining packages.
pub async fn run_batch(
    pool: &SqlitePool,
    config: &BuildConfig,
) -> Result<(Uuid, BatchStats)> {
    let batch = db::create_batch(
        pool,
        &config.profile,
        BuilderBackend::Sbuild,
    )
    .await?;

    info!(
        batch_id = %batch.id,
        batch_name = %batch.name,
        package_count = config.packages.len(),
        "Starting build batch"
    );

    let cancel_token = CancellationToken::new();
    let cancel_signal = cancel_token.clone();
    tokio::spawn(async move {
        let mut sigint = match signal(SignalKind::interrupt()) {
            Ok(s) => s,
            Err(e) => {
                error!("Failed to register SIGINT handler: {e}");
                return;
            }
        };
        sigint.recv().await;
        info!("Received Ctrl+C, cancelling batch...");
        cancel_signal.cancel();
    });

    let total = config.packages.len();
    for (idx, package_name) in config.packages.iter().enumerate() {
        if cancel_token.is_cancelled() {
            info!("Batch cancelled, aborting remaining builds");
            break;
        }

        let progress = format!("[{}/{}]", idx + 1, total);
        info!("{progress} Building {package_name}");

        match build_package(package_name, config, cancel_token.clone()).await {
            Ok(result) => {
                info!("{progress} {package_name} completed: {}", result.status.as_str());
                store_build_result(pool, batch.id, &result).await?;
            }
            Err(e) => {
                if e.to_string().contains("Interrupted by user") || cancel_token.is_cancelled() {
                    info!("Batch interrupted by user, aborting remaining builds");
                    break;
                }
                error!("{progress} {package_name} failed to run: {e}");
                let error_result = BuildResult {
                    source_package: package_name.clone(),
                    version: "unknown".into(),
                    status: BuildStatus::Failed,
                    build_duration_seconds: None,
                    peak_memory_mb: None,
                    disk_usage_mb: None,
                    build_log: format!("Build failed to execute: {e}"),
                    compiler_detected: None,
                };
                store_build_result(pool, batch.id, &error_result).await?;
            }
        }
    }

    db::finish_batch(pool, batch.id).await?;
    let stats = db::get_batch_stats(pool, batch.id).await?;

    info!(
        batch_id = %batch.id,
        total = stats.total,
        succeeded = stats.succeeded,
        failed = stats.failed,
        "Batch completed"
    );

    Ok((batch.id, stats))
}

/// Build a single source package: fetch source, run sbuild, log compiler status.
async fn build_package(
    package_name: &str,
    config: &BuildConfig,
    cancel_token: CancellationToken,
) -> Result<BuildResult> {
    let temp_dir = tempfile::tempdir().context("Failed to create temp directory")?;

    let series = &config.profile.target.series;
    info!(package = %package_name, "Fetching source");
    let source = fetch_source(package_name, series, temp_dir.path()).await?;

    info!(package = %package_name, version = %source.version, "Running sbuild");
    let sbuild_config = SbuildConfig {
        dsc_path: source.dsc_path,
        series: series.clone(),
        compiler_type: config.profile.compiler.compiler_type,
        compiler_version: config.profile.compiler.version.clone(),
        build_env: config.profile.build_env_vars(),
        timeout_seconds: config.timeout_seconds,
        verbose: config.verbose,
        run_tests: config.run_tests,
        jobs: config.jobs,
        cancel_token,
    };

    let result = run_sbuild(&sbuild_config).await?;

    match &result.compiler_detected {
        Some(ci) if ci.contains("confirmed") => {
            info!(package = %package_name, compiler = %ci, "Compiler verified");
        }
        Some(ci) => {
            warn!(package = %package_name, compiler = %ci, "Compiler verification problem");
        }
        None => {
            warn!(package = %package_name, "No compiler detection info");
        }
    }

    Ok(BuildResult {
        source_package: package_name.to_string(),
        version: source.version,
        status: result.status,
        build_duration_seconds: result.duration_seconds,
        peak_memory_mb: result.peak_memory_mb,
        disk_usage_mb: None,
        build_log: result.log,
        compiler_detected: result.compiler_detected,
    })
}

/// Persist a build result and run error-pattern analysis on failures.
async fn store_build_result(
    pool: &SqlitePool,
    batch_id: Uuid,
    result: &BuildResult,
) -> Result<()> {
    let now = chrono::Utc::now();

    let build = db::insert_build(
        pool,
        &db::NewBuild {
            batch_id,
            source_package: &result.source_package,
            version: &result.version,
            status: result.status,
            build_duration_seconds: result.build_duration_seconds,
            peak_memory_mb: result.peak_memory_mb,
            disk_usage_mb: None,
            build_log: Some(&result.build_log),
            compiler_detected: result.compiler_detected.as_deref(),
            submitted_at: now,
            completed_at: Some(now),
        },
    )
    .await?;

    if matches!(result.status, BuildStatus::Failed | BuildStatus::Timeout) {
        for finding in scan_log(&result.build_log) {
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

    Ok(())
}
