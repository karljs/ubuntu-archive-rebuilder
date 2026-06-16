//! Parser for /usr/bin/time -v output

use crate::models::ResourceMetrics;
use tracing::warn;

/// Parse the output of `/usr/bin/time -v` to extract resource metrics.
///
/// Returns a fully populated `ResourceMetrics` on success. Individual fields
/// are `None` when their line is absent from the output. If the output string
/// is non-empty but no recognised field is found, a warning is logged so that
/// silent failures don't go unnoticed in production runs.
///
/// The output format looks like:
/// ```text
///     Command being timed: "some command"
///     User time (seconds): 123.45
///     System time (seconds): 12.34
///     Percent of CPU this job got: 95%
///     Elapsed (wall clock) time (h:mm:ss or m:ss): 2:15:30
///     ...
///     Maximum resident set size (kbytes): 1048576
///     ...
///     Exit status: 0
/// ```
pub fn parse_time_output(output: &str) -> ResourceMetrics {
    let mut metrics = ResourceMetrics::default();

    for line in output.lines() {
        let line = line.trim();

        if let Some(value) = line.strip_prefix("User time (seconds): ") {
            metrics.user_time_seconds = value.parse().ok();
        } else if let Some(value) = line.strip_prefix("System time (seconds): ") {
            metrics.system_time_seconds = value.parse().ok();
        } else if let Some(value) = line.strip_prefix("Maximum resident set size (kbytes): ") {
            metrics.peak_memory_kb = value.parse().ok();
        } else if let Some(value) = line.strip_prefix("Elapsed (wall clock) time (h:mm:ss or m:ss): ") {
            metrics.wall_time_seconds = parse_wall_time(value);
        } else if let Some(value) = line.strip_prefix("Exit status: ") {
            metrics.exit_status = value.parse().ok();
        }
    }

    if !output.trim().is_empty() && metrics.exit_status.is_none() && metrics.wall_time_seconds.is_none() {
        warn!("Non-empty /usr/bin/time output but no recognisable fields parsed — check time output format");
    }

    metrics
}

/// Parse wall time in format "h:mm:ss" or "m:ss" or "ss.ss"
fn parse_wall_time(s: &str) -> Option<f64> {
    let parts: Vec<&str> = s.split(':').collect();

    match parts.len() {
        1 => {
            // Just seconds (possibly with decimals)
            parts[0].parse().ok()
        }
        2 => {
            // m:ss or m:ss.ss
            let minutes: f64 = parts[0].parse().ok()?;
            let seconds: f64 = parts[1].parse().ok()?;
            Some(minutes * 60.0 + seconds)
        }
        3 => {
            // h:mm:ss or h:mm:ss.ss
            let hours: f64 = parts[0].parse().ok()?;
            let minutes: f64 = parts[1].parse().ok()?;
            let seconds: f64 = parts[2].parse().ok()?;
            Some(hours * 3600.0 + minutes * 60.0 + seconds)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_TIME_OUTPUT: &str = r#"
        Command being timed: "sleep 2"
        User time (seconds): 0.00
        System time (seconds): 0.00
        Percent of CPU this job got: 0%
        Elapsed (wall clock) time (h:mm:ss or m:ss): 0:02.00
        Average shared text size (kbytes): 0
        Average unshared data size (kbytes): 0
        Average stack size (kbytes): 0
        Average total size (kbytes): 0
        Maximum resident set size (kbytes): 2048
        Average resident set size (kbytes): 0
        Major (requiring I/O) page faults: 0
        Minor (reclaiming a frame) page faults: 67
        Voluntary context switches: 2
        Involuntary context switches: 0
        Swaps: 0
        File system inputs: 0
        File system outputs: 0
        Socket messages sent: 0
        Socket messages received: 0
        Signals delivered: 0
        Page size (bytes): 4096
        Exit status: 0
    "#;

    #[test]
    fn test_parse_time_output() {
        let metrics = parse_time_output(SAMPLE_TIME_OUTPUT);

        assert_eq!(metrics.user_time_seconds, Some(0.0));
        assert_eq!(metrics.system_time_seconds, Some(0.0));
        assert_eq!(metrics.peak_memory_kb, Some(2048));
        assert_eq!(metrics.wall_time_seconds, Some(2.0));
        assert_eq!(metrics.exit_status, Some(0));
    }

    #[test]
    fn test_parse_wall_time_seconds() {
        assert_eq!(parse_wall_time("5.50"), Some(5.5));
    }

    #[test]
    fn test_parse_wall_time_minutes_seconds() {
        assert_eq!(parse_wall_time("2:30.00"), Some(150.0));
        assert_eq!(parse_wall_time("0:02.00"), Some(2.0));
    }

    #[test]
    fn test_parse_wall_time_hours() {
        assert_eq!(parse_wall_time("1:30:00"), Some(5400.0));
        assert_eq!(parse_wall_time("2:15:30"), Some(8130.0));
    }

    const REAL_BUILD_OUTPUT: &str = r#"
        Command being timed: "sbuild --chroot-mode=unshare --dist=noble hello_2.10-3.dsc"
        User time (seconds): 45.23
        System time (seconds): 12.87
        Percent of CPU this job got: 89%
        Elapsed (wall clock) time (h:mm:ss or m:ss): 1:05.12
        Average shared text size (kbytes): 0
        Average unshared data size (kbytes): 0
        Average stack size (kbytes): 0
        Average total size (kbytes): 0
        Maximum resident set size (kbytes): 524288
        Average resident set size (kbytes): 0
        Major (requiring I/O) page faults: 128
        Minor (reclaiming a frame) page faults: 45678
        Voluntary context switches: 1234
        Involuntary context switches: 567
        Swaps: 0
        File system inputs: 12345
        File system outputs: 67890
        Socket messages sent: 0
        Socket messages received: 0
        Signals delivered: 0
        Page size (bytes): 4096
        Exit status: 0
    "#;

    #[test]
    fn test_parse_real_build_output() {
        let metrics = parse_time_output(REAL_BUILD_OUTPUT);

        assert_eq!(metrics.user_time_seconds, Some(45.23));
        assert_eq!(metrics.system_time_seconds, Some(12.87));
        assert_eq!(metrics.peak_memory_kb, Some(524288));
        assert_eq!(metrics.wall_time_seconds, Some(65.12)); // 1:05.12 = 65.12 seconds
        assert_eq!(metrics.exit_status, Some(0));
    }

    #[test]
    fn test_parse_failed_build() {
        let output = r#"
            Command being timed: "sbuild --dist=noble broken.dsc"
            User time (seconds): 10.00
            System time (seconds): 2.00
            Elapsed (wall clock) time (h:mm:ss or m:ss): 0:15.00
            Maximum resident set size (kbytes): 102400
            Exit status: 1
        "#;

        let metrics = parse_time_output(output);
        assert_eq!(metrics.exit_status, Some(1));
        assert_eq!(metrics.wall_time_seconds, Some(15.0));
    }
}
