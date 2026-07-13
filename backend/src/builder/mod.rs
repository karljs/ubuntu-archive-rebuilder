//! Build orchestration — runs a batch of package builds sequentially,
//! recording results to the database and handling Ctrl+C gracefully.

mod cgroup;
mod sbuild;
mod source;
mod time_parser;

pub use cgroup::BuildCgroup;
pub use sbuild::{run_sbuild, SbuildConfig};
pub use source::{fetch_source, SourcePackage};
pub use time_parser::parse_time_output;

use crate::analyzer::scan_log;
use crate::db::{self, BatchStats};
use crate::models::{BuildResult, BuildStatus, BuilderBackend, StoreLogs};
use crate::profile::Profile;
use anyhow::{Context, Result};
use flate2::{write::GzEncoder, Compression};
use sqlx::SqlitePool;
use std::io::Write;
use std::path::PathBuf;
use tokio::signal::unix::{signal, SignalKind};
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};
use uuid::Uuid;

/// Parameters for a full batch build run (multiple packages).
pub struct BuildConfig {
    pub profile: Profile,
    /// `(package_name, optional archive component)` for each package to build.
    /// The component is forwarded to the per-build DB row when present, so
    /// results can be sliced by main / universe / etc.
    pub packages: Vec<(String, Option<String>)>,
    pub timeout_seconds: u64,
    pub verbose: bool,
    pub run_tests: bool,
    pub jobs: usize,
    /// Log storage policy.  Defaults to `All` (backward-compatible).
    pub store_logs: StoreLogs,
    /// Base directory for source package downloads.
    /// Defaults to `/var/tmp/rebuild-source` (real disk, not tmpfs).
    pub source_dir: PathBuf,
    /// Target build architecture.  Passed to sbuild as `--arch=<arch>`.
    pub arch: String,
    /// Memory limit for the build cgroup, in MiB.  0 means no limit.
    pub memory_limit_mb: u64,
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
        &config.arch,
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

    // Verify cgroup v2 availability for memory limiting. If unavailable,
    // builds run without protection (graceful degradation).
    if config.memory_limit_mb > 0 {
        match std::fs::read_to_string("/proc/self/cgroup") {
            Ok(content) => {
                let has_v2 = content.lines().any(|l| l.starts_with("0::"));
                if !has_v2 {
                    warn!("Cgroup v2 not available. Builds will run without memory protection.");
                }
            }
            Err(e) => {
                warn!("Cannot read /proc/self/cgroup: {e}. Builds will run without memory protection.");
            }
        }
    }

    let total = config.packages.len();
    for (idx, (package_name, component)) in config.packages.iter().enumerate() {
        if cancel_token.is_cancelled() {
            info!("Batch cancelled, aborting remaining builds");
            break;
        }

        let progress = format!("[{}/{}]", idx + 1, total);
        info!("{progress} Building {package_name}");

        let mut attempt: u32 = 1;
        let mut current_jobs = config.jobs;

        loop {
            match build_package(
                package_name,
                component.as_deref(),
                config,
                current_jobs,
                attempt,
                cancel_token.clone(),
            )
            .await
            {
                Ok(result) => {
                    info!("{progress} {package_name} completed (attempt {attempt}): {}", result.status.as_str());
                    let status = result.status;
                    store_build_result(pool, batch.id, &result, config).await?;

                    // Retry only if OOM-killed on first attempt with jobs > 1.
                    if status == BuildStatus::OomKilled && attempt == 1 && current_jobs > 1 {
                        info!("{progress} {package_name} OOM-killed at {current_jobs} jobs, retrying at 1 job");
                        attempt = 2;
                        current_jobs = 1;
                        continue;
                    }
                    break;
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
                        build_log: format!("Build failed to execute: {e}"),
                        compiler_detected: None,
                        component: component.clone(),
                        jobs: current_jobs,
                        memory_limit_mb: None,
                        attempt_number: attempt,
                    };
                    store_build_result(pool, batch.id, &error_result, config).await?;
                    break;
                }
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
    component: Option<&str>,
    config: &BuildConfig,
    jobs: usize,
    attempt: u32,
    cancel_token: CancellationToken,
) -> Result<BuildResult> {
    // Use a temp dir on real disk (not the RAM-backed /tmp tmpfs) to avoid
    // exhausting RAM with large source tarballs during long build runs.
    std::fs::create_dir_all(&config.source_dir)
        .with_context(|| format!("Failed to create source dir {}", config.source_dir.display()))?;
    let temp_dir = tempfile::Builder::new()
        .tempdir_in(&config.source_dir)
        .context("Failed to create temp directory for source download")?;

    let series = &config.profile.target.series;
    info!(package = %package_name, "Fetching source");
    let source = fetch_source(package_name, series, temp_dir.path()).await?;

    info!(package = %package_name, version = %source.version, "Running sbuild");
    let sbuild_config = SbuildConfig {
        dsc_path: source.dsc_path,
        series: series.clone(),
        arch: config.arch.clone(),
        compiler_type: config.profile.compiler.compiler_type,
        compiler_version: config.profile.compiler.version.clone(),
        build_env: config.profile.build_env_vars(),
        timeout_seconds: config.timeout_seconds,
        verbose: config.verbose,
        run_tests: config.run_tests,
        jobs,
        cancel_token,
        memory_limit_mb: config.memory_limit_mb,
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
        build_log: result.log,
        compiler_detected: result.compiler_detected,
        component: component.map(|s| s.to_string()),
        jobs,
        memory_limit_mb: result.memory_limit_mb,
        attempt_number: attempt,
    })
}

/// Persist a build result: scan for findings, then store according to policy.
///
/// Findings are extracted first (while the log is in memory) so they are always
/// captured regardless of the store-logs policy.  The log is then compressed and
/// stored or dropped based on `config.store_logs`.
async fn store_build_result(
    pool: &SqlitePool,
    batch_id: Uuid,
    result: &BuildResult,
    config: &BuildConfig,
) -> Result<()> {
    let now = chrono::Utc::now();

    // Scan findings first, while the log is still in memory.
    let findings = if result.status.should_scan_for_errors()
        || result.status.should_scan_for_observations()
    {
        scan_log(&result.build_log, result.status)
    } else {
        vec![]
    };

    // Decide whether to store the log based on policy.
    let log_blob: Option<Vec<u8>> = match config.store_logs {
        StoreLogs::None => None,
        StoreLogs::Failures if result.status == BuildStatus::Succeeded => None,
        _ => Some(gzip_compress(result.build_log.as_bytes())?),
    };

    let build = db::insert_build(
        pool,
        &db::NewBuild {
            batch_id,
            source_package: &result.source_package,
            version: &result.version,
            status: result.status,
            build_duration_seconds: result.build_duration_seconds,
            peak_memory_mb: result.peak_memory_mb,
            build_log: log_blob,
            compiler_detected: result.compiler_detected.as_deref(),
            submitted_at: now,
            completed_at: Some(now),
            component: result.component.as_deref(),
            attempt_number: result.attempt_number as i64,
            jobs: Some(result.jobs as i64),
            memory_limit_mb: result.memory_limit_mb.map(|v| v as i64),
        },
    )
    .await?;

    for finding in findings {
        db::insert_finding(
            pool,
            build.id,
            &finding.category,
            &finding.description,
            &finding.excerpt,
            Some(finding.line_number as i64),
            finding.severity,
            finding.class,
        )
        .await?;
    }

    Ok(())
}

/// Gzip-compress bytes, returning the compressed form.
fn gzip_compress(data: &[u8]) -> Result<Vec<u8>> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(data).context("Failed to write to gzip encoder")?;
    encoder.finish().context("Failed to finish gzip compression")
}
