//! Build-log scanning against ERROR_PATTERNS / OBSERVATION_PATTERNS.

mod patterns;

pub use patterns::{match_pattern, ErrorPattern, ERROR_PATTERNS, OBSERVATION_PATTERNS};

use crate::models::{BuildStatus, FindingClass, FindingSeverity};

const MAX_FINDINGS_PER_CATEGORY: usize = 5;

#[derive(Debug, Clone)]
pub struct Finding {
    pub category: String,
    pub description: String,
    pub excerpt: String,
    /// 1-indexed.
    pub line_number: usize,
    pub severity: FindingSeverity,
    pub class: FindingClass,
}

pub fn scan_log(log: &str, status: BuildStatus) -> Vec<Finding> {
    match status {
        s if s.should_scan_for_errors() => scan(log, ERROR_PATTERNS, FindingSeverity::Error),
        s if s.should_scan_for_observations() => {
            scan(log, OBSERVATION_PATTERNS, FindingSeverity::Observation)
        }
        _ => vec![],
    }
}

fn scan(log: &str, patterns: &[&ErrorPattern], severity: FindingSeverity) -> Vec<Finding> {
    let lines: Vec<&str> = log.lines().collect();
    let mut findings: Vec<Finding> = Vec::new();

    let mut seen: std::collections::HashSet<(String, String)> = std::collections::HashSet::new();
    let mut category_counts: std::collections::HashMap<String, usize> =
        std::collections::HashMap::new();

    for (idx, line) in lines.iter().enumerate() {
        // Pure warnings only matter under -Werror, which emits an `error:` line too.
        if severity == FindingSeverity::Error
            && line.contains("warning:")
            && !line.contains("error:")
        {
            continue;
        }

        let Some(pattern) = match_pattern(line, patterns) else {
            continue;
        };

        let extracted_key = if pattern.dedup_by_extracted_key {
            extract_key(line, pattern)
        } else {
            String::new()
        };

        if !seen.insert((pattern.key.to_string(), extracted_key.clone())) {
            continue;
        }

        let count = category_counts.entry(pattern.key.to_string()).or_insert(0);
        *count += 1;

        if *count > MAX_FINDINGS_PER_CATEGORY {
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
            class: pattern.class,
        });
    }

    // "and N more" summary for capped categories.
    for (category, count) in &category_counts {
        if *count > MAX_FINDINGS_PER_CATEGORY {
            let overflow = count - MAX_FINDINGS_PER_CATEGORY;
            let pattern = patterns.iter().find(|p| p.key == category.as_str());
            let base_desc = pattern
                .map(|p| p.description)
                .unwrap_or("additional occurrences");
            let class = pattern.map(|p| p.class).unwrap_or(FindingClass::Toolchain);
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
                class,
            });
        }
    }

    // Catch-alls (LINK_FAILURE) drop when a `suppressed_by` category also matched.
    let present: std::collections::HashSet<String> =
        findings.iter().map(|f| f.category.clone()).collect();
    findings.retain(|f| {
        let suppressed_by = patterns
            .iter()
            .find(|p| p.key == f.category)
            .map(|p| p.suppressed_by)
            .unwrap_or(&[]);
        !suppressed_by.iter().any(|c| present.contains(*c))
    });

    findings
}

/// Dedup key from a matching line: quoted token, or the -l name for
/// LINK_MISSING_LIBRARY.
fn extract_key(line: &str, pattern: &ErrorPattern) -> String {
    if let Some(start) = line.find('`') {
        let rest = &line[start + 1..];
        let end = rest.find(['\'', '`']).unwrap_or(rest.len().min(80));
        let candidate = &rest[..end];
        if !candidate.is_empty() && candidate.len() < 120 {
            return candidate.to_string();
        }
    }

    if let Some(start) = line.find('\'') {
        let rest = &line[start + 1..];
        if let Some(end) = rest.find('\'') {
            let candidate = &rest[..end];
            if !candidate.is_empty() && candidate.len() < 120 {
                return candidate.to_string();
            }
        }
    }

    // "cannot find -lfoo" has no quotes.
    if pattern.key == "LINK_MISSING_LIBRARY" {
        if let Some(pos) = line.find("-l") {
            let rest = &line[pos + 2..];
            let end = rest
                .find(|c: char| c.is_whitespace() || c == '\'')
                .unwrap_or(rest.len().min(60));
            let candidate = &rest[..end];
            if !candidate.is_empty() {
                return candidate.to_string();
            }
        }
    }

    String::new()
}

fn extract_context(lines: &[&str], line_idx: usize, context: usize) -> String {
    let start = line_idx.saturating_sub(context);
    let end = (line_idx + context + 1).min(lines.len());
    lines[start..end].join("\n")
}

pub fn infer_status(log: &str, exit_code: Option<i32>) -> BuildStatus {
    // "Timed out" alone matches test-suite output ("panic: test timed out").
    if log.contains("Build killed") {
        return BuildStatus::Timeout;
    }

    if log.contains("unsatisfiable build-dependencies")
        || log.contains("build-dependency not installable")
        || log.contains("Dependency wait")
    {
        return BuildStatus::DepWait;
    }

    let strong_success = log.contains("Build finished successfully")
        || log.contains("dpkg-buildpackage: info: binary-only upload");

    if strong_success {
        return if exit_code.is_none_or(|c| c == 0) {
            BuildStatus::Succeeded
        } else {
            BuildStatus::Failed
        };
    }

    if exit_code == Some(0)
        && log.contains("dpkg-deb: building package")
        && !log.contains("error:")
        && !log.contains("FAILED")
        && !log.contains("Build failure")
    {
        return BuildStatus::Succeeded;
    }

    BuildStatus::Failed
}

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
    fn fat_lto_command_lines_produce_no_observation() {
        let log = "gcc -g -O2 -flto=auto -ffat-lto-objects -c -o a.o a.c\n\
                   gcc -g -O2 -flto=auto -ffat-lto-objects -c -o b.o b.c\n\
                   Build finished successfully";
        let findings = scan_log(log, BuildStatus::Succeeded);
        assert!(
            findings
                .iter()
                .all(|f| f.category != "LTO_FAT_OBJECTS_IGNORED"),
            "command-line occurrences of -ffat-lto-objects must not produce findings"
        );
    }

    #[test]
    fn environmental_class_assigned_to_install_race() {
        use crate::models::FindingClass;
        let log = "install: cannot create directory '/build/x/usr/lib/udev/rules.d'\n\
                   make[3]: *** [Makefile:63: 55-dm_install] Error 1";
        let findings = scan_log(log, BuildStatus::Failed);
        let race = findings
            .iter()
            .find(|f| f.category == "PARALLEL_INSTALL_RACE")
            .unwrap();
        assert_eq!(race.class, FindingClass::Environmental);
    }

    #[test]
    fn toolchain_class_assigned_to_compiler_error() {
        use crate::models::FindingClass;
        let log = "bogl-font.c:84:3: error: function definition is not allowed here";
        let findings = scan_log(log, BuildStatus::Failed);
        let f = findings
            .iter()
            .find(|f| f.category == "GNU_NESTED_FUNCTIONS")
            .unwrap();
        assert_eq!(f.class, FindingClass::Toolchain);
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
        let log = "clang: warning: optimization flag '-ffat-lto-objects' is not supported [-Wignored-optimization-argument]\n\
                   bogl-font.c:84:3: error: function definition is not allowed here";
        let findings = scan_log(log, BuildStatus::Failed);
        assert!(findings
            .iter()
            .all(|f| f.category != "LTO_FAT_OBJECTS_IGNORED"));
        assert!(findings
            .iter()
            .any(|f| f.category == "GNU_NESTED_FUNCTIONS"));
    }

    #[test]
    fn per_symbol_dedup_produces_multiple_findings() {
        let log = "/usr/bin/ld: undefined reference to `foo'\n\
                   /usr/bin/ld: undefined reference to `bar'\n\
                   /usr/bin/ld: undefined reference to `baz'";
        let findings = scan_log(log, BuildStatus::Failed);
        let link_findings: Vec<_> = findings
            .iter()
            .filter(|f| f.category == "LINK_MISSING_SYMBOL")
            .collect();
        assert_eq!(link_findings.len(), 3);
    }

    #[test]
    fn link_failure_suppressed_when_specific_cause_present() {
        let log = "process.c:6500: undefined reference to `crypt'\n\
                   collect2: error: ld returned 1 exit status\n\
                   make[2]: *** [Makefile:79: screen] Error 1";
        let findings = scan_log(log, BuildStatus::Failed);
        assert!(
            findings.iter().any(|f| f.category == "LINK_MISSING_SYMBOL"),
            "specific link cause must be present"
        );
        assert!(
            findings.iter().all(|f| f.category != "LINK_FAILURE"),
            "generic LINK_FAILURE must be suppressed when a specific cause exists"
        );
    }

    #[test]
    fn link_failure_kept_when_no_specific_cause() {
        let log = "collect2: error: ld returned 1 exit status\n\
                   make[2]: *** [Makefile:79: thing] Error 1";
        let findings = scan_log(log, BuildStatus::Failed);
        assert!(
            findings.iter().any(|f| f.category == "LINK_FAILURE"),
            "LINK_FAILURE must be kept when it is the only link finding"
        );
    }

    #[test]
    fn parallel_install_race_is_categorised() {
        let log = "install: cannot create directory '/build/x/usr/lib/udev/rules.d'\n\
                   make[3]: *** [Makefile:63: 55-dm_install] Error 1";
        let findings = scan_log(log, BuildStatus::Failed);
        assert!(
            findings
                .iter()
                .any(|f| f.category == "PARALLEL_INSTALL_RACE"),
            "install-directory race must be categorised"
        );
    }

    #[test]
    fn cap_at_max_with_summary() {
        let syms = ["a", "b", "c", "d", "e", "f", "g"];
        let log = syms
            .iter()
            .map(|s| format!("/usr/bin/ld: undefined reference to `{s}'"))
            .collect::<Vec<_>>()
            .join("\n");
        let findings = scan_log(&log, BuildStatus::Failed);
        let link_findings: Vec<_> = findings
            .iter()
            .filter(|f| f.category == "LINK_MISSING_SYMBOL")
            .collect();
        assert_eq!(link_findings.len(), 6);
        assert!(link_findings
            .last()
            .unwrap()
            .description
            .contains("additional occurrence"));
    }

    #[test]
    fn pure_warning_line_skipped_in_error_scan() {
        let log =
            "barcode.c:42:5: warning: format string is not a string literal [-Wformat-security]";
        let findings = scan_log(log, BuildStatus::Failed);
        assert!(
            findings.is_empty(),
            "pure warning lines must not produce error findings"
        );
    }

    #[test]
    fn werror_promoted_warning_is_found() {
        let log = "barcode.c:42:5: error: format string is not a string literal [-Werror,-Wformat-security]";
        let findings = scan_log(log, BuildStatus::Failed);
        assert!(!findings.is_empty());
    }

    #[test]
    fn infer_status_depwait() {
        assert_eq!(
            infer_status("unsatisfiable build-dependencies", None),
            BuildStatus::DepWait
        );
    }

    #[test]
    fn infer_status_timeout() {
        assert_eq!(
            infer_status("Build killed with signal TERM", None),
            BuildStatus::Timeout
        );
    }

    #[test]
    fn infer_status_success() {
        assert_eq!(
            infer_status("Build finished successfully", Some(0)),
            BuildStatus::Succeeded
        );
        assert_eq!(
            infer_status("Build finished successfully", None),
            BuildStatus::Succeeded
        );
    }

    #[test]
    fn infer_status_nonzero_exit_suppresses_success() {
        assert_eq!(
            infer_status("Build finished successfully", Some(1)),
            BuildStatus::Failed
        );
    }

    #[test]
    fn infer_status_unknown_exit_rejects_weak_success_marker() {
        let log = "dpkg-deb: building package 'hello'\ndpkg-deb: building package 'hello-dbgsym'";
        assert_eq!(infer_status(log, None), BuildStatus::Failed);
        assert_eq!(infer_status(log, Some(0)), BuildStatus::Succeeded);
    }

    #[test]
    fn infer_status_unknown_exit_accepts_strong_success_marker() {
        assert_eq!(
            infer_status("Build finished successfully", None),
            BuildStatus::Succeeded
        );
        assert_eq!(
            infer_status("dpkg-buildpackage: info: binary-only upload", None),
            BuildStatus::Succeeded
        );
    }

    #[test]
    fn infer_status_test_suite_timed_out_is_not_timeout() {
        let log = "--- FAIL: TestFoo\npanic: test timed out after 30s\nFAIL";
        assert_eq!(infer_status(log, Some(1)), BuildStatus::Failed);
        assert_ne!(infer_status(log, None), BuildStatus::Timeout);
    }
}
