//! Log scanning and error/observation categorisation.
//!
//! Two scan modes:
//!
//! - **Error scan** (failed builds): matches [`patterns::ERROR_PATTERNS`] and
//!   returns findings with [`FindingSeverity::Error`].
//!
//! - **Observation scan** (succeeded builds): matches
//!   [`patterns::OBSERVATION_PATTERNS`] and returns findings with
//!   [`FindingSeverity::Observation`].
//!
//! Deduplication is per `(category, extracted_key)` where `extracted_key` is
//! either empty (category-level dedup) or the specific identifier extracted
//! from the matching line (e.g. the undefined symbol name).  Each unique key
//! within a category produces a separate finding, up to a per-category cap of
//! [`MAX_FINDINGS_PER_CATEGORY`]; if there are more, a synthetic summary
//! finding is appended.

mod patterns;

pub use patterns::{match_pattern, ErrorPattern, ERROR_PATTERNS, OBSERVATION_PATTERNS};

use crate::models::{BuildStatus, FindingSeverity};

/// Maximum number of distinct findings per category before a summary is emitted.
const MAX_FINDINGS_PER_CATEGORY: usize = 5;

/// A finding extracted from a build log.
#[derive(Debug, Clone)]
pub struct Finding {
    /// Error category key.
    pub category: String,
    /// Human-readable description.
    pub description: String,
    /// Log excerpt with context lines.
    pub excerpt: String,
    /// Line number in the log (1-indexed).
    pub line_number: usize,
    /// Severity: error (failed build) or observation (succeeded build).
    pub severity: FindingSeverity,
}

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

/// Scan a build log and extract findings appropriate for the build status.
///
/// - Failed builds → error-level findings from [`ERROR_PATTERNS`].
/// - Succeeded builds → observation-level findings from [`OBSERVATION_PATTERNS`].
/// - All other statuses (Timeout, DepWait, Pending, Building) → no findings.
///   Timed-out logs are often truncated and rarely yield clean matches;
///   dep-wait builds have no compilation log worth analysing.
pub fn scan_log(log: &str, status: BuildStatus) -> Vec<Finding> {
    match status {
        s if s.should_scan_for_errors() => scan(log, ERROR_PATTERNS, FindingSeverity::Error),
        s if s.should_scan_for_observations() => {
            scan(log, OBSERVATION_PATTERNS, FindingSeverity::Observation)
        }
        _ => vec![],
    }
}

// ---------------------------------------------------------------------------
// Internal scanner
// ---------------------------------------------------------------------------

fn scan(log: &str, patterns: &[&ErrorPattern], severity: FindingSeverity) -> Vec<Finding> {
    let lines: Vec<&str> = log.lines().collect();
    let mut findings: Vec<Finding> = Vec::new();

    // Track (category, extracted_key) pairs seen so far and counts per category.
    let mut seen: std::collections::HashSet<(String, String)> = std::collections::HashSet::new();
    let mut category_counts: std::collections::HashMap<String, usize> =
        std::collections::HashMap::new();

    for (idx, line) in lines.iter().enumerate() {
        // Skip lines that are pure warnings for the error scan; they are only
        // relevant if the build also uses -Werror, in which case a separate
        // `error:` line will appear and be matched.
        if severity == FindingSeverity::Error
            && line.contains("warning:")
            && !line.contains("error:")
        {
            continue;
        }

        let Some(pattern) = match_pattern(line, patterns) else {
            continue;
        };

        // Extract the deduplication key from the line when requested.
        let extracted_key = if pattern.dedup_by_extracted_key {
            extract_key(line, pattern)
        } else {
            String::new()
        };

        let dedup_pair = (pattern.key.to_string(), extracted_key.clone());
        if !seen.insert(dedup_pair) {
            // Already have this (category, key) pair.
            continue;
        }

        let count = category_counts.entry(pattern.key.to_string()).or_insert(0);
        *count += 1;

        if *count > MAX_FINDINGS_PER_CATEGORY {
            // Already over the cap — don't add more; the summary is added at the end.
            continue;
        }

        let excerpt = extract_context(&lines, idx, 2);
        let description = if pattern.dedup_by_extracted_key && !extracted_key.is_empty() {
            format!("{}: `{}`", pattern.description, extracted_key)
        } else {
            pattern.description.to_string()
        };

        findings.push(Finding {
            category: pattern.key.to_string(),
            description,
            excerpt,
            line_number: idx + 1,
            severity,
        });
    }

    // Append a synthetic "and N more" summary for capped categories.
    for (category, count) in &category_counts {
        if *count > MAX_FINDINGS_PER_CATEGORY {
            let overflow = count - MAX_FINDINGS_PER_CATEGORY;
            // Find the pattern to get the base description.
            let base_desc = patterns
                .iter()
                .find(|p| p.key == category.as_str())
                .map(|p| p.description)
                .unwrap_or("additional occurrences");
            findings.push(Finding {
                category: category.clone(),
                description: format!(
                    "{} ({} additional occurrence{} not shown)",
                    base_desc,
                    overflow,
                    if overflow == 1 { "" } else { "s" }
                ),
                excerpt: String::new(),
                line_number: 0,
                severity,
            });
        }
    }

    findings
}

// ---------------------------------------------------------------------------
// Key extraction for per-symbol deduplication
// ---------------------------------------------------------------------------

/// Extract a meaningful identifier from a matching log line for deduplication.
///
/// Strategy: look for quoted tokens, backtick-quoted identifiers, or the
/// word after a known keyword like "to", "for", "identifier".  Falls back
/// to an empty string (category-level dedup) if nothing useful is found.
fn extract_key<'a>(line: &'a str, pattern: &ErrorPattern) -> String {
    // Try backtick-quoted identifiers: `symbol'  or `symbol`
    if let Some(start) = line.find('`') {
        let rest = &line[start + 1..];
        let end = rest.find(['\'', '`']).unwrap_or(rest.len().min(80));
        let candidate = &rest[..end];
        if !candidate.is_empty() && candidate.len() < 120 {
            return candidate.to_string();
        }
    }

    // Try single-quoted tokens: 'symbol'
    if let Some(start) = line.find('\'') {
        let rest = &line[start + 1..];
        if let Some(end) = rest.find('\'') {
            let candidate = &rest[..end];
            if !candidate.is_empty() && candidate.len() < 120 {
                return candidate.to_string();
            }
        }
    }

    // Fallback for specific pattern types: use first needle word after keyword.
    // E.g. "use of undeclared identifier 'fmt'" — handled by single-quote above.
    // "undefined reference to `foo'" — handled by backtick above.
    // "unknown warning option '-Wlogical-op'" — handled by single-quote above.
    // "cannot find -lfoo" — extract library name.
    if pattern.key == "LINK_MISSING_LIBRARY" {
        if let Some(pos) = line.find("-l") {
            let rest = &line[pos + 2..];
            let end = rest.find(|c: char| c.is_whitespace() || c == '\'').unwrap_or(rest.len().min(60));
            let candidate = &rest[..end];
            if !candidate.is_empty() {
                return candidate.to_string();
            }
        }
    }

    String::new()
}

// ---------------------------------------------------------------------------
// Context extraction
// ---------------------------------------------------------------------------

fn extract_context(lines: &[&str], line_idx: usize, context: usize) -> String {
    let start = line_idx.saturating_sub(context);
    let end = (line_idx + context + 1).min(lines.len());
    lines[start..end].join("\n")
}

// ---------------------------------------------------------------------------
// Status inference (unchanged from original)
// ---------------------------------------------------------------------------

/// Determine build status from log content and an optional process exit code.
///
/// `exit_code` should be the exit status of the build process when known.
/// A non-zero exit code prevents a success determination even when a success
/// marker is present in the log, guarding against partial logs.
pub fn infer_status(log: &str, exit_code: Option<i32>) -> BuildStatus {
    if log.contains("Build killed") || log.contains("Timed out") {
        return BuildStatus::Timeout;
    }

    if log.contains("unsatisfiable build-dependencies")
        || log.contains("build-dependency not installable")
        || log.contains("Dependency wait")
    {
        return BuildStatus::DepWait;
    }

    let clean_exit = exit_code.map_or(true, |c| c == 0);

    if clean_exit
        && (log.contains("Build finished successfully")
            || log.contains("dpkg-buildpackage: info: binary-only upload")
            || (log.contains("dpkg-deb: building package")
                && !log.contains("error:")
                && !log.contains("FAILED")
                && !log.contains("Build failure")))
    {
        return BuildStatus::Succeeded;
    }

    BuildStatus::Failed
}

#[allow(deprecated)]
#[deprecated(note = "use infer_status(log, None) instead")]
pub fn infer_status_from_log(log: &str) -> BuildStatus {
    infer_status(log, None)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_scan_on_failed_build() {
        let log = "bogl-font.c:84:3: error: function definition is not allowed here\n  {\n  ^";
        let findings = scan_log(log, BuildStatus::Failed);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].category, "GNU_NESTED_FUNCTIONS");
        assert_eq!(findings[0].severity, FindingSeverity::Error);
    }

    #[test]
    fn observation_scan_on_succeeded_build() {
        let log = "clang: warning: optimization flag '-ffat-lto-objects' is not supported [-Wignored-optimization-argument]";
        let findings = scan_log(log, BuildStatus::Succeeded);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].category, "LTO_FAT_OBJECTS_IGNORED");
        assert_eq!(findings[0].severity, FindingSeverity::Observation);
    }

    #[test]
    fn no_findings_on_depwait() {
        let log = "unsatisfiable build-dependencies for package";
        let findings = scan_log(log, BuildStatus::DepWait);
        assert!(findings.is_empty());
    }

    #[test]
    fn no_findings_on_timeout() {
        let log = "Build killed with signal TERM after timeout";
        let findings = scan_log(log, BuildStatus::Timeout);
        assert!(findings.is_empty());
    }

    #[test]
    fn fat_lto_on_failed_build_produces_no_finding() {
        // -ffat-lto-objects warning on a failed build should be ignored —
        // it is not an error pattern.
        let log = "clang: warning: optimization flag '-ffat-lto-objects' is not supported [-Wignored-optimization-argument]\n\
                   bogl-font.c:84:3: error: function definition is not allowed here";
        let findings = scan_log(log, BuildStatus::Failed);
        assert!(findings.iter().all(|f| f.category != "LTO_FAT_OBJECTS_IGNORED"));
        assert!(findings.iter().any(|f| f.category == "GNU_NESTED_FUNCTIONS"));
    }

    #[test]
    fn per_symbol_dedup_produces_multiple_findings() {
        let log = "/usr/bin/ld: undefined reference to `foo'\n\
                   /usr/bin/ld: undefined reference to `bar'\n\
                   /usr/bin/ld: undefined reference to `baz'";
        let findings = scan_log(log, BuildStatus::Failed);
        let link_findings: Vec<_> = findings.iter()
            .filter(|f| f.category == "LINK_MISSING_SYMBOL")
            .collect();
        // All three symbols are distinct, should each produce a finding.
        assert_eq!(link_findings.len(), 3);
    }

    #[test]
    fn cap_at_max_with_summary() {
        // 7 distinct undefined references — expect 5 findings + 1 summary.
        let syms = ["a", "b", "c", "d", "e", "f", "g"];
        let log = syms
            .iter()
            .map(|s| format!("/usr/bin/ld: undefined reference to `{s}'"))
            .collect::<Vec<_>>()
            .join("\n");
        let findings = scan_log(&log, BuildStatus::Failed);
        let link_findings: Vec<_> = findings.iter()
            .filter(|f| f.category == "LINK_MISSING_SYMBOL")
            .collect();
        // 5 normal + 1 summary = 6
        assert_eq!(link_findings.len(), 6);
        assert!(link_findings.last().unwrap().description.contains("additional occurrence"));
    }

    #[test]
    fn pure_warning_line_skipped_in_error_scan() {
        let log = "barcode.c:42:5: warning: format string is not a string literal [-Wformat-security]";
        let findings = scan_log(log, BuildStatus::Failed);
        assert!(findings.is_empty(), "pure warning lines must not produce error findings");
    }

    #[test]
    fn werror_promoted_warning_is_found() {
        let log = "barcode.c:42:5: error: format string is not a string literal [-Werror,-Wformat-security]";
        let findings = scan_log(log, BuildStatus::Failed);
        assert!(!findings.is_empty());
    }

    #[test]
    fn infer_status_depwait() {
        assert_eq!(infer_status("unsatisfiable build-dependencies", None), BuildStatus::DepWait);
    }

    #[test]
    fn infer_status_timeout() {
        assert_eq!(infer_status("Build killed with signal TERM", None), BuildStatus::Timeout);
    }

    #[test]
    fn infer_status_success() {
        assert_eq!(infer_status("Build finished successfully", Some(0)), BuildStatus::Succeeded);
        assert_eq!(infer_status("Build finished successfully", None), BuildStatus::Succeeded);
    }

    #[test]
    fn infer_status_nonzero_exit_suppresses_success() {
        assert_eq!(infer_status("Build finished successfully", Some(1)), BuildStatus::Failed);
    }
}
