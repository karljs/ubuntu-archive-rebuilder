//! Ubuntu archive package list fetcher.
//!
//! Downloads and parses `Sources.gz` from the Ubuntu archive to produce a list
//! of source package names suitable for passing to `rebuilder build --packages`.
//!
//! # Architecture filtering
//!
//! Each source package stanza carries an `Architecture:` field that declares
//! which architectures the package builds on.  Special values:
//!
//! - `any`       — builds on every architecture; always included.
//! - `all`       — architecture-independent; always included.
//! - `linux-any` — any Linux architecture (excludes kFreeBSD etc.); included.
//! - An explicit space-separated list — included only if the target arch
//!   appears in the list, or if `any` or `linux-any` is in the list.
//!
//! # Mirror selection
//!
//! `archive.ubuntu.com/ubuntu` carries `amd64` and `i386`.
//! All other architectures are hosted on `ports.ubuntu.com/ubuntu-ports`.
//! [`default_mirror_for_arch`] encodes this so callers get the right default
//! without having to know the split.  The user can always override via
//! `--url`.
//!
//! # Future: multi-arch build runs
//!
//! The `arch` parameter here filters the *source* list.  When multi-arch build
//! support is added to the pipeline, the `Batch` model will need an `arch`
//! field and `sbuild` will need `--arch=<arch>`.  The fetcher is already
//! arch-aware so it will integrate cleanly.

use anyhow::{Context, Result};
use flate2::read::GzDecoder;
use std::io::{BufRead, BufReader};

/// Returns the default Ubuntu archive mirror URL for a given architecture.
///
/// `amd64` and `i386` are hosted on the primary archive; everything else lives
/// on the ports mirror.
pub fn default_mirror_for_arch(arch: &str) -> &'static str {
    match arch {
        "amd64" | "i386" => "https://archive.ubuntu.com/ubuntu",
        _ => "https://ports.ubuntu.com/ubuntu-ports",
    }
}

/// Fetch source package names from the Ubuntu archive for the given series,
/// components, and target architecture.
///
/// Downloads `{mirror_url}/dists/{series}/{component}/source/Sources.gz` for
/// each requested component, parses the RFC-822-style stanza format, filters
/// by architecture, and returns a deduplicated sorted list of source package
/// names.
///
/// # Arguments
///
/// * `series`     — Ubuntu series name, e.g. `"noble"`.
/// * `components` — Archive components to fetch, e.g. `["main"]` or
///   `["main", "universe"]`.
/// * `arch`       — Target build architecture, e.g. `"amd64"`.  Used to
///   exclude source packages that cannot build on this arch.
/// * `mirror_url` — Base URL of the Ubuntu archive mirror, without trailing
///   slash, e.g. `"https://archive.ubuntu.com/ubuntu"`.
///
/// # Returns
///
/// A `Vec<(String, String)>` of `(package_name, component)` pairs, sorted by
/// package name.  The component is included so the caller can report per-
/// component counts.
pub fn fetch_package_list(
    series: &str,
    components: &[&str],
    arch: &str,
    mirror_url: &str,
) -> Result<Vec<(String, String)>> {
    let mut results: Vec<(String, String)> = Vec::new();

    for &component in components {
        let url = format!(
            "{}/dists/{}/{}/source/Sources.gz",
            mirror_url.trim_end_matches('/'),
            series,
            component,
        );

        eprintln!("Fetching {url} ...");

        let response = ureq::get(&url)
            .call()
            .with_context(|| format!("HTTP request failed for {url}"))?;

        let gz = GzDecoder::new(response.into_reader());
        let reader = BufReader::new(gz);

        let packages = parse_sources(reader, arch, component)
            .with_context(|| format!("Failed to parse Sources.gz from {url}"))?;

        results.extend(packages);
    }

    // Sort by package name; within the same name, earlier component wins
    // (shouldn't happen in practice for Ubuntu's main/universe split, but
    // be defensive).
    results.sort_by(|a, b| a.0.cmp(&b.0));
    results.dedup_by(|a, b| a.0 == b.0); // keep first (earlier component)

    Ok(results)
}

/// Parse a `Sources` file (already decompressed) and return the names of
/// source packages that can build on `arch`.
fn parse_sources<R: std::io::Read>(
    reader: BufReader<R>,
    arch: &str,
    component: &str,
) -> Result<Vec<(String, String)>> {
    let mut packages = Vec::new();

    // State for the current stanza.
    let mut current_package: Option<String> = None;
    let mut current_arch: Option<String> = None;

    for line in reader.lines() {
        let line = line.context("I/O error reading Sources")?;

        if line.is_empty() {
            // Blank line — end of stanza.  Emit the package if we have both
            // fields and it passes the arch filter.
            if let (Some(pkg), Some(arch_field)) = (current_package.take(), current_arch.take()) {
                if arch_matches(&arch_field, arch) {
                    packages.push((pkg, component.to_string()));
                }
            } else {
                // Discard incomplete stanza (shouldn't happen in a well-formed
                // Sources file, but be resilient).
                current_package = None;
                current_arch = None;
            }
            continue;
        }

        // Continuation lines (folded fields) start with whitespace — skip
        // them; we only care about the first line of each field.
        if line.starts_with(' ') || line.starts_with('\t') {
            continue;
        }

        // Field: value
        if let Some((field, value)) = line.split_once(':') {
            let field = field.trim();
            let value = value.trim();
            match field {
                "Package" => current_package = Some(value.to_string()),
                "Architecture" => current_arch = Some(value.to_string()),
                _ => {}
            }
        }
    }

    // Handle a final stanza that isn't terminated by a blank line.
    if let (Some(pkg), Some(arch_field)) = (current_package, current_arch) {
        if arch_matches(&arch_field, arch) {
            packages.push((pkg, component.to_string()));
        }
    }

    Ok(packages)
}

/// Returns true if the `Architecture:` field value from a Sources stanza
/// indicates the package can build on `target_arch`.
///
/// The field is a space-separated list of architecture qualifiers.  Any of
/// the following cause the package to be included:
///
/// - `any`       — builds everywhere
/// - `all`       — architecture-independent
/// - `linux-any` — any Linux architecture (Ubuntu only runs Linux)
/// - `<target_arch>` — exact match
fn arch_matches(arch_field: &str, target_arch: &str) -> bool {
    for token in arch_field.split_whitespace() {
        match token {
            "any" | "all" | "linux-any" => return true,
            t if t == target_arch => return true,
            _ => {}
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_arch_matches_any() {
        assert!(arch_matches("any", "amd64"));
        assert!(arch_matches("any", "arm64"));
    }

    #[test]
    fn test_arch_matches_all() {
        assert!(arch_matches("all", "amd64"));
        assert!(arch_matches("all", "riscv64"));
    }

    #[test]
    fn test_arch_matches_linux_any() {
        assert!(arch_matches("linux-any", "amd64"));
        assert!(arch_matches("linux-any", "arm64"));
    }

    #[test]
    fn test_arch_matches_explicit() {
        assert!(arch_matches("amd64 arm64", "amd64"));
        assert!(arch_matches("amd64 arm64", "arm64"));
        assert!(!arch_matches("amd64 arm64", "riscv64"));
    }

    #[test]
    fn test_arch_matches_single_exclusion() {
        assert!(!arch_matches("i386", "amd64"));
        assert!(arch_matches("i386", "i386"));
    }

    #[test]
    fn test_default_mirror_amd64() {
        assert_eq!(
            default_mirror_for_arch("amd64"),
            "https://archive.ubuntu.com/ubuntu"
        );
    }

    #[test]
    fn test_default_mirror_ports() {
        assert_eq!(
            default_mirror_for_arch("arm64"),
            "https://ports.ubuntu.com/ubuntu-ports"
        );
        assert_eq!(
            default_mirror_for_arch("riscv64"),
            "https://ports.ubuntu.com/ubuntu-ports"
        );
    }

    #[test]
    fn test_parse_sources_basic() {
        let input = "\
Package: hello
Architecture: any
Version: 2.10-3
Priority: optional

Package: arch-specific
Architecture: i386
Version: 1.0-1
Priority: optional

Package: data-pkg
Architecture: all
Version: 3.0-1
Priority: optional

";
        let reader = BufReader::new(input.as_bytes());
        let result = parse_sources(reader, "amd64", "main").unwrap();
        let names: Vec<&str> = result.iter().map(|(n, _)| n.as_str()).collect();
        assert!(names.contains(&"hello"), "should include arch=any");
        assert!(names.contains(&"data-pkg"), "should include arch=all");
        assert!(
            !names.contains(&"arch-specific"),
            "should exclude i386-only on amd64"
        );
    }
}
