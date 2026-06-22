//! Ubuntu Archive Rebuilder — CLI entry point.

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use rebuilder::{builder, db, export, fetcher, models::StoreLogs, profile::Profile};
use std::path::{Path, PathBuf};
use tracing::info;
use tracing_subscriber::EnvFilter;
use uuid::Uuid;

#[derive(Parser)]
#[command(
    name = "rebuilder",
    about = "Ubuntu archive rebuilder — build packages with different compilers and analyse results",
    version
)]
struct Cli {
    /// Database file path.
    #[arg(long, default_value = "rebuilder.db", env = "REBUILD_DB")]
    db: PathBuf,

    /// Enable verbose output (includes full sbuild output on stdout).
    #[arg(short, long)]
    verbose: bool,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Build packages using a compiler profile.
    Build {
        /// Path to a profile TOML file (e.g. profiles/clang-18-noble.toml).
        #[arg(long)]
        profile: PathBuf,

        /// File containing package names to build (one per line).
        #[arg(long)]
        packages: PathBuf,

        /// Build timeout per package, in seconds.
        #[arg(long, default_value = "14400")]
        timeout: u64,

        /// Parallel make jobs per build (default: CPU count).
        #[arg(short, long)]
        jobs: Option<usize>,

        /// Run package test suites (default: skip tests).
        #[arg(long, default_value = "false")]
        run_tests: bool,

        /// Log storage policy:
        ///   all      — compress and store every build log (default).
        ///   failures — store only failed/timeout/dep_wait logs; succeeded logs
        ///              are scanned for findings then discarded.
        ///   none     — scan for findings then discard all logs.
        #[arg(long, default_value = "all")]
        store_logs: StoreLogs,

        /// Base directory for downloaded source packages.
        /// Defaults to /var/tmp/rebuild-source (real disk, not RAM tmpfs).
        #[arg(long, default_value = "/var/tmp/rebuild-source")]
        source_dir: PathBuf,
    },

    /// List all batches.
    List,

    /// Show batch status and statistics.
    Status {
        /// Batch ID or name.
        #[arg(long, group = "selector")]
        id: Option<String>,

        /// Show the most recent batch.
        #[arg(long, group = "selector")]
        latest: bool,
    },

    /// Export data for the report viewer.
    Export {
        /// Output directory for the export (receives rebuild.db and logs/).
        #[arg(long)]
        output_dir: PathBuf,

        /// Write log files only for this batch (by ID or name).
        /// The exported database always contains all batches.
        #[arg(long)]
        batch: Option<String>,
    },

    /// Re-derive findings for all builds by re-scanning their stored build logs.
    ///
    /// Deletes existing findings and regenerates them with the current analyzer
    /// patterns. Useful after fixing or adding error/observation patterns.
    Rescan {
        /// Re-scan every build in the database.
        #[arg(long)]
        all: bool,
    },

    /// Fetch a list of source packages from the Ubuntu archive.
    ///
    /// Downloads and parses the Sources.gz index for the given series and
    /// components, filters by target architecture, and writes a package list
    /// file ready to pass to `rebuilder build --packages`.
    ///
    /// Example:
    ///   rebuilder fetch-packages --series noble --output packages-noble.txt
    ///   rebuilder fetch-packages --series noble --components main,universe \
    ///       --arch arm64 --output packages-noble-arm64.txt
    FetchPackages {
        /// Ubuntu series to fetch packages for (e.g. noble, jammy).
        #[arg(long)]
        series: String,

        /// Archive components to include, comma-separated.
        #[arg(long, default_value = "main", value_delimiter = ',')]
        components: Vec<String>,

        /// Target build architecture.  Packages that cannot build on this
        /// architecture are excluded.  Defaults to amd64.
        ///
        /// Also controls the default mirror: amd64 and i386 use
        /// archive.ubuntu.com; all other architectures use
        /// ports.ubuntu.com.  Override with --url if needed.
        #[arg(long, default_value = "amd64")]
        arch: String,

        /// Override the archive mirror base URL.  Defaults to the standard
        /// mirror for the chosen architecture.
        #[arg(long)]
        url: Option<String>,

        /// Output file to write package names to.
        #[arg(long)]
        output: PathBuf,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    let filter = if cli.verbose {
        EnvFilter::new("debug")
    } else {
        EnvFilter::new("info")
    };
    tracing_subscriber::fmt().with_env_filter(filter).init();

    let pool = db::init(&cli.db)
        .await
        .context("Failed to initialise database")?;

    match cli.command {
        Commands::Build {
            profile: profile_path,
            packages,
            timeout,
            jobs,
            run_tests,
            store_logs,
            source_dir,
        } => {
            let profile = Profile::load(&profile_path)?;
            profile.validate_series_available()?;

            let package_list = read_package_list(&packages)?;
            if package_list.is_empty() {
                bail!("No packages to build");
            }

            let jobs = jobs.unwrap_or_else(|| {
                std::thread::available_parallelism()
                    .map(|n| n.get())
                    .unwrap_or(4)
            });

            info!(
                packages = package_list.len(),
                profile = %profile.name,
                compiler = %profile.compiler.compiler_type,
                version = %profile.compiler.version,
                series = %profile.target.series,
                jobs,
                "Starting build run"
            );

            let config = builder::BuildConfig {
                profile,
                packages: package_list,
                timeout_seconds: timeout,
                verbose: cli.verbose,
                run_tests,
                jobs,
                store_logs,
                source_dir,
            };

            let (batch_id, stats) = builder::run_batch(&pool, &config).await?;

            println!();
            println!("Batch completed: {batch_id}");
            println!("  Total: {}", stats.total);
            println!("  Succeeded: {} ({:.1}%)", stats.succeeded, stats.percent(stats.succeeded));
            println!("  Failed: {} ({:.1}%)", stats.failed, stats.percent(stats.failed));
            println!("  Dep-wait: {}", stats.dep_wait);
            println!("  Timeout: {}", stats.timeout);
        }

        Commands::List => {
            let batches = db::list_batches(&pool).await?;
            if batches.is_empty() {
                println!("No batches found.");
            } else {
                println!("{:<20}  {:<8}  {:<8}  {:<10}  {:<20}", "STARTED", "COMPILER", "VERSION", "SERIES", "NAME");
                println!("{}", "-".repeat(75));
                for b in batches {
                    println!(
                        "{:<20}  {:<8}  {:<8}  {:<10}  {:<20}",
                        b.started_at.format("%Y-%m-%d %H:%M:%S"),
                        b.compiler_type,
                        b.compiler_version,
                        b.series,
                        b.name,
                    );
                }
            }
        }

        Commands::Status { id, latest } => {
            let batch = resolve_batch(&pool, id.as_deref(), latest).await?;
            let stats = db::get_batch_stats(&pool, batch.id).await?;
            let findings = db::get_finding_stats(&pool, batch.id).await?;

            println!("Batch: {}", batch.name);
            println!("  ID: {}", batch.id);
            println!("  Compiler: {} {}", batch.compiler_type, batch.compiler_version);
            println!("  Series: {}", batch.series);
            println!("  Profile: {}", batch.profile_name);
            println!("  Backend: {}", batch.builder_backend);
            println!("  Started: {}", batch.started_at);
            if let Some(finished) = batch.finished_at {
                println!("  Finished: {finished}");
            }

            println!();
            println!("Build Status:");
            println!("  Total: {}", stats.total);
            println!("  Succeeded: {} ({:.1}%)", stats.succeeded, stats.percent(stats.succeeded));
            println!("  Failed: {} ({:.1}%)", stats.failed, stats.percent(stats.failed));
            if stats.environmental > 0 {
                println!("  Environmental (excluded): {}", stats.environmental);
            }
            println!("  Dep-wait: {}", stats.dep_wait);
            println!("  Timeout: {}", stats.timeout);

            if !findings.is_empty() {
                println!();
                println!("Error Categories:");
                for (category, count) in findings.iter().take(15) {
                    println!("  {category}: {count}");
                }
                if findings.len() > 15 {
                    println!("  ... and {} more categories", findings.len() - 15);
                }
            }
        }

        Commands::Export { output_dir, batch } => {
            let batch_filter = match batch {
                Some(ref s) => {
                    let b = resolve_batch(&pool, Some(s), false).await?;
                    Some(vec![b.id])
                }
                None => None,
            };

            export::export_data(&pool, &output_dir, batch_filter.as_deref()).await?;
            info!(output_dir = %output_dir.display(), "Export complete");
            println!("Exported data to {}", output_dir.display());
        }

        Commands::Rescan { all } => {
            if !all {
                bail!("rescan requires --all");
            }

            let builds = db::list_all_builds(&pool).await?;
            info!(builds = builds.len(), "Re-scanning build logs");

            let mut scanned = 0usize;
            let mut skipped = 0usize;
            let mut findings_before = 0u64;
            let mut findings_after = 0u64;

            for build in &builds {
                // Fetch the log separately — list_all_builds intentionally
                // omits log content to avoid loading gigabytes into memory.
                let Some(log) = db::get_build_log(&pool, build.id).await? else {
                    skipped += 1;
                    continue;
                };
                if log.is_empty() {
                    skipped += 1;
                    continue;
                }

                findings_before += db::delete_findings_for_build(&pool, build.id).await?;

                let findings = rebuilder::analyzer::scan_log(&log, build.status);
                for finding in &findings {
                    db::insert_finding(
                        &pool,
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
                findings_after += findings.len() as u64;
                scanned += 1;
            }

            println!();
            println!("Rescan complete:");
            println!("  Builds scanned: {scanned}");
            println!("  Builds skipped (no log): {skipped}");
            println!("  Findings before: {findings_before}");
            println!("  Findings after:  {findings_after}");
        }

        Commands::FetchPackages {
            series,
            components,
            arch,
            url,
            output,
        } => {
            let mirror = url.unwrap_or_else(|| {
                fetcher::default_mirror_for_arch(&arch).to_string()
            });

            // ureq is synchronous; run it off the async executor.
            let series2 = series.clone();
            let arch2 = arch.clone();
            let mirror2 = mirror.clone();
            let components2 = components.clone();
            let packages = tokio::task::spawn_blocking(move || {
                let components_ref: Vec<&str> = components2.iter().map(String::as_str).collect();
                fetcher::fetch_package_list(&series2, &components_ref, &arch2, &mirror2)
            })
            .await
            .context("fetch task panicked")??;

            // Build per-component counts for the summary.
            let mut comp_counts: std::collections::BTreeMap<&str, usize> =
                std::collections::BTreeMap::new();
            for (_, comp) in &packages {
                *comp_counts.entry(comp.as_str()).or_default() += 1;
            }

            // Write output file with a comment header.
            let now = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ");
            let comp_str = components.join(", ");
            let mut lines = Vec::with_capacity(packages.len() + 8);
            lines.push(format!("# Ubuntu source package list"));
            lines.push(format!("# Series:     {series}"));
            lines.push(format!("# Components: {comp_str}"));
            lines.push(format!("# Arch:       {arch}"));
            lines.push(format!("# Mirror:     {mirror}"));
            lines.push(format!("# Generated:  {now}"));
            lines.push(format!("# Total:      {}", packages.len()));
            lines.push(String::new());
            for (pkg, _comp) in &packages {
                lines.push(pkg.clone());
            }
            lines.push(String::new()); // trailing newline

            std::fs::write(&output, lines.join("\n"))
                .with_context(|| format!("Failed to write {}", output.display()))?;

            println!("Fetched {} source packages:", packages.len());
            for (comp, count) in &comp_counts {
                println!("  {comp}: {count}");
            }
            println!("Written to {}", output.display());
        }
    }

    Ok(())
}

/// Read package names from a file, one per line.  Blank lines and `#` comments
/// are skipped.
fn read_package_list(path: &Path) -> Result<Vec<String>> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read package list: {}", path.display()))?;

    Ok(content
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(str::to_string)
        .collect())
}

/// Resolve a batch from an optional ID/name string, or fall back to the latest.
async fn resolve_batch(
    pool: &sqlx::SqlitePool,
    id_or_name: Option<&str>,
    latest: bool,
) -> Result<db::Batch> {
    if latest || id_or_name.is_none() {
        return db::get_latest_batch(pool)
            .await?
            .context("No batches found");
    }

    if let Some(s) = id_or_name {
        if let Ok(uuid) = Uuid::parse_str(s) {
            db::get_batch(pool, uuid)
                .await?
                .context("Batch not found")
        } else {
            db::get_batch_by_name(pool, s)
                .await?
                .context("Batch not found")
        }
    } else {
        unreachable!("id_or_name is Some, checked above")
    }
}
