//! Sequential batch orchestration with Ctrl+C handling.

mod cgroup;
mod sbuild;
mod source;
mod time_parser;

pub use sbuild::{run_sbuild, ChrootMode, SbuildConfig};
pub use source::{fetch_source, SourceIndex, SourcePackage};
pub use time_parser::parse_time_output;

use crate::analyzer::scan_log;
use crate::db::{self, BatchStats};
use crate::models::{BuildResult, BuildStatus, BuilderBackend, StoreLogs};
use crate::profile::Profile;
use anyhow::{bail, Context, Result};
use flate2::{write::GzEncoder, Compression};
use sqlx::SqlitePool;
use std::io::Write;
use std::path::PathBuf;
use tokio::signal::unix::{signal, SignalKind};
use tokio::task;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};
use uuid::Uuid;

pub struct BuildConfig {
    pub profile: Profile,
    /// (package, optional component); component lands in the DB row.
    pub packages: Vec<(String, Option<String>)>,
    pub timeout_seconds: u64,
    pub verbose: bool,
    pub run_tests: bool,
    pub jobs: usize,
    pub store_logs: StoreLogs,
    /// Real disk: /tmp is often tmpfs and tarballs are large.
    pub source_dir: PathBuf,
    pub arch: String,
    pub memory_limit_mb: u64,
    pub chroot_mode: ChrootMode,
}

fn find_in_path(name: &str) -> bool {
    if name.contains('/') {
        return PathBuf::from(name).exists();
    }
    std::env::var_os("PATH")
        .map(|paths| std::env::split_paths(&paths).any(|dir| dir.join(name).is_file()))
        .unwrap_or(false)
}

// Without this a missing binary fails 2347 times as opaque exit-127 junk
// instead of once with an actionable message.
fn preflight(chroot_mode: ChrootMode) -> Result<()> {
    if !find_in_path("sbuild") {
        bail!("sbuild not found in PATH; install it: sudo apt install sbuild");
    }
    if !find_in_path("/usr/bin/time") {
        bail!("/usr/bin/time not found; install it: sudo apt install time");
    }
    if chroot_mode == ChrootMode::Unshare && !find_in_path("mmdebstrap") {
        bail!("mmdebstrap not found (sbuild unshare mode needs it): sudo apt install mmdebstrap");
    }
    Ok(())
}

/// Ctrl+C cancels the current build and skips the rest.
pub async fn run_batch(pool: &SqlitePool, config: &BuildConfig) -> Result<(Uuid, BatchStats)> {
    preflight(config.chroot_mode)?;

    let batch =
        db::create_batch(pool, &config.profile, BuilderBackend::Sbuild, &config.arch).await?;

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

    let source_index = {
        let series = config.profile.target.series.clone();
        let arch = config.arch.clone();
        let index = task::spawn_blocking(move || SourceIndex::load(&series, &arch))
            .await
            .context("source index load task panicked")??;
        std::sync::Arc::new(index)
    };

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
                source_index.clone(),
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
                    info!(
                        "{progress} {package_name} completed (attempt {attempt}): {}",
                        result.status.as_str()
                    );
                    let status = result.status;
                    if let Err(e) = store_build_result(pool, batch.id, &result, config).await {
                        error!("{progress} {package_name}: failed to store build result: {e}");
                        break;
                    }

                    if status == BuildStatus::OomKilled && attempt == 1 && current_jobs > 1 {
                        info!("{progress} {package_name} OOM-killed at {current_jobs} jobs, retrying at 1 job");
                        attempt = 2;
                        current_jobs = 1;
                        continue;
                    }
                    break;
                }
                Err(e) => {
                    if e.to_string().contains("Interrupted by user") || cancel_token.is_cancelled()
                    {
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
                    if let Err(se) = store_build_result(pool, batch.id, &error_result, config).await
                    {
                        error!("{progress} {package_name}: failed to store error result: {se}");
                    }
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

async fn build_package(
    source_index: std::sync::Arc<SourceIndex>,
    package_name: &str,
    component: Option<&str>,
    config: &BuildConfig,
    jobs: usize,
    attempt: u32,
    cancel_token: CancellationToken,
) -> Result<BuildResult> {
    // Real disk: /tmp tmpfs + big tarballs = OOM host.
    std::fs::create_dir_all(&config.source_dir).with_context(|| {
        format!(
            "Failed to create source dir {}",
            config.source_dir.display()
        )
    })?;
    let temp_dir = tempfile::Builder::new()
        .tempdir_in(&config.source_dir)
        .context("Failed to create temp directory for source download")?;

    let series = &config.profile.target.series;
    info!(package = %package_name, "Fetching source");
    let source = fetch_source(source_index, package_name, temp_dir.path()).await?;

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
        chroot_mode: config.chroot_mode,
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

/// Findings are extracted before the log is maybe dropped per store-logs.
async fn store_build_result(
    pool: &SqlitePool,
    batch_id: Uuid,
    result: &BuildResult,
    config: &BuildConfig,
) -> Result<()> {
    let now = chrono::Utc::now();

    let findings =
        if result.status.should_scan_for_errors() || result.status.should_scan_for_observations() {
            scan_log(&result.build_log, result.status)
        } else {
            vec![]
        };

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

fn gzip_compress(data: &[u8]) -> Result<Vec<u8>> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder
        .write_all(data)
        .context("Failed to write to gzip encoder")?;
    encoder
        .finish()
        .context("Failed to finish gzip compression")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_in_path_finds_real_binary() {
        assert!(find_in_path("sh"));
        assert!(find_in_path("/usr/bin/time"));
    }

    #[test]
    fn find_in_path_rejects_missing() {
        assert!(!find_in_path("definitely-not-a-real-command-xyz"));
        assert!(!find_in_path("/no/such/path/xyz"));
    }

    #[test]
    fn preflight_passes_on_a_working_machine() {
        preflight(ChrootMode::Unshare).unwrap();
    }
}
