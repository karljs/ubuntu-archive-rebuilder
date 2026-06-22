//! Export module — produces a stripped SQLite database and log files for the frontend.
//!
//! The exported `rebuilder.db` contains all batches but with `build_log` columns
//! nulled out, keeping the file small enough for the browser to load via sql.js.
//! Build logs are written separately to `logs/<build-id>.log` and fetched on demand.
//!
//! The export also materialises a `profile_configs` table derived from the
//! snapshotted `profile_content` TOML stored in each batch row.  This lets the
//! frontend treat profile configurations as first-class queryable entities without
//! parsing TOML in JavaScript.

use anyhow::{Context, Result};
use flate2::read::GzDecoder;
use serde::Deserialize;
use sqlx::sqlite::SqlitePoolOptions;
use sqlx::{Row, SqlitePool};
use std::collections::BTreeSet;
use std::io::Read;
use std::path::Path;
use tokio::fs;
use tracing::info;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Minimal profile deserialisation — only the [[flags]] section is needed here.
// Using a separate struct (not profile::Profile) so this stays forward-compatible
// if Profile gains new fields with deny_unknown_fields.
// ---------------------------------------------------------------------------

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

/// Export data to the output directory.
///
/// Always writes a complete `rebuild.db` containing all batches.  Log files
/// are written to `logs/<build-id>.log`; `batch_filter` controls which batches
/// have their logs written — pass `None` to write logs for all batches.
pub async fn export_data(
    pool: &SqlitePool,
    output_dir: &Path,
    batch_filter: Option<&[Uuid]>,
) -> Result<()> {
    fs::create_dir_all(output_dir).await?;
    fs::create_dir_all(output_dir.join("logs")).await?;

    // Write log files from the live DB before the export copy strips them.
    write_logs(pool, output_dir, batch_filter).await?;

    // Create a clean, compacted copy of the live DB.
    let db_path = output_dir.join("rebuild.db");
    if db_path.exists() {
        fs::remove_file(&db_path).await?;
    }
    let db_path_str = db_path.to_string_lossy();
    sqlx::query(&format!("VACUUM INTO '{db_path_str}'"))
        .execute(pool)
        .await
        .context("Failed to create export database")?;

    // Open the export copy, null out build_log, then compact to reclaim the freed pages.
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

    export_pool.close().await;

    info!(path = %db_path.display(), "Wrote export database");
    Ok(())
}

/// Materialise the `profile_configs` table in the export database.
///
/// Each row represents one distinct profile (by profile_name).  The flags are
/// parsed from the snapshotted TOML and reduced to a human-readable summary
        /// and a full JSON representation for the frontend to use without any TOML parsing.
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

    // One row per distinct profile_name (profiles are snapshotted per batch but
    // the name uniquely identifies a profile file).
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

        let parsed: ProfileForExport = toml::from_str(&content).with_context(|| {
            format!("Failed to parse profile_content for '{profile_name}'")
        })?;

        // Collect unique flag *values* (deduplicated — the same flag is often
        // applied to both DEB_CFLAGS_APPEND and DEB_CXXFLAGS_APPEND).
        let unique_flags: BTreeSet<String> =
            parsed.flags.iter().map(|f| f.flag.clone()).collect();

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

        // Full JSON for tooltip detail: include var, flag, reason for every entry.
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

/// Write per-build log files from the live database.
///
/// Logs are stored as gzip-compressed blobs.  This function decompresses each
/// one before writing the plain-text `.log` file so the frontend can serve them
/// as-is.  Legacy plain-text blobs (pre-migration 004) are handled by falling
/// back to raw UTF-8 if gzip decompression fails.
async fn write_logs(
    pool: &SqlitePool,
    output_dir: &Path,
    batch_filter: Option<&[Uuid]>,
) -> Result<()> {
    let logs_dir = output_dir.join("logs");

    let rows = match batch_filter {
        Some(ids) => {
            let mut all = Vec::new();
            for id in ids {
                let batch_rows = sqlx::query(
                    "SELECT id, build_log FROM builds
                     WHERE batch_id = ? AND build_log IS NOT NULL",
                )
                .bind(id.to_string())
                .fetch_all(pool)
                .await
                .context("Failed to fetch build logs")?;
                all.extend(batch_rows);
            }
            all
        }
        None => sqlx::query("SELECT id, build_log FROM builds WHERE build_log IS NOT NULL")
            .fetch_all(pool)
            .await
            .context("Failed to fetch build logs")?,
    };

    let count = rows.len();
    for row in rows {
        let id: String = row.get("id");
        let blob: Vec<u8> = row.get("build_log");
        let text = decompress_log(&blob);
        fs::write(logs_dir.join(format!("{id}.log")), text).await?;
    }
    info!(count, "Wrote log files");
    Ok(())
}

/// Decompress a gzip-compressed log blob to a String.
/// Falls back to raw UTF-8 interpretation for legacy plain-text blobs.
fn decompress_log(blob: &[u8]) -> String {
    let mut gz = GzDecoder::new(blob);
    let mut s = String::new();
    if gz.read_to_string(&mut s).is_ok() {
        s
    } else {
        String::from_utf8_lossy(blob).into_owned()
    }
}
