//! Core data models.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

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

    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::Succeeded | Self::Failed | Self::DepWait | Self::Timeout | Self::OomKilled
        )
    }

    pub fn should_scan_for_errors(&self) -> bool {
        matches!(self, Self::Failed)
    }

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FindingSeverity {
    Error,
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

/// Environmental findings are excluded from compiler comparison.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FindingClass {
    Toolchain,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Batch {
    pub id: Uuid,
    pub name: String,
    pub compiler_type: String,
    pub compiler_version: String,
    pub series: String,
    pub arch: String,
    pub profile_name: String,
    pub profile_content: String,
    pub builder_backend: BuilderBackend,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Build {
    pub id: Uuid,
    pub batch_id: Uuid,
    pub source_package: String,
    pub version: String,
    pub status: BuildStatus,
    pub build_duration_seconds: Option<f64>,
    pub peak_memory_mb: Option<i64>,
    /// Use db::get_build_log(); logs can be gigabytes.
    pub compiler_detected: Option<String>,
    pub submitted_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
    /// NULL for legacy rows.
    pub component: Option<String>,
    pub attempt_number: i64,
    pub jobs: Option<i64>,
    pub memory_limit_mb: Option<i64>,
}

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

#[derive(Debug, Clone)]
pub struct BuildResult {
    pub source_package: String,
    pub version: String,
    pub status: BuildStatus,
    pub build_duration_seconds: Option<f64>,
    pub peak_memory_mb: Option<i64>,
    pub build_log: String,
    pub compiler_detected: Option<String>,
    pub component: Option<String>,
    pub jobs: usize,
    pub memory_limit_mb: Option<u64>,
    pub attempt_number: u32,
}

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
            other => Err(format!(
                "unknown store-logs value '{other}': expected all, failures, or none"
            )),
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
