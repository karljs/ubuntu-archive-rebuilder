//! Log scanning and error categorization

mod patterns;

pub use patterns::{match_pattern, ErrorPattern, CLANG_ERROR_PATTERNS};

use crate::models::BuildStatus;

/// A finding extracted from a build log
#[derive(Debug, Clone)]
pub struct Finding {
    /// Error category key
    pub category: String,
    /// Human-readable description
    pub description: String,
    /// Log excerpt with context
    pub excerpt: String,
    /// Line number in the log (1-indexed)
    pub line_number: usize,
}

/// Scan a build log and extract error findings
///
/// Returns a list of findings, deduplicated by category (only first occurrence kept)
pub fn scan_log(log: &str) -> Vec<Finding> {
    let lines: Vec<&str> = log.lines().collect();
    let mut findings = Vec::new();
    let mut seen_categories = std::collections::HashSet::new();

    for (idx, line) in lines.iter().enumerate() {
        // Skip lines that are pure compiler warnings — they are only a finding
        // if the build also uses -Werror and the warning is subsequently promoted
        // to an error (which would appear on a separate "error:" line).
        // This prevents format-string and -Wunused warnings from showing as
        // findings on builds that don't use -Werror.
        if line.contains("warning:") && !line.contains("error:") {
            continue;
        }

        if let Some(pattern) = match_pattern(line) {
            if seen_categories.insert(pattern.key) {
                let excerpt = extract_context(&lines, idx, 2);
                findings.push(Finding {
                    category: pattern.key.to_string(),
                    description: pattern.description.to_string(),
                    excerpt,
                    line_number: idx + 1,
                });
            }
        }
    }

    findings
}

/// Extract context lines around a given line
fn extract_context(lines: &[&str], line_idx: usize, context: usize) -> String {
    let start = line_idx.saturating_sub(context);
    let end = (line_idx + context + 1).min(lines.len());
    lines[start..end].join("\n")
}

/// Determine build status from log content and an optional process exit code.
///
/// `exit_code` should be the exit status of the build process when known (the
/// builder passes it; the log importer passes `None`). A non-zero exit code
/// prevents a success determination even when a success marker is present in
/// the log, which guards against partial logs that end mid-flight.
///
/// Checks are ordered from most specific to least: timeout, dep-wait, success,
/// then failure as the default.
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

    // The dpkg-deb line alone is not enough to call a build successful;
    // guard it against error markers that can appear in the same log.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_scan_log_finds_errors() {
        let log = r#"
Building package foo
In file included from main.c:1:
fatal error: 'omp.h' file not found
#include <omp.h>
         ^~~~~~~
1 error generated.
make: *** [Makefile:10: main.o] Error 1
"#;
        let findings = scan_log(log);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].category, "OPENMP_NOT_AVAILABLE");
    }

    #[test]
    fn test_scan_log_deduplicates() {
        let log = r#"
error: undefined reference to `foo'
error: undefined reference to `bar'
error: undefined reference to `baz'
"#;
        let findings = scan_log(log);
        // Should only have one finding even though pattern matches 3 times
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].category, "LINKER_UNDEFINED_REF");
    }

    #[test]
    fn test_infer_status_depwait() {
        let log = "E: unsatisfiable build-dependencies for package";
        assert_eq!(infer_status(log, None), BuildStatus::DepWait);
    }

    #[test]
    fn test_infer_status_timeout() {
        let log = "Build killed with signal TERM";
        assert_eq!(infer_status(log, None), BuildStatus::Timeout);
    }

    #[test]
    fn test_infer_status_success() {
        let log = "Build finished successfully";
        assert_eq!(infer_status(log, Some(0)), BuildStatus::Succeeded);
        assert_eq!(infer_status(log, None), BuildStatus::Succeeded);
    }

    #[test]
    fn test_infer_status_nonzero_exit_suppresses_success() {
        // A success marker in the log should not win against a non-zero exit.
        let log = "Build finished successfully";
        assert_eq!(infer_status(log, Some(1)), BuildStatus::Failed);
    }

    #[test]
    fn test_warning_only_line_not_a_finding() {
        // A bare warning (e.g. barcode with -Wformat-security) should not
        // produce a finding — only promoted errors (lines with "error:") count.
        let log = "barcode.c:42:5: warning: format string is not a string literal [-Wformat-security]";
        let findings = scan_log(log);
        assert!(findings.is_empty(), "pure warning lines must not produce findings");
    }

    #[test]
    fn test_werror_promoted_warning_is_a_finding() {
        // When -Werror promotes a warning to an error, the follow-up "error:" line
        // should still be caught.
        let log = "barcode.c:42:5: warning: format string is not a string literal [-Wformat-security]\n\
                   barcode.c:42:5: error: format string is not a string literal [-Werror,-Wformat-security]";
        let findings = scan_log(log);
        assert!(!findings.is_empty(), "Werror-promoted error lines must be caught");
    }

    #[test]
    fn test_infer_status_dependency_wait_sbuild_marker() {
        // "Dependency wait" is the sbuild-specific marker not present in the
        // old infer_status_from_log; verify it is handled.
        let log = "Dependency wait: libfoo-dev";
        assert_eq!(infer_status(log, Some(1)), BuildStatus::DepWait);
    }
}
