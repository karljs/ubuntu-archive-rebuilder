//! Source package download directly from the archive mirror pool.
//! The Sources.gz index gives file names, sizes and sha256 sums, so no
//! Launchpad access is needed.

use crate::fetcher;
use anyhow::{bail, Context, Result};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::task;
use tracing::{debug, info};

/// Wall-clock cap on one package's download set. Socket-level timeouts
/// bound the individual requests.
pub const FETCH_TIMEOUT: Duration = Duration::from_secs(30 * 60);

const COMPONENTS: [&str; 4] = ["main", "universe", "restricted", "multiverse"];

#[derive(Debug)]
pub struct SourcePackage {
    pub name: String,
    pub version: String,
    pub dsc_path: PathBuf,
}

struct SourceFile {
    name: String,
    size: u64,
    sha256: Option<String>,
}

struct SourceEntry {
    directory: String,
    files: Vec<SourceFile>,
}

/// Package list for one series; fetched and parsed once per batch.
pub struct SourceIndex {
    mirror: String,
    agent: ureq::Agent,
    entries: HashMap<String, SourceEntry>,
}

impl SourceIndex {
    pub fn load(series: &str, arch: &str) -> Result<Self> {
        let mirror = fetcher::default_mirror_for_arch(arch).to_string();
        let agent = fetcher::http_agent();
        let mut entries = HashMap::new();
        for comp in COMPONENTS {
            let url = format!(
                "{}/dists/{series}/{comp}/source/Sources.gz",
                mirror.trim_end_matches('/')
            );
            debug!("Fetching {url}");
            let bytes = download(&agent, &url)?;
            parse_sources_gz(&bytes, &mut entries);
        }
        info!(series, packages = entries.len(), "Loaded source index");
        Ok(Self {
            mirror,
            agent,
            entries,
        })
    }

    pub fn fetch(&self, package: &str, work_dir: &Path) -> Result<SourcePackage> {
        let entry = self
            .entries
            .get(package)
            .with_context(|| format!("package {package} not found in the series source index"))?;

        for f in &entry.files {
            let url = format!("{}/{}/{}", self.mirror, entry.directory, f.name);
            let dest = work_dir.join(&f.name);
            debug!("Downloading {url}");
            let bytes = download(&self.agent, &url)?;
            verify(&bytes, f)?;
            std::fs::write(&dest, &bytes)
                .with_context(|| format!("failed to write {}", dest.display()))?;
        }

        let dsc = entry
            .files
            .iter()
            .find(|f| f.name.ends_with(".dsc"))
            .map(|f| work_dir.join(&f.name))
            .with_context(|| format!("no .dsc listed for {package}"))?;
        let version = extract_version_from_dsc(&dsc)?;

        Ok(SourcePackage {
            name: package.to_string(),
            version,
            dsc_path: dsc,
        })
    }
}

// Timed-out spawn_blocking tasks keep downloading until their socket
// timeouts fire; the batch moves on regardless.
pub async fn fetch_source(
    index: std::sync::Arc<SourceIndex>,
    package_name: &str,
    work_dir: &Path,
) -> Result<SourcePackage> {
    let pkg = package_name.to_string();
    let dir = work_dir.to_path_buf();
    let fut = task::spawn_blocking(move || index.fetch(&pkg, &dir));
    match tokio::time::timeout(FETCH_TIMEOUT, fut).await {
        Ok(res) => res.context("source download task panicked")?,
        Err(_) => bail!("source fetch for {package_name} timed out"),
    }
}

fn download(agent: &ureq::Agent, url: &str) -> Result<Vec<u8>> {
    let resp = agent
        .get(url)
        .call()
        .with_context(|| format!("HTTP request failed for {url}"))?;
    let mut buf = Vec::new();
    let reader = std::io::BufReader::new(resp.into_reader());
    let mut limited = reader.take(4 * 1024 * 1024 * 1024);
    std::io::Read::read_to_end(&mut limited, &mut buf)
        .with_context(|| format!("failed to download {url}"))?;
    Ok(buf)
}

fn verify(bytes: &[u8], f: &SourceFile) -> Result<()> {
    if bytes.len() as u64 != f.size {
        bail!(
            "size mismatch for {}: expected {} bytes, got {}",
            f.name,
            f.size,
            bytes.len()
        );
    }
    if let Some(expected) = &f.sha256 {
        let got = hex(&Sha256::digest(bytes));
        if !got.eq_ignore_ascii_case(expected) {
            bail!(
                "sha256 mismatch for {}: expected {expected}, got {got}",
                f.name
            );
        }
    }
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn parse_sources_gz(gz: &[u8], entries: &mut HashMap<String, SourceEntry>) {
    let mut reader = flate2::read::GzDecoder::new(gz);
    let mut text = String::new();
    if std::io::Read::read_to_string(&mut reader, &mut text).is_err() {
        return;
    }

    let mut package: Option<String> = None;
    let mut directory = String::new();
    let mut files: Vec<(String, u64, Option<String>)> = Vec::new();
    let mut section = ""; // "", "files", or "sha256"

    let flush = |package: &mut Option<String>,
                 directory: &mut String,
                 files: &mut Vec<(String, u64, Option<String>)>,
                 entries: &mut HashMap<String, SourceEntry>| {
        if let Some(p) = package.take() {
            if !directory.is_empty() {
                let mut merged: HashMap<String, (u64, Option<String>)> = HashMap::new();
                for (name, size, sha) in files.drain(..) {
                    merged.insert(name, (size, sha));
                }
                entries.insert(
                    p,
                    SourceEntry {
                        directory: std::mem::take(directory),
                        files: merged
                            .into_iter()
                            .map(|(name, (size, sha256))| SourceFile { name, size, sha256 })
                            .collect(),
                    },
                );
            }
        }
        directory.clear();
        files.clear();
    };

    for line in text.lines() {
        if line.is_empty() {
            flush(&mut package, &mut directory, &mut files, entries);
            section = "";
            continue;
        }
        if let Some(rest) = line.strip_prefix(' ') {
            if section != "files" && section != "sha256" {
                continue;
            }
            // "checksum size filename" continuation of Files/Checksums-Sha256
            let mut it = rest.split_whitespace();
            let (sum, size, name) = (it.next(), it.next(), it.next());
            if let (Some(name), Some(size)) = (name, size) {
                let size: u64 = size.parse().unwrap_or(0);
                if let Some(sum) = sum {
                    let sha = if section == "sha256" {
                        Some(sum.to_string())
                    } else {
                        None
                    };
                    if let Some(existing) = files.iter_mut().find(|f| f.0 == name) {
                        if sha.is_some() {
                            existing.2 = sha;
                        }
                    } else {
                        files.push((name.to_string(), size, sha));
                    }
                }
            }
            continue;
        }
        if let Some((field, value)) = line.split_once(':') {
            match field.trim() {
                "Package" => {
                    flush(&mut package, &mut directory, &mut files, entries);
                    package = Some(value.trim().to_string());
                }
                "Directory" => directory = value.trim().to_string(),
                "Files" => section = "files",
                "Checksums-Sha256" => section = "sha256",
                // every other field ends a folded section (Binary, Package-List, ...)
                _ => section = "",
            }
        }
    }
    flush(&mut package, &mut directory, &mut files, entries);
}

/// `hello_2.10-3.dsc` -> `2.10-3`.
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

    fn gzip(data: &str) -> Vec<u8> {
        let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        std::io::Write::write_all(&mut enc, data.as_bytes()).unwrap();
        enc.finish().unwrap()
    }

    #[test]
    fn parses_files_and_merges_sha256() {
        let text = "Package: hello
Binary: hello, hello-docs
Version: 2.10-3
Directory: pool/main/h/hello
Package-List:
 hello devel optional devel
Files:
 d66a24971b6b1c1c... 61600 hello_2.10-3.dsc
 abc123 50000 hello_2.10.orig.tar.gz
Checksums-Sha256:
 47d0b0f1e1... 61600 hello_2.10-3.dsc
 deadbeef 50000 hello_2.10.orig.tar.gz

Package: other
Version: 1.0
Directory: pool/universe/o/other
Files:
 ccdd 100 other_1.0.dsc

";
        let mut entries = HashMap::new();
        parse_sources_gz(&gzip(text), &mut entries);

        assert_eq!(entries.len(), 2);
        let hello = &entries["hello"];
        assert_eq!(hello.directory, "pool/main/h/hello");
        assert_eq!(hello.files.len(), 2);
        let dsc = hello
            .files
            .iter()
            .find(|f| f.name.ends_with(".dsc"))
            .unwrap();
        assert_eq!(dsc.size, 61600);
        assert_eq!(dsc.sha256.as_deref(), Some("47d0b0f1e1..."));
        let tar = hello
            .files
            .iter()
            .find(|f| f.name.ends_with(".tar.gz"))
            .unwrap();
        assert_eq!(tar.sha256.as_deref(), Some("deadbeef"));

        let other = &entries["other"];
        assert_eq!(other.files.len(), 1);
        assert!(other.files[0].sha256.is_none());
    }

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
}
