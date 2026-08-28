//! Fetch Ubuntu Sources.gz indices and turn them into package lists.
//! Arch filtering uses Debian `Architecture:` semantics; mirror choice
//! follows the archive/ports split (see default_mirror_for_arch).

use anyhow::{Context, Result};
use flate2::read::GzDecoder;
use std::io::{BufRead, BufReader};
use std::time::Duration;

/// amd64/i386: primary archive. Everything else: ports.
/// *.clouds.archive.ubuntu.com is a full mirror of archive.ubuntu.com.
pub fn default_mirror_for_arch(arch: &str) -> &'static str {
    match arch {
        "amd64" | "i386" => "https://us.clouds.archive.ubuntu.com/ubuntu",
        _ => "https://ports.ubuntu.com/ubuntu-ports",
    }
}

// ureq ignores proxy env vars; locked-down hosts need them.
pub fn http_agent() -> ureq::Agent {
    let mut builder = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(30))
        .timeout_read(Duration::from_secs(300));
    if let Some(url) = proxy_from_env() {
        match ureq::Proxy::new(&url) {
            Ok(p) => builder = builder.proxy(p),
            Err(e) => eprintln!("Warning: invalid proxy {url}: {e}"),
        }
    }
    builder.build()
}

fn proxy_from_env() -> Option<String> {
    proxy_from_lookup(|k| std::env::var(k).ok().filter(|v| !v.trim().is_empty()))
}

fn proxy_from_lookup(get: impl Fn(&str) -> Option<String>) -> Option<String> {
    for key in [
        "https_proxy",
        "HTTPS_PROXY",
        "all_proxy",
        "ALL_PROXY",
        "http_proxy",
        "HTTP_PROXY",
    ] {
        if let Some(v) = get(key) {
            return Some(v);
        }
    }
    None
}

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

        let response = http_agent()
            .get(&url)
            .call()
            .with_context(|| format!("HTTP request failed for {url}"))?;

        let gz = GzDecoder::new(response.into_reader());
        let reader = BufReader::new(gz);

        let packages = parse_sources(reader, arch, component)
            .with_context(|| format!("Failed to parse Sources.gz from {url}"))?;

        results.extend(packages);
    }

    results.sort_by(|a, b| a.0.cmp(&b.0));
    results.dedup_by(|a, b| a.0 == b.0);

    Ok(results)
}

fn parse_sources<R: std::io::Read>(
    reader: BufReader<R>,
    arch: &str,
    component: &str,
) -> Result<Vec<(String, String)>> {
    let mut packages = Vec::new();

    let mut current_package: Option<String> = None;
    let mut current_arch: Option<String> = None;

    for line in reader.lines() {
        let line = line.context("I/O error reading Sources")?;

        if line.is_empty() {
            if let (Some(pkg), Some(arch_field)) = (current_package.take(), current_arch.take()) {
                if arch_matches(&arch_field, arch) {
                    packages.push((pkg, component.to_string()));
                }
            } else {
                current_package = None;
                current_arch = None;
            }
            continue;
        }

        if line.starts_with(' ') || line.starts_with('\t') {
            continue;
        }

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

    // Final stanza without a trailing blank line.
    if let (Some(pkg), Some(arch_field)) = (current_package, current_arch) {
        if arch_matches(&arch_field, arch) {
            packages.push((pkg, component.to_string()));
        }
    }

    Ok(packages)
}

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
            "https://us.clouds.archive.ubuntu.com/ubuntu"
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
