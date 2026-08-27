//! OOM detection for builds run under a memory-limited systemd scope.
//!
//! The sbuild runner wraps each build in `systemd-run --user --scope
//! --property=MemoryMax=<bytes>` (see builder::sbuild).  systemd creates the
//! scope cgroup under user@UID.service with the memory limit already set, so
//! this module only needs to read the scope's `memory.events` after the
//! build to detect cgroup-local OOM kills.

use anyhow::{Context, Result};
use std::fs;
use std::path::PathBuf;
use tracing::debug;

/// Extract the `oom_kill` count from `memory.events` content.
/// Used by `SystemdScopeCgroup::read_oom_kill()` but extracted for unit testing.
pub fn parse_oom_kill_count(events: &str) -> u64 {
    for line in events.lines() {
        if let Some(rest) = line.strip_prefix("oom_kill ") {
            return rest.trim().parse().unwrap_or(0);
        }
    }
    0
}

/// OOM detection for a systemd transient scope.
///
/// When sbuild is wrapped in `systemd-run --user --scope --unit=<name>
/// --property=MemoryMax=<bytes>`, systemd creates a cgroup under
/// `user@UID.service/<name>` with the memory limit already set.
///
/// After the build finishes (pipes closed) but before `child.wait()` returns,
/// the scope's cgroup still exists because `systemd-run` is still alive
/// collecting the exit code.  `read_oom_kill()` reads `memory.events` in
/// that window.  If the scope has already been cleaned up (race lost), it
/// returns `Ok(false)` — graceful degradation.
pub struct SystemdScopeCgroup {
    cgroup_path: PathBuf,
}

impl SystemdScopeCgroup {
    /// Construct from a scope unit name (e.g. `"rebuild-<uuid>.scope"`).
    /// The cgroup path is derived from the UID and scope name.
    pub fn from_scope_name(scope_name: &str) -> Result<Self> {
        let uid = nix::unistd::getuid().as_raw();
        let cgroup_path = PathBuf::from("/sys/fs/cgroup")
            .join(format!("user.slice/user-{uid}.slice/user@{uid}.service"))
            .join(scope_name);

        if !cgroup_path.exists() {
            anyhow::bail!(
                "Scope cgroup not found at {} — scope may have been cleaned up already",
                cgroup_path.display()
            );
        }

        debug!("Systemd scope cgroup: {}", cgroup_path.display());
        Ok(Self { cgroup_path })
    }

    /// Read `memory.events` and return `true` if `oom_kill > 0`.
    /// Returns `Ok(false)` if the cgroup is gone (race lost).
    pub fn read_oom_kill(&self) -> Result<bool> {
        let events =
            fs::read_to_string(self.cgroup_path.join("memory.events")).with_context(|| {
                format!(
                    "Failed to read memory.events at {} (scope may be cleaned up)",
                    self.cgroup_path.display()
                )
            })?;
        Ok(parse_oom_kill_count(&events) > 0)
    }
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
