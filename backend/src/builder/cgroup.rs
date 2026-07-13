//! Per-build cgroup v2 management for memory limiting and OOM detection.
//!
//! Operates within the user-delegated cgroup subtree (no root required).
//! Creates a transient cgroup per build, sets `memory.max`, attaches the
//! sbuild process tree, and reads `memory.events` after completion to detect
//! cgroup-local OOM kills.

use anyhow::{Context, Result};
use std::fs;
use std::path::PathBuf;
use uuid::Uuid;

pub struct BuildCgroup {
    path: PathBuf,
}

impl BuildCgroup {
    /// Discover the user-delegated cgroup root by reading `/proc/self/cgroup`.
    /// Returns the full filesystem path (e.g. `/sys/fs/cgroup/user.slice/...`).
    fn discover_cgroup_root() -> Result<PathBuf> {
        let raw = fs::read_to_string("/proc/self/cgroup")
            .context("Failed to read /proc/self/cgroup")?;
        // cgroup v2 format: "0::/user.slice/user-1000.slice/user@1000.service"
        let line = raw.lines().next().ok_or_else(|| {
            anyhow::anyhow!("/proc/self/cgroup is empty")
        })?;
        let path = line
            .splitn(3, ':')
            .nth(2)
            .ok_or_else(|| anyhow::anyhow!("Unexpected /proc/self/cgroup format: {line}"))?;
        if path.is_empty() {
            anyhow::bail!("Empty cgroup path in /proc/self/cgroup");
        }
        Ok(PathBuf::from("/sys/fs/cgroup").join(path))
    }

    /// Create a cgroup for this build under the user-delegated subtree,
    /// with a memory limit set.
    pub fn create(build_id: Uuid, memory_limit_mb: u64) -> Result<Self> {
        let root = Self::discover_cgroup_root()?;
        let path = root.join(format!("rebuild-{build_id}"));

        fs::create_dir_all(&path)
            .with_context(|| format!("Failed to create cgroup at {}", path.display()))?;

        // Set memory limit (convert MB to bytes).
        let limit_bytes = memory_limit_mb * 1024 * 1024;
        fs::write(path.join("memory.max"), limit_bytes.to_string())
            .with_context(|| format!("Failed to set memory.max at {}", path.display()))?;

        Ok(BuildCgroup { path })
    }

    /// Move a process (by PID) into this cgroup by writing to cgroup.procs.
    pub fn add_process(&self, pid: u32) -> Result<()> {
        fs::write(self.path.join("cgroup.procs"), pid.to_string())
            .with_context(|| format!("Failed to add PID {pid} to cgroup at {}", self.path.display()))?;
        Ok(())
    }

    /// Check whether the OOM killer fired inside this cgroup.
    /// Reads `memory.events` and looks for `oom_kill` with a count > 0.
    pub fn was_oom_killed(&self) -> Result<bool> {
        let events = fs::read_to_string(self.path.join("memory.events"))
            .with_context(|| format!("Failed to read memory.events at {}", self.path.display()))?;
        Ok(parse_oom_kill_count(&events) > 0)
    }

    /// Remove the cgroup directory. Call after `was_oom_killed()`.
    pub fn cleanup(self) -> Result<()> {
        fs::remove_dir(&self.path)
            .with_context(|| format!("Failed to remove cgroup at {}", self.path.display()))?;
        Ok(())
    }
}

/// Extract the `oom_kill` count from `memory.events` content.
/// Used by `was_oom_killed()` but extracted for unit testing.
fn parse_oom_kill_count(events: &str) -> u64 {
    for line in events.lines() {
        if let Some(rest) = line.strip_prefix("oom_kill ") {
            return rest.trim().parse().unwrap_or(0);
        }
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_memory_events_no_oom() {
        let events = "low 0\nhigh 0\noom 0\noom_kill 0\noom_pause 0\n";
        let result = parse_oom_kill_count(events);
        assert_eq!(result, 0);
    }

    #[test]
    fn parse_memory_events_with_oom() {
        let events = "low 123\nhigh 456\noom 1\noom_kill 3\noom_pause 0\n";
        let result = parse_oom_kill_count(events);
        assert_eq!(result, 3);
    }

    #[test]
    fn parse_memory_events_missing_oom_kill_line() {
        let events = "low 0\nhigh 0\n";
        let result = parse_oom_kill_count(events);
        assert_eq!(result, 0);
    }
}
