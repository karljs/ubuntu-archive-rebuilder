//! Source package fetching using `pull-lp-source`.

use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};
use tokio::process::Command;
use tracing::debug;

/// A fetched source package, ready for sbuild.
#[derive(Debug)]
pub struct SourcePackage {
    pub name: String,
    pub version: String,
    pub dsc_path: PathBuf,
}

/// Download a source package from the Ubuntu archive and return the `.dsc` path.
pub async fn fetch_source(
    package_name: &str,
    series: &str,
    work_dir: &Path,
) -> Result<SourcePackage> {
    debug!(package = %package_name, %series, work_dir = %work_dir.display(), "Fetching source");

    let output = Command::new("pull-lp-source")
        .arg("-d")
        .arg(package_name)
        .arg(series)
        .current_dir(work_dir)
        .output()
        .await
        .context("Failed to execute pull-lp-source")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        bail!(
            "pull-lp-source failed for {package_name} in {series}: {stderr}\n{stdout}"
        );
    }

    let dsc_path = find_dsc_file(work_dir, package_name)?;
    let version = extract_version_from_dsc(&dsc_path)?;

    Ok(SourcePackage {
        name: package_name.to_string(),
        version,
        dsc_path,
    })
}

/// Find the `.dsc` file in a directory, preferring an exact package-name
/// prefix match but falling back to any `.dsc` if only one exists.
fn find_dsc_file(dir: &Path, package_name: &str) -> Result<PathBuf> {
    let mut exact_match: Option<PathBuf> = None;
    let mut any_dsc: Option<PathBuf> = None;

    for entry in std::fs::read_dir(dir).context("Failed to read work directory")? {
        let path = entry?.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if !name.ends_with(".dsc") {
            continue;
        }
        if name.starts_with(package_name) {
            exact_match = Some(path);
            break;
        }
        any_dsc = Some(path);
    }

    exact_match
        .or(any_dsc)
        .with_context(|| format!("No .dsc file found for {package_name} in {}", dir.display()))
}

/// Extract version from a `.dsc` filename (e.g. `hello_2.10-3.dsc` → `2.10-3`).
fn extract_version_from_dsc(dsc_path: &Path) -> Result<String> {
    let filename = dsc_path
        .file_name()
        .and_then(|n| n.to_str())
        .context("Invalid .dsc path")?;

    let base = filename
        .strip_suffix(".dsc")
        .context("File doesn't end with .dsc")?;

    base.find('_')
        .map(|idx| base[idx + 1..].to_string())
        .with_context(|| format!("Cannot parse version from .dsc filename: {filename}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_simple() {
        let path = PathBuf::from("/tmp/coreutils_8.32-4ubuntu1.dsc");
        assert_eq!(extract_version_from_dsc(&path).unwrap(), "8.32-4ubuntu1");
    }

    #[test]
    fn version_with_dfsg() {
        let path = PathBuf::from("/tmp/tar_1.34+dfsg-1.dsc");
        assert_eq!(extract_version_from_dsc(&path).unwrap(), "1.34+dfsg-1");
    }

    #[test]
    fn version_ubuntu_suffix() {
        let path = PathBuf::from("/tmp/gcc-defaults_1.193ubuntu2.dsc");
        assert_eq!(extract_version_from_dsc(&path).unwrap(), "1.193ubuntu2");
    }
}
