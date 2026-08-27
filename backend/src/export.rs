//! Frontend export: a stripped rebuild.db (build_log nulled) plus
//! logs/<build-id>.log files, a profile_configs table, and export_meta
//! (series release order) so the frontend needs no hardcoded knowledge.

use anyhow::{Context, Result};
use flate2::read::GzDecoder;
use serde::Deserialize;
use sqlx::sqlite::SqlitePoolOptions;
use sqlx::{Row, SqlitePool};
use std::cmp::Ordering;
use std::collections::BTreeSet;
use std::io::Read;
use std::path::Path;
use tokio::fs;
use tracing::{info, warn};
use uuid::Uuid;

// Only [[flags]] is needed; a separate struct stays forward-compatible with
// Profile's deny_unknown_fields.
#[derive(Deserialize)]
struct ProfileForExport {
    #[serde(default)]
    flags: Vec<FlagForExport>,
}

#[derive(Deserialize)]
struct FlagForExport {
    var: String,
    flag: String,
    reason: String,
}

/// batch_filter limits which batches get log files; None writes all. The
/// exported database always contains every batch.
pub async fn export_data(
    pool: &SqlitePool,
    output_dir: &Path,
    batch_filter: Option<&[Uuid]>,
) -> Result<()> {
    fs::create_dir_all(output_dir).await?;
    fs::create_dir_all(output_dir.join("logs")).await?;

    write_logs(pool, output_dir, batch_filter).await?;

    let db_path = output_dir.join("rebuild.db");
    if db_path.exists() {
        fs::remove_file(&db_path).await?;
    }
    let db_path_str = db_path.to_string_lossy();
    let escaped_path = db_path_str.replace('\'', "''");
    sqlx::query(&format!("VACUUM INTO '{escaped_path}'"))
        .execute(pool)
        .await
        .context("Failed to create export database")?;

    let export_pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect(&format!("sqlite:{db_path_str}"))
        .await
        .context("Failed to open export database")?;

    sqlx::query("UPDATE builds SET build_log = NULL")
        .execute(&export_pool)
        .await
        .context("Failed to strip build logs")?;

    sqlx::query("VACUUM")
        .execute(&export_pool)
        .await
        .context("Failed to compact export database")?;

    write_profile_configs(&export_pool).await?;
    write_export_meta(&export_pool, Path::new(DISTRO_INFO_CSV)).await?;

    export_pool.close().await;

    info!(path = %db_path.display(), "Wrote export database");
    Ok(())
}

// Authoritative Ubuntu release order; rows are in release order and the
// file covers future series. Ships with distro-info (ubuntu-dev-tools dep).
const DISTRO_INFO_CSV: &str = "/usr/share/distro-info/ubuntu.csv";

// export_meta.series_order: series present in the data, in release order.
// Series unknown to the CSV (typos, PPAs) append by first-seen started_at.
async fn write_export_meta(pool: &SqlitePool, distro_info: &Path) -> Result<()> {
    let csv = match fs::read_to_string(distro_info).await {
        Ok(c) => c,
        Err(e) => {
            warn!("Cannot read {} ({e}); series_order omitted, frontend falls back to first-seen order", distro_info.display());
            return Ok(());
        }
    };
    let known = parse_series_column(&csv);
    if known.is_empty() {
        warn!(
            "{} has no usable series column; series_order omitted",
            distro_info.display()
        );
        return Ok(());
    }

    let rows = sqlx::query("SELECT series, MIN(started_at) AS first FROM batches GROUP BY series")
        .fetch_all(pool)
        .await
        .context("Failed to fetch series list")?;
    let mut present: Vec<(String, String)> = rows
        .iter()
        .map(|r| {
            let series: String = r.get("series");
            let first: String = r.get("first");
            (series, first)
        })
        .collect();

    let known_idx = |s: &str| known.iter().position(|k| k == s);
    present.sort_by(|a, b| match (known_idx(&a.0), known_idx(&b.0)) {
        (Some(x), Some(y)) => x.cmp(&y),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => a.1.cmp(&b.1).then_with(|| a.0.cmp(&b.0)),
    });
    let order: Vec<&str> = present.iter().map(|(s, _)| s.as_str()).collect();

    sqlx::query(
        "CREATE TABLE IF NOT EXISTS export_meta (key TEXT PRIMARY KEY, value TEXT NOT NULL)",
    )
    .execute(pool)
    .await
    .context("Failed to create export_meta table")?;
    sqlx::query("INSERT OR REPLACE INTO export_meta (key, value) VALUES ('series_order', ?)")
        .bind(serde_json::to_string(&order).context("Failed to serialise series_order")?)
        .execute(pool)
        .await
        .context("Failed to insert series_order")?;

    info!(count = order.len(), "Wrote export_meta.series_order");
    Ok(())
}

fn parse_series_column(csv: &str) -> Vec<String> {
    let mut lines = csv.lines();
    let Some(header) = lines.next() else {
        return vec![];
    };
    let Some(idx) = header.split(',').position(|c| c.trim() == "series") else {
        return vec![];
    };
    lines
        .filter_map(|l| l.split(',').nth(idx))
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

async fn write_profile_configs(pool: &SqlitePool) -> Result<()> {
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS profile_configs (
            id           TEXT PRIMARY KEY,
            profile_name TEXT NOT NULL,
            has_flags    INTEGER NOT NULL,
            flag_summary TEXT NOT NULL,
            flags_json   TEXT NOT NULL
        )",
    )
    .execute(pool)
    .await
    .context("Failed to create profile_configs table")?;

    let rows = sqlx::query(
        "SELECT DISTINCT profile_name, profile_content FROM batches ORDER BY profile_name",
    )
    .fetch_all(pool)
    .await
    .context("Failed to fetch distinct profiles")?;

    let mut count = 0usize;
    for row in rows {
        let profile_name: String = row.get("profile_name");
        let content: String = row.get("profile_content");

        let parsed: ProfileForExport = toml::from_str(&content)
            .with_context(|| format!("Failed to parse profile_content for '{profile_name}'"))?;

        // Unique flag values; the same flag often applies to CFLAGS and CXXFLAGS.
        let unique_flags: BTreeSet<String> = parsed.flags.iter().map(|f| f.flag.clone()).collect();

        let has_flags = if unique_flags.is_empty() { 0i64 } else { 1i64 };

        let flag_summary = match unique_flags.len() {
            0 => "baseline".to_string(),
            1 => unique_flags.iter().next().unwrap().clone(),
            2 => unique_flags.iter().cloned().collect::<Vec<_>>().join(", "),
            n => {
                let first_two: Vec<_> = unique_flags.iter().take(2).cloned().collect();
                format!("{} +{} more", first_two.join(", "), n - 2)
            }
        };

        let flags_json = serde_json::to_string(
            &parsed
                .flags
                .iter()
                .map(|f| {
                    serde_json::json!({
                        "var": f.var,
                        "flag": f.flag,
                        "reason": f.reason
                    })
                })
                .collect::<Vec<_>>(),
        )
        .context("Failed to serialise flags_json")?;

        sqlx::query(
            "INSERT OR REPLACE INTO profile_configs
             (id, profile_name, has_flags, flag_summary, flags_json)
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(&profile_name)
        .bind(&profile_name)
        .bind(has_flags)
        .bind(&flag_summary)
        .bind(&flags_json)
        .execute(pool)
        .await
        .with_context(|| format!("Failed to insert profile_config for '{profile_name}'"))?;

        count += 1;
    }

    info!(count, "Wrote profile_configs");
    Ok(())
}

// Stale .log files are removed first: the frontend serves whatever sits in
// logs/. Rows stream one at a time (a full batch's compressed logs can reach
// hundreds of MB).
async fn write_logs(
    pool: &SqlitePool,
    output_dir: &Path,
    batch_filter: Option<&[Uuid]>,
) -> Result<()> {
    let logs_dir = output_dir.join("logs");

    let mut entries = fs::read_dir(&logs_dir)
        .await
        .context("Failed to read logs directory")?;
    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some("log") {
            fs::remove_file(&path)
                .await
                .with_context(|| format!("Failed to remove stale log {}", path.display()))?;
        }
    }

    let sql = match batch_filter {
        Some(ids) if !ids.is_empty() => {
            let placeholders = vec!["?"; ids.len()].join(",");
            format!(
                "SELECT id, build_log FROM builds
                 WHERE build_log IS NOT NULL AND batch_id IN ({placeholders}) AND id > ?
                 ORDER BY id LIMIT 1"
            )
        }
        _ => "SELECT id, build_log FROM builds
              WHERE build_log IS NOT NULL AND id > ?
              ORDER BY id LIMIT 1"
            .to_string(),
    };

    let mut last_id = String::new();
    let mut count = 0usize;
    loop {
        let mut query = sqlx::query(&sql);
        if let Some(ids) = batch_filter {
            for id in ids {
                query = query.bind(id.to_string());
            }
        }
        let Some(row) = query
            .bind(&last_id)
            .fetch_optional(pool)
            .await
            .context("Failed to fetch build logs")?
        else {
            break;
        };

        last_id = row.get("id");
        let blob: Vec<u8> = row.get("build_log");
        let text = decompress_log(&blob);
        fs::write(logs_dir.join(format!("{last_id}.log")), text).await?;
        count += 1;
    }

    info!(count, "Wrote log files");
    Ok(())
}

fn decompress_log(blob: &[u8]) -> String {
    let mut gz = GzDecoder::new(blob);
    let mut s = String::new();
    if gz.read_to_string(&mut s).is_ok() {
        s
    } else {
        String::from_utf8_lossy(blob).into_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;
    use crate::models::{BuildStatus, BuilderBackend};
    use crate::profile::{Compiler, CompilerType, Profile, Target};
    use flate2::{write::GzEncoder, Compression};
    use std::io::Write as _;

    fn sample_profile() -> Profile {
        Profile {
            compiler: Compiler {
                compiler_type: CompilerType::Clang,
                version: "18".to_string(),
            },
            target: Target {
                series: "noble".to_string(),
            },
            flags: vec![],
            name: "clang-18-noble".to_string(),
            raw_content:
                "[compiler]\ntype = \"clang\"\nversion = \"18\"\n[target]\nseries = \"noble\"\n"
                    .to_string(),
        }
    }

    fn gzip(data: &str) -> Vec<u8> {
        let mut enc = GzEncoder::new(Vec::new(), Compression::default());
        enc.write_all(data.as_bytes()).unwrap();
        enc.finish().unwrap()
    }

    #[tokio::test]
    async fn export_writes_only_current_logs_and_removes_stale() {
        let dir = tempfile::tempdir().unwrap();
        let pool = db::init(&dir.path().join("rebuilder.db")).await.unwrap();

        let profile = sample_profile();
        let batch = db::create_batch(&pool, &profile, BuilderBackend::Sbuild, "amd64")
            .await
            .unwrap();

        let now = chrono::Utc::now();
        let mut build_ids = Vec::new();
        for (pkg, log) in [
            ("foo", "foo log line 1\nfoo log line 2"),
            ("bar", "bar log"),
        ] {
            let build = db::insert_build(
                &pool,
                &db::NewBuild {
                    batch_id: batch.id,
                    source_package: pkg,
                    version: "1.0",
                    status: BuildStatus::Succeeded,
                    build_duration_seconds: None,
                    peak_memory_mb: None,
                    build_log: Some(gzip(log)),
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
            build_ids.push(build.id);
        }
        db::insert_build(
            &pool,
            &db::NewBuild {
                batch_id: batch.id,
                source_package: "nolog",
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

        let out = dir.path().join("export");
        fs::create_dir_all(out.join("logs")).await.unwrap();
        fs::write(out.join("logs/stale.log"), "stale")
            .await
            .unwrap();
        fs::write(out.join("logs/README.txt"), "keep me")
            .await
            .unwrap();

        export_data(&pool, &out, None).await.unwrap();

        let mut names: Vec<String> = Vec::new();
        let mut entries = fs::read_dir(out.join("logs")).await.unwrap();
        while let Some(entry) = entries.next_entry().await.unwrap() {
            names.push(entry.file_name().to_string_lossy().into_owned());
        }
        names.sort();
        let mut expected: Vec<String> = build_ids.iter().map(|id| format!("{id}.log")).collect();
        expected.push("README.txt".to_string());
        expected.sort();
        assert_eq!(
            names, expected,
            "logs dir must contain exactly the current builds' logs (stale removed, non-log kept)"
        );

        let foo_log = fs::read_to_string(out.join("logs").join(format!("{}.log", build_ids[0])))
            .await
            .unwrap();
        assert_eq!(foo_log, "foo log line 1\nfoo log line 2");

        let export_pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect(&format!("sqlite:{}", out.join("rebuild.db").display()))
            .await
            .unwrap();
        let remaining: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM builds WHERE build_log IS NOT NULL")
                .fetch_one(&export_pool)
                .await
                .unwrap();
        export_pool.close().await;
        assert_eq!(remaining, 0, "exported db must not carry log blobs");
    }

    fn profile_for(name: &str, series: &str) -> Profile {
        Profile {
            compiler: Compiler {
                compiler_type: CompilerType::Clang,
                version: "18".to_string(),
            },
            target: Target {
                series: series.to_string(),
            },
            flags: vec![],
            name: name.to_string(),
            raw_content: format!(
                "[compiler]\ntype = \"clang\"\nversion = \"18\"\n[target]\nseries = \"{series}\"\n"
            ),
        }
    }

    #[tokio::test]
    async fn series_order_follows_distro_info_with_unknowns_appended() {
        let dir = tempfile::tempdir().unwrap();
        let pool = db::init(&dir.path().join("rebuilder.db")).await.unwrap();

        // Fixture CSV in release order; "resolute" absent, "typo" unknown.
        let csv = dir.path().join("ubuntu.csv");
        std::fs::write(
            &csv,
            "version,codename,series,created,release\n\
             22.04,Jammy Jellyfish,jammy,2021-10-14,2022-04-21\n\
             24.04,Noble Numbat,noble,2023-10-12,2024-04-25\n",
        )
        .unwrap();

        for (name, series) in [
            ("clang-18-noble", "noble"),
            ("clang-18-jammy", "jammy"),
            ("clang-18-typo", "typo"),
        ] {
            db::create_batch(
                &pool,
                &profile_for(name, series),
                BuilderBackend::Sbuild,
                "amd64",
            )
            .await
            .unwrap();
        }

        write_export_meta(&pool, &csv).await.unwrap();

        let value: String =
            sqlx::query_scalar("SELECT value FROM export_meta WHERE key = 'series_order'")
                .fetch_one(&pool)
                .await
                .unwrap();
        let order: Vec<String> = serde_json::from_str(&value).unwrap();
        assert_eq!(
            order,
            vec!["jammy".to_string(), "noble".to_string(), "typo".to_string()],
            "known series in release order, unknown appended"
        );
    }

    #[tokio::test]
    async fn series_order_skipped_when_csv_missing() {
        let dir = tempfile::tempdir().unwrap();
        let pool = db::init(&dir.path().join("rebuilder.db")).await.unwrap();
        db::create_batch(
            &pool,
            &profile_for("clang-18-noble", "noble"),
            BuilderBackend::Sbuild,
            "amd64",
        )
        .await
        .unwrap();

        write_export_meta(&pool, &dir.path().join("nonexistent.csv"))
            .await
            .unwrap();

        let has_table: bool =
            sqlx::query_scalar("SELECT COUNT(*) > 0 FROM pragma_table_info('export_meta')")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(!has_table, "no export_meta when distro-info is unavailable");
    }
}
