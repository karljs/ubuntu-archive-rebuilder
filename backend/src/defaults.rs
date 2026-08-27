//! Default-compiler resolution and profile generation for run-defaults.

use crate::profile::{Compiler, CompilerType, Flag, Profile, Target};
use anyhow::{bail, Context, Result};
use flate2::read::GzDecoder;
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read};
use std::time::Duration;

// One compressed Packages index per (series, component), shared across
// lookups within a run.
pub type IndexCache = HashMap<(String, String), Vec<u8>>;

/// The archive's own metapackages define the default compilers; no table to
/// maintain. gcc lives in main, clang in universe (with a main fallback).
pub fn default_compiler_version(
    series: &str,
    compiler: CompilerType,
    arch: &str,
    mirror: &str,
    cache: &mut IndexCache,
) -> Result<String> {
    let (package, prefix, components): (&str, &str, &[&str]) = match compiler {
        CompilerType::Gcc => ("gcc", "gcc-", &["main"]),
        CompilerType::Clang => ("clang", "clang-", &["universe", "main"]),
    };

    for comp in components {
        let key = (series.to_string(), comp.to_string());
        if !cache.contains_key(&key) {
            let url = format!(
                "{}/dists/{}/{}/binary-{arch}/Packages.gz",
                mirror.trim_end_matches('/'),
                series,
                comp,
            );
            let bytes = fetch_index(&url)
                .with_context(|| format!("fetching {package} index for {series}/{comp}"))?;
            cache.insert(key.clone(), bytes);
        }
        let bytes: &Vec<u8> = &cache[&key];
        if let Some(depends) = find_depends(bytes, package) {
            if let Some(version) = extract_version(&depends, prefix) {
                return Ok(version);
            }
        }
    }
    bail!(
        "no {package} metapackage found for {series}; cannot resolve the default {}",
        compiler.as_str()
    );
}

fn fetch_index(url: &str) -> Result<Vec<u8>> {
    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(300))
        .build();
    let resp = agent
        .get(url)
        .call()
        .with_context(|| format!("HTTP request failed for {url}"))?;
    let mut buf = Vec::new();
    resp.into_reader()
        .take(512 * 1024 * 1024)
        .read_to_end(&mut buf)
        .with_context(|| format!("failed to download {url}"))?;
    Ok(buf)
}

fn find_depends(gz: &[u8], package: &str) -> Option<String> {
    let reader = BufReader::new(GzDecoder::new(gz));
    let mut current: Option<String> = None;
    for line in reader.lines() {
        let line = line.ok()?;
        if line.is_empty() {
            current = None;
            continue;
        }
        if let Some(name) = line.strip_prefix("Package: ") {
            current = Some(name.trim().to_string());
        } else if let Some(deps) = line.strip_prefix("Depends: ") {
            if current.as_deref() == Some(package) {
                return Some(deps.trim().to_string());
            }
        }
    }
    None
}

// "gcc-13 (>= 13.2.0-11~)" -> "13"; rejects "gcc-x86-64-linux-gnu".
fn extract_version(depends: &str, prefix: &str) -> Option<String> {
    depends.split(',').find_map(|dep| {
        let name = dep.trim().split(' ').next().unwrap_or("");
        let rest = name.strip_prefix(prefix)?;
        if !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit()) {
            Some(rest.to_string())
        } else {
            None
        }
    })
}

/// In-memory profile for run-defaults: never written to profiles/, snapshotted
/// into the batch like any hand-written profile.
pub fn generate_profile(series: &str, compiler: CompilerType, version: &str) -> Profile {
    // The one auto-injected flag: Clang >= 15 defaults to DWARF5, which dwz
    // cannot process; without -gdwarf-4 most clang builds fail in dh_dwz.
    let needs_dwarf4 =
        compiler == CompilerType::Clang && version.parse::<u32>().is_ok_and(|v| v >= 15);
    let flags: Vec<Flag> = if needs_dwarf4 {
        ["DEB_CFLAGS_APPEND", "DEB_CXXFLAGS_APPEND"]
            .iter()
            .map(|var| Flag {
                var: var.to_string(),
                flag: "-gdwarf-4".to_string(),
                reason: "dwz cannot process Clang's DWARF5 output".to_string(),
            })
            .collect()
    } else {
        vec![]
    };
    let name = format!("{}-{}-{}", compiler.as_str(), version, series);
    let raw_content = render_profile_toml(series, compiler, version, &flags);
    Profile {
        compiler: Compiler {
            compiler_type: compiler,
            version: version.to_string(),
        },
        target: Target {
            series: series.to_string(),
        },
        flags,
        name,
        raw_content,
    }
}

fn render_profile_toml(
    series: &str,
    compiler: CompilerType,
    version: &str,
    flags: &[Flag],
) -> String {
    fn esc(s: &str) -> String {
        s.replace('\\', "\\\\").replace('"', "\\\"")
    }
    let mut s = String::new();
    s.push_str("[compiler]\n");
    s.push_str(&format!("type = \"{}\"\n", compiler.as_str()));
    s.push_str(&format!("version = \"{}\"\n", esc(version)));
    s.push_str("\n[target]\n");
    s.push_str(&format!("series = \"{}\"\n", esc(series)));
    for f in flags {
        s.push_str("\n[[flags]]\n");
        s.push_str(&format!("var = \"{}\"\n", esc(&f.var)));
        s.push_str(&format!("flag = \"{}\"\n", esc(&f.flag)));
        s.push_str(&format!("reason = \"{}\"\n", esc(&f.reason)));
    }
    s
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
    fn finds_metapackage_depends() {
        let text = "Package: gcc\nArchitecture: amd64\n\
                    Depends: cpp (= 4:13.2.0-7ubuntu1), gcc-13 (>= 13.2.0-11~), gcc-x86-64-linux-gnu (= 4:13.2.0-7ubuntu1)\n\n\
                    Package: gcc-13\nDepends: cpp-13 (= 13.2.0-11~), gcc-13-base (= 13.2.0-11~)\n\n";
        let gz = gzip(text);
        assert_eq!(
            find_depends(&gz, "gcc").as_deref(),
            Some("cpp (= 4:13.2.0-7ubuntu1), gcc-13 (>= 13.2.0-11~), gcc-x86-64-linux-gnu (= 4:13.2.0-7ubuntu1)")
        );
        assert_eq!(find_depends(&gz, "clang"), None);
        assert_eq!(
            find_depends(&gz, "gcc-13").as_deref(),
            Some("cpp-13 (= 13.2.0-11~), gcc-13-base (= 13.2.0-11~)")
        );
    }

    #[test]
    fn extracts_version_from_depends() {
        assert_eq!(
            extract_version(
                "cpp (= 4:13.2.0-7ubuntu1), gcc-13 (>= 13.2.0-11~), gcc-x86-64-linux-gnu (= x)",
                "gcc-"
            )
            .as_deref(),
            Some("13")
        );
        assert_eq!(
            extract_version("clang-18 (>= 18~)", "clang-").as_deref(),
            Some("18")
        );
        assert_eq!(
            extract_version("clang-21 (>= 1:21.1.6-1)", "clang-").as_deref(),
            Some("21")
        );
        assert_eq!(extract_version("gcc-x86-64-linux-gnu (= x)", "gcc-"), None);
        assert_eq!(extract_version("cpp (= x)", "gcc-"), None);
    }

    #[test]
    fn dwarf4_for_clang_15_plus_only() {
        assert!(generate_profile("jammy", CompilerType::Clang, "14")
            .flags
            .is_empty());
        assert_eq!(
            generate_profile("jammy", CompilerType::Clang, "15")
                .flags
                .len(),
            2
        );
        assert_eq!(
            generate_profile("resolute", CompilerType::Clang, "21")
                .flags
                .len(),
            2
        );
        assert!(generate_profile("resolute", CompilerType::Gcc, "15")
            .flags
            .is_empty());
    }

    #[test]
    fn generated_profile_round_trips_through_toml() {
        let p = generate_profile("noble", CompilerType::Clang, "18");
        assert_eq!(p.name, "clang-18-noble");
        let parsed: Profile = toml::from_str(&p.raw_content).unwrap();
        assert_eq!(parsed.compiler.compiler_type, CompilerType::Clang);
        assert_eq!(parsed.compiler.version, "18");
        assert_eq!(parsed.target.series, "noble");
        assert_eq!(parsed.flags.len(), 2);
        assert!(parsed.flags.iter().all(|f| f.flag == "-gdwarf-4"));
        assert!(parsed
            .flags
            .iter()
            .all(|f| f.var == "DEB_CFLAGS_APPEND" || f.var == "DEB_CXXFLAGS_APPEND"));
    }

    #[test]
    fn gcc_profile_has_no_flags_and_round_trips() {
        let p = generate_profile("noble", CompilerType::Gcc, "13");
        assert_eq!(p.name, "gcc-13-noble");
        assert!(p.flags.is_empty());
        let parsed: Profile = toml::from_str(&p.raw_content).unwrap();
        assert_eq!(parsed.compiler.compiler_type, CompilerType::Gcc);
        assert!(parsed.flags.is_empty());
    }
}
