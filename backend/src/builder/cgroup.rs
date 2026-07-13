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

impl Drop for BuildCgroup {
    fn drop(&mut self) {
        let _ = fs::remove_dir(&self.path);
    }
}

impl BuildCgroup {
    /// Discover the current process's cgroup path by reading `/proc/self/cgroup`.
    /// Returns the cgroup v2 hierarchy path (e.g. `/user.slice/user-1000.slice/...`).
    fn read_cgroup_v2_path() -> Result<String> {
        let raw = fs::read_to_string("/proc/self/cgroup")
            .context("Failed to read /proc/self/cgroup")?;
        // cgroup v2 format: "0::/user.slice/user-1000.slice/user@1000.service"
        // On hybrid systems, filter for the v2 entry (starts with "0::").
        let line = raw
            .lines()
            .find(|l| l.starts_with("0::"))
            .ok_or_else(|| anyhow::anyhow!("No cgroup v2 entry found in /proc/self/cgroup"))?;
        let path = line
            .strip_prefix("0::")
            .ok_or_else(|| anyhow::anyhow!("Unexpected cgroup v2 format: {line}"))?;
        if path.is_empty() {
            anyhow::bail!("Empty cgroup path in /proc/self/cgroup");
        }
        Ok(path.to_string())
    }

    /// Find a writable parent cgroup where we can create a memory-limited child.
    ///
    /// Walks up the cgroup hierarchy from the current process's cgroup, looking
    /// for the first ancestor where:
    ///   1. `memory` is available in `cgroup.controllers`
    ///   2. We can enable it in `cgroup.subtree_control`
    ///
    /// This handles the common case where the process is inside a terminal scope
    /// (e.g. `app-ghostty-surface-transient-*.scope`) that doesn't have the
    /// memory controller delegated. The standard delegation point is
    /// `user@UID.service`, which is an ancestor of the terminal scope.
    fn find_writable_parent() -> Result<PathBuf> {
        let cgroup_path = Self::read_cgroup_v2_path()?;
        let full = PathBuf::from("/sys/fs/cgroup").join(&cgroup_path);

        // Walk up from the current cgroup to the root, trying each level.
        let mut current = Some(full.as_path());
        while let Some(dir) = current {
            // Check if `memory` is available at this level.
            let controllers = fs::read_to_string(dir.join("cgroup.controllers"))
                .unwrap_or_default();
            if controllers.split_whitespace().any(|c| c == "memory") {
                // Try to enable memory for children at this level.
                // This may fail with EPERM if we don't own this cgroup,
                // but that's fine — we'll try the parent next.
                let _ = fs::write(dir.join("cgroup.subtree_control"), "+memory");
                // Verify it actually got enabled.
                let subtree = fs::read_to_string(dir.join("cgroup.subtree_control"))
                    .unwrap_or_default();
                if subtree.split_whitespace().any(|c| c == "memory") {
                    return Ok(dir.to_path_buf());
                }
            }
            current = dir.parent();
        }
        anyhow::bail!(
            "No writable cgroup parent with memory controller found \
             (walked up from {cgroup_path}). \
             Ensure cgroup v2 delegation is enabled for your user session."
        )
    }

    /// Create a cgroup for this build under the user-delegated subtree,
    /// with a memory limit set.
    pub fn create(build_id: Uuid, memory_limit_mb: u64) -> Result<Self> {
        let parent = Self::find_writable_parent()?;
        let path = parent.join(format!("rebuild-{build_id}"));

        fs::create_dir(&path)
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
