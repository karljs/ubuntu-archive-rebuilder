//! OOM detection for the systemd-run scope each build runs in
//! (see builder::sbuild).

use anyhow::{Context, Result};
use std::fs;
use std::path::PathBuf;
use tracing::debug;

pub fn parse_oom_kill_count(events: &str) -> u64 {
    for line in events.lines() {
        if let Some(rest) = line.strip_prefix("oom_kill ") {
            return rest.trim().parse().unwrap_or(0);
        }
    }
    0
}

/// Must be read after the build's pipes close but BEFORE child.wait():
/// systemd-run is then still alive collecting the exit code, so the scope
/// cgroup still exists. Once reaped, the cgroup is gone and the read fails.
pub struct SystemdScopeCgroup {
    cgroup_path: PathBuf,
}

impl SystemdScopeCgroup {
    pub fn from_scope_name(scope_name: &str) -> Result<Self> {
        let uid = nix::unistd::getuid().as_raw();
        let cgroup_path = PathBuf::from("/sys/fs/cgroup")
            .join(format!("user.slice/user-{uid}.slice/user@{uid}.service"))
            .join(scope_name);

        if !cgroup_path.exists() {
            anyhow::bail!(
                "Scope cgroup not found at {} (scope may have been cleaned up already)",
                cgroup_path.display()
            );
        }

        debug!("Systemd scope cgroup: {}", cgroup_path.display());
        Ok(Self { cgroup_path })
    }

    /// Ok(false) when the cgroup is already gone.
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
