//! Per-build cgroup v2 management for memory limiting and OOM detection.
//!
//! Operates within the user-delegated cgroup subtree (no root required).
//! Creates a transient cgroup per build, sets `memory.max`, attaches the
//! sbuild process tree, and reads `memory.events` after completion to detect
//! cgroup-local OOM kills.

use anyhow::{Context, Result};
use std::fs;
use std::path::PathBuf;
use tracing::debug;
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

    /// Open a cgroupfs file for writing without O_TRUNC (which cgroupfs rejects).
    fn cgroup_write(path: &std::path::Path, data: &str) -> std::io::Result<()> {
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new().write(true).open(path)?;
        file.write_all(data.as_bytes())?;
        Ok(())
    }

    /// Find a writable parent cgroup where we can create a memory-limited child,
    /// and create the child there.
    ///
    /// Walks up the cgroup hierarchy from the current process's cgroup. At each
    /// level, tries to:
    ///   1. Enable `memory` in `cgroup.subtree_control` (may already be enabled)
    ///   2. Create a child cgroup directory
    ///   3. Write `memory.max`
    ///
    /// If all three succeed, the child cgroup is ready. If any step fails, cleans
    /// up and tries the parent. This handles the common case where the process is
    /// inside a terminal scope that doesn't have the memory controller delegated —
    /// it walks up to `user@UID.service` or `app.slice` where memory is already
    /// enabled.
    fn find_and_create_child(build_id: Uuid, memory_limit_mb: u64) -> Result<PathBuf> {
        let cgroup_path = Self::read_cgroup_v2_path()?;
        let full = PathBuf::from("/sys/fs/cgroup").join(&cgroup_path);
        let child_name = format!("rebuild-{build_id}");
        let limit_bytes = memory_limit_mb * 1024 * 1024;

        debug!("Searching for writable cgroup parent, starting from {}", full.display());

        let mut current = Some(full.as_path());
        while let Some(dir) = current {
            debug!("Trying cgroup parent: {}", dir.display());

            // Check if `memory` is available at this level.
            let controllers = fs::read_to_string(dir.join("cgroup.controllers"))
                .unwrap_or_default();
            debug!("  cgroup.controllers: {:?}", controllers.trim());

            if !controllers.split_whitespace().any(|c| c == "memory") {
                debug!("  no memory controller, skipping to parent");
                current = dir.parent();
                continue;
            }

            // Try to enable memory for children (no-op if already enabled).
            let subtree_ctl = dir.join("cgroup.subtree_control");
            match Self::cgroup_write(&subtree_ctl, "+memory") {
                Ok(()) => debug!("  subtree_control +memory: OK"),
                Err(e) => debug!("  subtree_control +memory: failed ({e})"),
            }

            // Try to create the child cgroup.
            let child_path = dir.join(&child_name);
            match fs::create_dir(&child_path) {
                Ok(()) => debug!("  mkdir child: OK at {}", child_path.display()),
                Err(e) => {
                    debug!("  mkdir child: failed ({e}), skipping to parent");
                    current = dir.parent();
                    continue;
                }
            }

            // Try to set the memory limit.
            match Self::cgroup_write(&child_path.join("memory.max"), &limit_bytes.to_string()) {
                Ok(()) => {
                    debug!("  memory.max write: OK — cgroup ready at {}", child_path.display());
                    return Ok(child_path);
                }
                Err(e) => {
                    debug!("  memory.max write: failed ({e}), cleaning up and trying parent");
                    let _ = fs::remove_dir(&child_path);
                    current = dir.parent();
                    continue;
                }
            }
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
        let path = Self::find_and_create_child(build_id, memory_limit_mb)?;
        Ok(BuildCgroup { path })
    }

    /// Move a process (by PID) into this cgroup by writing to cgroup.procs.
    pub fn add_process(&self, pid: u32) -> Result<()> {
        Self::cgroup_write(&self.path.join("cgroup.procs"), &pid.to_string())
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
