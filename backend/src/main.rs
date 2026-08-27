//! CLI entry point.

use anyhow::{anyhow, bail, Context, Result};
use clap::{Args, Parser, Subcommand};
use rebuilder::{
    builder,
    builder::ChrootMode,
    db, defaults, distro_info, export, fetcher,
    models::StoreLogs,
    profile::{CompilerType, Profile},
};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;
use uuid::Uuid;

#[derive(Parser)]
#[command(
    name = "rebuilder",
    about = "Build Ubuntu archive packages with different compilers and analyse results",
    version
)]
struct Cli {
    /// Database file path.
    #[arg(long, default_value = "rebuilder.db", env = "REBUILD_DB")]
    db: PathBuf,

    /// Verbose output (full sbuild output on stdout).
    #[arg(short, long)]
    verbose: bool,

    #[command(subcommand)]
    command: Commands,
}

/// Flags shared by build commands.
#[derive(Args)]
struct BuildArgs {
    /// Build timeout per package, in seconds.
    #[arg(long, default_value = "14400")]
    timeout: u64,

    /// Parallel make jobs per build (default: CPU count).
    #[arg(short, long)]
    jobs: Option<usize>,

    /// Run package test suites.
    #[arg(long, default_value = "false")]
    run_tests: bool,

    /// Log storage policy: all, failures, or none.
    #[arg(long, default_value = "all")]
    store_logs: StoreLogs,

    /// Base directory for source downloads (real disk, not tmpfs).
    #[arg(long, default_value = "/var/tmp/rebuild-source")]
    source_dir: PathBuf,

    /// Target build architecture (non-amd64/i386 uses ports.ubuntu.com).
    #[arg(long, default_value = "amd64")]
    arch: String,

    /// Per-build cgroup memory limit in MB; 0 disables.
    #[arg(long, default_value = "14336")]
    memory_limit_mb: u64,

    /// Chroot backend: unshare (ephemeral, default) or schroot (persistent).
    #[arg(long, default_value = "unshare", env = "REBUILD_CHROOT_MODE")]
    chroot_mode: ChrootMode,
}

#[derive(Subcommand)]
enum Commands {
    /// Build packages using a compiler profile.
    Build {
        /// Profile TOML file (e.g. profiles/clang-18-noble.toml).
        #[arg(long)]
        profile: PathBuf,

        /// Package list file, one name per line.
        #[arg(long)]
        packages: PathBuf,

        #[command(flatten)]
        build: BuildArgs,
    },

    /// Build the archive's default gcc and clang for each maintained LTS series.
    ///
    /// Series come from distro-info, compiler defaults from the archive's
    /// gcc/clang metapackages, and package lists are fetched fresh from the
    /// archive. Profiles are generated in memory (clang >= 15 gets
    /// -gdwarf-4; dwz cannot process Clang's DWARF5 output).
    RunDefaults {
        /// Series to build (default: maintained LTS releases).
        #[arg(long, value_delimiter = ',')]
        series: Vec<String>,

        /// Compilers to build (default: gcc,clang).
        #[arg(long, value_delimiter = ',')]
        compilers: Vec<String>,

        /// Archive components for the package list (default: main).
        #[arg(long, default_value = "main", value_delimiter = ',')]
        components: Vec<String>,

        /// Package list file; overrides fetching lists from the archive.
        #[arg(long)]
        packages: Option<PathBuf>,

        /// Print the plan without building.
        #[arg(long)]
        dry_run: bool,

        /// Export the frontend database here after building.
        #[arg(long)]
        export_dir: Option<PathBuf>,

        #[command(flatten)]
        build: BuildArgs,
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
        /// Output directory (receives rebuild.db and logs/).
        #[arg(long)]
        output_dir: PathBuf,

        /// Write log files only for this batch (by ID or name).
        #[arg(long)]
        batch: Option<String>,
    },

    /// Re-derive findings for all builds by re-scanning stored logs.
    Rescan {
        /// Re-scan every build in the database.
        #[arg(long)]
        all: bool,
    },

    /// Fetch a source package list from the Ubuntu archive.
    FetchPackages {
        /// Ubuntu series (e.g. noble, jammy).
        #[arg(long)]
        series: String,

        /// Archive components to include, comma-separated.
        #[arg(long, default_value = "main", value_delimiter = ',')]
        components: Vec<String>,

        /// Target architecture (also selects the default mirror).
        #[arg(long, default_value = "amd64")]
        arch: String,

        /// Override the archive mirror base URL.
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

    let verbose = cli.verbose;
    match cli.command {
        Commands::Build {
            profile: profile_path,
            packages,
            build: shared,
        } => {
            let profile = Profile::load(&profile_path)?;

            if shared.chroot_mode == ChrootMode::Unshare {
                profile.validate_series_available()?;
            }

            let package_list = read_package_list(&packages)?;
            if package_list.is_empty() {
                bail!("No packages to build");
            }

            let jobs = default_jobs(shared.jobs);

            info!(
                packages = package_list.len(),
                profile = %profile.name,
                compiler = %profile.compiler.compiler_type,
                version = %profile.compiler.version,
                series = %profile.target.series,
                arch = %shared.arch,
                jobs,
                chroot_mode = %shared.chroot_mode,
                "Starting build run"
            );

            let config = builder::BuildConfig {
                profile,
                packages: package_list,
                timeout_seconds: shared.timeout,
                verbose: cli.verbose,
                run_tests: shared.run_tests,
                jobs,
                store_logs: shared.store_logs,
                source_dir: shared.source_dir,
                arch: shared.arch,
                memory_limit_mb: shared.memory_limit_mb,
                chroot_mode: shared.chroot_mode,
            };

            let (batch_id, stats) = builder::run_batch(&pool, &config).await?;

            println!();
            println!("Batch completed: {batch_id}");
            println!("  Total: {}", stats.total);
            println!(
                "  Succeeded: {} ({:.1}%)",
                stats.succeeded,
                stats.percent(stats.succeeded)
            );
            println!(
                "  Failed: {} ({:.1}%)",
                stats.failed,
                stats.percent(stats.failed)
            );
            println!("  Dep-wait: {}", stats.dep_wait);
            println!("  Timeout: {}", stats.timeout);
            println!("  Oom-killed: {}", stats.oom_killed);
        }

        Commands::RunDefaults {
            series,
            compilers,
            components,
            packages,
            dry_run,
            export_dir,
            build: shared,
        } => {
            run_defaults(
                &pool,
                verbose,
                RunDefaultsArgs {
                    series,
                    compilers,
                    components,
                    packages,
                    dry_run,
                    export_dir,
                    shared,
                },
            )
            .await?;
        }

        Commands::List => {
            let batches = db::list_batches(&pool).await?;
            if batches.is_empty() {
                println!("No batches found.");
            } else {
                println!(
                    "{:<20}  {:<8}  {:<8}  {:<10}  {:<20}",
                    "STARTED", "COMPILER", "VERSION", "SERIES", "NAME"
                );
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
            println!(
                "  Compiler: {} {}",
                batch.compiler_type, batch.compiler_version
            );
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
            println!(
                "  Succeeded: {} ({:.1}%)",
                stats.succeeded,
                stats.percent(stats.succeeded)
            );
            println!(
                "  Failed: {} ({:.1}%)",
                stats.failed,
                stats.percent(stats.failed)
            );
            if stats.environmental > 0 {
                println!("  Environmental (excluded): {}", stats.environmental);
            }
            println!("  Dep-wait: {}", stats.dep_wait);
            println!("  Timeout: {}", stats.timeout);
            if stats.oom_killed > 0 {
                println!("  Oom-killed: {}", stats.oom_killed);
            }

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
            let mirror = url.unwrap_or_else(|| fetcher::default_mirror_for_arch(&arch).to_string());

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

            let mut comp_counts: std::collections::BTreeMap<&str, usize> =
                std::collections::BTreeMap::new();
            for (_, comp) in &packages {
                *comp_counts.entry(comp.as_str()).or_default() += 1;
            }

            let now = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ");
            let comp_str = components.join(", ");
            let mut lines = Vec::with_capacity(packages.len() + 8);
            lines.push("# Ubuntu source package list".to_string());
            lines.push(format!("# Series:     {series}"));
            lines.push(format!("# Components: {comp_str}"));
            lines.push(format!("# Arch:       {arch}"));
            lines.push(format!("# Mirror:     {mirror}"));
            lines.push(format!("# Generated:  {now}"));
            lines.push(format!("# Total:      {}", packages.len()));
            lines.push(String::new());
            for (pkg, comp) in &packages {
                lines.push(format!("{pkg}\t{comp}"));
            }
            lines.push(String::new());

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

type PackageList = Vec<(String, Option<String>)>;

struct RunDefaultsArgs {
    series: Vec<String>,
    compilers: Vec<String>,
    components: Vec<String>,
    packages: Option<PathBuf>,
    dry_run: bool,
    export_dir: Option<PathBuf>,
    shared: BuildArgs,
}

fn default_jobs(jobs: Option<usize>) -> usize {
    jobs.unwrap_or_else(|| {
        std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4)
    })
}

async fn run_defaults(pool: &sqlx::SqlitePool, verbose: bool, args: RunDefaultsArgs) -> Result<()> {
    let shared = &args.shared;

    // gcc first so baselines exist before the clang comparisons.
    let requested = if args.compilers.is_empty() {
        vec!["gcc".to_string(), "clang".to_string()]
    } else {
        args.compilers
    };
    let mut compilers: Vec<CompilerType> = Vec::new();
    for c in requested {
        let parsed: CompilerType = c.parse().map_err(|e: String| anyhow!(e))?;
        if !compilers.contains(&parsed) {
            compilers.push(parsed);
        }
    }
    compilers.sort_by_key(|c| if *c == CompilerType::Gcc { 0 } else { 1 });

    let series_list: Vec<String> = if args.series.is_empty() {
        let csv = std::fs::read_to_string(distro_info::DISTRO_INFO_CSV).context(
            "failed to read distro-info; install the distro-info package or pass --series",
        )?;
        let rows = distro_info::parse_csv(&csv);
        let lts = distro_info::maintained_lts(&rows, chrono::Utc::now().date_naive());
        if lts.is_empty() {
            bail!("no maintained LTS series found; pass --series");
        }
        lts
    } else {
        args.series
    };

    // Package list per series, or one shared list from --packages.
    let mut package_lists: HashMap<String, PackageList> = HashMap::new();
    if let Some(ref path) = args.packages {
        let list = read_package_list(path)?;
        if list.is_empty() {
            bail!("No packages to build");
        }
        for s in &series_list {
            package_lists.insert(s.clone(), list.clone());
        }
    } else {
        for s in &series_list {
            let s2 = s.clone();
            let arch2 = shared.arch.clone();
            let mirror2 = fetcher::default_mirror_for_arch(&shared.arch).to_string();
            let comps2 = args.components.clone();
            let list = tokio::task::spawn_blocking(move || {
                let comp_refs: Vec<&str> = comps2.iter().map(String::as_str).collect();
                fetcher::fetch_package_list(&s2, &comp_refs, &arch2, &mirror2)
            })
            .await
            .context("package list fetch task panicked")??;
            if list.is_empty() {
                bail!(
                    "no packages found for series {s} (components: {})",
                    args.components.join(",")
                );
            }
            let list: PackageList = list
                .into_iter()
                .map(|(name, comp)| (name, Some(comp)))
                .collect();
            package_lists.insert(s.clone(), list);
        }
    }

    let mirror = fetcher::default_mirror_for_arch(&shared.arch).to_string();
    let mut index_cache: defaults::IndexCache = HashMap::new();
    let mut plan: Vec<(Profile, PackageList)> = Vec::new();
    for s in &series_list {
        for c in &compilers {
            let version =
                defaults::default_compiler_version(s, *c, &shared.arch, &mirror, &mut index_cache)
                    .with_context(|| format!("resolving default {} for {s}", c.as_str()))?;
            let profile = defaults::generate_profile(s, *c, &version);
            plan.push((profile, package_lists[s].clone()));
        }
    }

    let total = plan.len();
    println!(
        "Plan: {total} batch(es) across {} series",
        series_list.len()
    );
    for (profile, pkgs) in &plan {
        let mut flags: Vec<&str> = profile.flags.iter().map(|f| f.flag.as_str()).collect();
        flags.sort();
        flags.dedup();
        let flags_str = if flags.is_empty() {
            String::new()
        } else {
            format!("  flags: {}", flags.join(" "))
        };
        println!(
            "  {:<22} {:<6} {:<10} {:>6} pkgs{}",
            profile.name,
            profile.compiler.compiler_type,
            profile.target.series,
            pkgs.len(),
            flags_str
        );
    }
    if args.dry_run {
        println!("[dry-run] nothing built.");
        return Ok(());
    }

    let jobs = default_jobs(shared.jobs);
    let mut failures: Vec<String> = Vec::new();
    for (idx, (profile, pkgs)) in plan.into_iter().enumerate() {
        println!();
        println!(
            "[{}/{}] {} ({} packages)",
            idx + 1,
            total,
            profile.name,
            pkgs.len()
        );
        if shared.chroot_mode == ChrootMode::Unshare {
            profile.validate_series_available()?;
        }
        let config = builder::BuildConfig {
            profile: profile.clone(),
            packages: pkgs,
            timeout_seconds: shared.timeout,
            verbose,
            run_tests: shared.run_tests,
            jobs,
            store_logs: shared.store_logs,
            source_dir: shared.source_dir.clone(),
            arch: shared.arch.clone(),
            memory_limit_mb: shared.memory_limit_mb,
            chroot_mode: shared.chroot_mode,
        };
        match builder::run_batch(pool, &config).await {
            Ok((_id, stats)) => println!(
                "  {} succeeded, {} failed, {} dep-wait, {} timeout, {} oom-killed of {}",
                stats.succeeded,
                stats.failed,
                stats.dep_wait,
                stats.timeout,
                stats.oom_killed,
                stats.total
            ),
            Err(e) => {
                eprintln!("  batch failed: {e:#}");
                failures.push(profile.name);
            }
        }
    }

    if let Some(dir) = &args.export_dir {
        export::export_data(pool, dir, None).await?;
        println!("Exported to {}", dir.display());
    } else {
        println!("Skipping export; pass --export-dir <dir> to export for the frontend.");
    }

    if !failures.is_empty() {
        bail!(
            "{} batch(es) failed: {}",
            failures.len(),
            failures.join(", ")
        );
    }
    Ok(())
}

/// Lines are `name` or `name<TAB>component` (the form fetch-packages writes).
/// Duplicates are dropped: a second insert would violate the batch's UNIQUE
/// constraint and abort the run.
fn read_package_list(path: &Path) -> Result<Vec<(String, Option<String>)>> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read package list: {}", path.display()))?;

    let mut seen = std::collections::HashSet::new();
    let mut list = Vec::new();

    for line in content.lines().map(str::trim) {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        let (name, comp) = line
            .split_once('\t')
            .or_else(|| line.split_once(' '))
            .map(|(name, comp)| {
                let name = name.trim();
                let comp = comp.trim();
                if name.is_empty() {
                    (line, "")
                } else {
                    (name, comp)
                }
            })
            .unwrap_or((line, ""));
        let comp = if comp.is_empty() {
            None
        } else {
            Some(comp.to_string())
        };

        if seen.insert(name.to_string()) {
            list.push((name.to_string(), comp));
        } else {
            warn!("Duplicate package '{name}' in list, keeping first occurrence");
        }
    }

    Ok(list)
}

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
            db::get_batch(pool, uuid).await?.context("Batch not found")
        } else {
            db::get_batch_by_name(pool, s)
                .await?
                .context("Batch not found")
        }
    } else {
        unreachable!("id_or_name is Some, checked above")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_tmp(content: &str) -> std::path::PathBuf {
        let mut f = tempfile::Builder::new().suffix(".txt").tempfile().unwrap();
        f.write_all(content.as_bytes()).unwrap();
        let (_, path) = f.keep().unwrap();
        path
    }

    #[test]
    fn read_package_list_bare_names() {
        let path = write_tmp("# header\nfoo\nbar\n\n  # indented comment\nbaz\n");
        let list = read_package_list(&path).unwrap();
        assert_eq!(
            list,
            vec![
                ("foo".to_string(), None),
                ("bar".to_string(), None),
                ("baz".to_string(), None),
            ]
        );
    }

    #[test]
    fn read_package_list_tab_delimited_component() {
        let path = write_tmp("foo\tmain\nbar\tuniverse\nbaz\trestricted\n");
        let list = read_package_list(&path).unwrap();
        assert_eq!(
            list,
            vec![
                ("foo".to_string(), Some("main".to_string())),
                ("bar".to_string(), Some("universe".to_string())),
                ("baz".to_string(), Some("restricted".to_string())),
            ]
        );
    }

    #[test]
    fn read_package_list_space_delimited_component() {
        let path = write_tmp("foo main\nbar universe\n");
        let list = read_package_list(&path).unwrap();
        assert_eq!(
            list,
            vec![
                ("foo".to_string(), Some("main".to_string())),
                ("bar".to_string(), Some("universe".to_string())),
            ]
        );
    }

    #[test]
    fn read_package_list_mixed_bare_and_component() {
        let path = write_tmp("foo\nbar\tuniverse\nbaz\n");
        let list = read_package_list(&path).unwrap();
        assert_eq!(
            list,
            vec![
                ("foo".to_string(), None),
                ("bar".to_string(), Some("universe".to_string())),
                ("baz".to_string(), None),
            ]
        );
    }

    #[test]
    fn read_package_list_empty_file() {
        let path = write_tmp("# only comments\n\n");
        let list = read_package_list(&path).unwrap();
        assert!(list.is_empty());
    }

    #[test]
    fn read_package_list_trailing_whitespace_in_component_is_trimmed() {
        let path = write_tmp("foo\tmain   \n");
        let list = read_package_list(&path).unwrap();
        assert_eq!(list, vec![("foo".to_string(), Some("main".to_string()))]);
    }

    #[test]
    fn read_package_list_dedups_repeated_names() {
        let path = write_tmp("foo\tmain\nbar\nfoo\tuniverse\nfoo\nbaz\n");
        let list = read_package_list(&path).unwrap();
        assert_eq!(
            list,
            vec![
                ("foo".to_string(), Some("main".to_string())),
                ("bar".to_string(), None),
                ("baz".to_string(), None),
            ]
        );
    }
}
