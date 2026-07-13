//! Core data models for rebuild experiments.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Outcome of a package build attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BuildStatus {
    Pending,
    Building,
    Succeeded,
    Failed,
    DepWait,
    Timeout,
    OomKilled,
}

impl BuildStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Building => "building",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::DepWait => "dep_wait",
            Self::Timeout => "timeout",
            Self::OomKilled => "oom_killed",
        }
    }

    /// Returns true if the build has reached a final state.
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Succeeded | Self::Failed | Self::DepWait | Self::Timeout | Self::OomKilled)
    }

    /// Returns true if the build log should be scanned for error-level findings.
    ///
    /// Only failed builds produce actionable error findings.
    pub fn should_scan_for_errors(&self) -> bool {
        matches!(self, Self::Failed)
    }

    /// Returns true if the build log should be scanned for observations.
    ///
    /// Succeeded builds may contain non-fatal compiler warnings worth noting.
    pub fn should_scan_for_observations(&self) -> bool {
        matches!(self, Self::Succeeded)
    }
}

impl std::str::FromStr for BuildStatus {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s {
            "pending" => Ok(Self::Pending),
            "building" => Ok(Self::Building),
            "succeeded" => Ok(Self::Succeeded),
            "failed" => Ok(Self::Failed),
            "dep_wait" => Ok(Self::DepWait),
            "timeout" => Ok(Self::Timeout),
            "oom_killed" => Ok(Self::OomKilled),
            other => Err(format!("unknown build status: {other}")),
        }
    }
}

impl std::fmt::Display for BuildStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Severity of a build finding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FindingSeverity {
    /// A finding that contributed to a build failure.
    Error,
    /// A finding on a succeeded build — the build completed despite the issue,
    /// but the issue is worth noting for toolchain analysis.
    Observation,
}

impl FindingSeverity {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Observation => "observation",
        }
    }
}

impl std::str::FromStr for FindingSeverity {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s {
            "error" => Ok(Self::Error),
            "observation" => Ok(Self::Observation),
            other => Err(format!("unknown finding severity: {other}")),
        }
    }
}

impl std::fmt::Display for FindingSeverity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Whether a finding reflects a toolchain (compiler) issue or an environmental
/// / infrastructure artifact unrelated to the GCC-vs-Clang comparison.
///
/// This lets the analysis separate "this package is broken under Clang" from
/// "this build hit a flaky/infra problem" (e.g. a parallel-install race or a
/// source-fetch failure) so the latter don't count against a compiler.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FindingClass {
    /// A genuine compiler/toolchain incompatibility (the default).
    Toolchain,
    /// An environmental or infrastructure artifact, independent of the
    /// compiler under test (e.g. parallel-install races, source-fetch
    /// failures). Should be excluded from compiler-comparison success rates.
    Environmental,
}

impl FindingClass {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Toolchain => "toolchain",
            Self::Environmental => "environmental",
        }
    }
}

impl std::str::FromStr for FindingClass {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s {
            "toolchain" => Ok(Self::Toolchain),
            "environmental" => Ok(Self::Environmental),
            other => Err(format!("unknown finding class: {other}")),
        }
    }
}

impl std::fmt::Display for FindingClass {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// How builds were executed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BuilderBackend {
    Sbuild,
    Launchpad,
    External,
}

impl BuilderBackend {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Sbuild => "sbuild",
            Self::Launchpad => "launchpad",
            Self::External => "external",
        }
    }
}

impl std::str::FromStr for BuilderBackend {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s {
            "sbuild" => Ok(Self::Sbuild),
            "launchpad" => Ok(Self::Launchpad),
            "external" => Ok(Self::External),
            other => Err(format!("unknown builder backend: {other}")),
        }
    }
}

impl std::fmt::Display for BuilderBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A batch is a collection of builds sharing a compiler profile and target
/// architecture.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Batch {
    pub id: Uuid,
    pub name: String,
    pub compiler_type: String,
    pub compiler_version: String,
    pub series: String,
    /// Target architecture for every build in this batch (e.g. "amd64").
    /// A batch is implicitly single-arch: sbuild is invoked once per package
    /// with the same `--arch`.
    pub arch: String,
    pub profile_name: String,
    pub profile_content: String,
    pub builder_backend: BuilderBackend,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
}

/// A single package build attempt, as stored in the database.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Build {
    pub id: Uuid,
    pub batch_id: Uuid,
    pub source_package: String,
    pub version: String,
    pub status: BuildStatus,
    pub build_duration_seconds: Option<f64>,
    pub peak_memory_mb: Option<i64>,
    pub build_log: Option<String>,
    pub compiler_detected: Option<String>,
    pub submitted_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
    /// Archive component the source package belongs to (main / universe /
    /// restricted / multiverse).  `None` for legacy rows and bare-name
    /// package lists that did not carry component metadata.
    pub component: Option<String>,
}

/// An error finding or observation from build-log analysis.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuildFinding {
    pub id: Uuid,
    pub build_id: Uuid,
    pub category: String,
    pub description: String,
    pub excerpt: String,
    pub line_number: Option<i64>,
    pub severity: FindingSeverity,
    pub class: FindingClass,
}

/// Result from running a single build (before database insertion).
#[derive(Debug, Clone)]
pub struct BuildResult {
    pub source_package: String,
    pub version: String,
    pub status: BuildStatus,
    pub build_duration_seconds: Option<f64>,
    pub peak_memory_mb: Option<i64>,
    pub build_log: String,
    pub compiler_detected: Option<String>,
    /// Archive component the source package belongs to, if known from the
    /// package list.  Forwarded to `NewBuild` for persistence.
    pub component: Option<String>,
}

/// How build logs are stored.
///
/// `All`      — compress and store every log (default, backward-compatible).
/// `Failures` — compress and store only failed/timeout/dep_wait builds;
///              succeeded build logs are scanned for observations then dropped.
/// `None`     — scan for findings then drop all logs; nothing stored in the DB.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StoreLogs {
    #[default]
    All,
    Failures,
    None,
}

impl std::str::FromStr for StoreLogs {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s {
            "all" => Ok(Self::All),
            "failures" => Ok(Self::Failures),
            "none" => Ok(Self::None),
            other => Err(format!("unknown store-logs value '{other}': expected all, failures, or none")),
        }
    }
}

impl std::fmt::Display for StoreLogs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::All => f.write_str("all"),
            Self::Failures => f.write_str("failures"),
            Self::None => f.write_str("none"),
        }
    }
}

/// Resource usage metrics parsed from `/usr/bin/time -v` output.
#[derive(Debug, Clone, Default)]
pub struct ResourceMetrics {
    pub wall_time_seconds: Option<f64>,
    pub user_time_seconds: Option<f64>,
    pub system_time_seconds: Option<f64>,
    pub peak_memory_kb: Option<i64>,
    pub exit_status: Option<i32>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oom_killed_is_terminal() {
        assert!(BuildStatus::OomKilled.is_terminal());
    }

    #[test]
    fn oom_killed_as_str() {
        assert_eq!(BuildStatus::OomKilled.as_str(), "oom_killed");
    }

    #[test]
    fn oom_killed_from_str() {
        let status: BuildStatus = "oom_killed".parse().unwrap();
        assert_eq!(status, BuildStatus::OomKilled);
    }

    #[test]
    fn oom_killed_display() {
        assert_eq!(format!("{}", BuildStatus::OomKilled), "oom_killed");
    }

    #[test]
    fn oom_killed_does_not_scan_for_errors() {
        assert!(!BuildStatus::OomKilled.should_scan_for_errors());
    }

    #[test]
    fn oom_killed_does_not_scan_for_observations() {
        assert!(!BuildStatus::OomKilled.should_scan_for_observations());
    }
}
