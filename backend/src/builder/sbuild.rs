//! sbuild invocation and process management. Each build runs under
//! `/usr/bin/time -v` in its own process group (setpgid/killpg) so timeouts
//! and Ctrl+C kill the whole tree. Clang substitution is injected via sbuild
//! hooks: chroot-setup configures the apt proxy (all profiles) and installs
//! clang (clang profiles); starting-build wraps gcc to clang.

use std::io::Write;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use anyhow::{Context, Result};
use nix::sys::signal::{killpg, Signal};
use nix::unistd::Pid;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, trace, warn};

use crate::analyzer::infer_status;
use crate::builder::cgroup::SystemdScopeCgroup;
use crate::builder::time_parser::parse_time_output;
use crate::models::BuildStatus;
use crate::profile::CompilerType;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChrootMode {
    Unshare,
    Schroot,
}

impl ChrootMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Unshare => "unshare",
            Self::Schroot => "schroot",
        }
    }
}

impl std::str::FromStr for ChrootMode {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s {
            "unshare" => Ok(Self::Unshare),
            "schroot" => Ok(Self::Schroot),
            other => Err(format!(
                "unknown chroot mode: {other} (expected: unshare, schroot)"
            )),
        }
    }
}

impl std::fmt::Display for ChrootMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

// Shellcheck-linted script templates; placeholders substituted at runtime.
const CHROOT_SETUP_SCRIPT: &str = include_str!("scripts/chroot_setup.sh");
const STARTING_BUILD_SCRIPT: &str = include_str!("scripts/starting_build.sh");
const GCC_VERIFY_SCRIPT: &str = include_str!("scripts/gcc_verify.sh");
const SBUILD_CONFIG_TEMPLATE: &str = include_str!("scripts/sbuild_config.pl.tmpl");

pub struct SbuildConfig {
    pub dsc_path: PathBuf,
    pub series: String,
    pub arch: String,
    pub compiler_type: CompilerType,
    pub compiler_version: String,
    pub build_env: Vec<(String, String)>,
    pub timeout_seconds: u64,
    pub verbose: bool,
    pub run_tests: bool,
    pub jobs: usize,
    pub cancel_token: CancellationToken,
    pub memory_limit_mb: u64,
    pub chroot_mode: ChrootMode,
}

pub struct SbuildResult {
    pub status: BuildStatus,
    pub log: String,
    pub duration_seconds: Option<f64>,
    pub peak_memory_mb: Option<i64>,
    pub compiler_detected: Option<String>,
    pub memory_limit_mb: Option<u64>,
}

/// Timeout/cancellation in Rust, not timeout(1): process-hierarchy issues
/// with timeout(1) orphaned chroot processes in earlier iterations.
pub async fn run_sbuild(config: &SbuildConfig) -> Result<SbuildResult> {
    let build_id = Uuid::new_v4();
    let (mut cmd, _config_file, metrics_file, scope_name) = build_command(config, build_id)?;

    debug!("Spawning: {:?}", cmd);

    let mut child = cmd.spawn().context("Failed to spawn sbuild")?;
    let child_pid = child.id().context("Failed to get child PID")?;
    let pgid = Pid::from_raw(child_pid as i32);

    let stdout = child.stdout.take().context("Failed to capture stdout")?;
    let stderr = child.stderr.take().context("Failed to capture stderr")?;
    let mut stdout_lines = BufReader::new(stdout).lines();
    let mut stderr_lines = BufReader::new(stderr).lines();

    let mut log_lines: Vec<String> = Vec::new();
    let mut stdout_done = false;
    let mut stderr_done = false;
    let mut timed_out = false;

    let timeout = Duration::from_secs(config.timeout_seconds);

    let read_result = tokio::time::timeout(timeout, async {
        loop {
            if stdout_done && stderr_done {
                break;
            }
            tokio::select! {
                _ = config.cancel_token.cancelled() => {
                    anyhow::bail!("Interrupted by user");
                }
                line = stdout_lines.next_line(), if !stdout_done => {
                    match line {
                        Ok(Some(line)) => {
                            if config.verbose { println!("{line}"); }
                            trace!("{line}");
                            log_lines.push(line);
                        }
                        Ok(None) => stdout_done = true,
                        Err(e) => { debug!("stdout read error: {e}"); stdout_done = true; }
                    }
                }
                line = stderr_lines.next_line(), if !stderr_done => {
                    match line {
                        Ok(Some(line)) => {
                            if config.verbose { eprintln!("{line}"); }
                            trace!(stderr = true, "{line}");
                            log_lines.push(line);
                        }
                        Ok(None) => stderr_done = true,
                        Err(e) => { debug!("stderr read error: {e}"); stderr_done = true; }
                    }
                }
            }
        }
        Ok::<(), anyhow::Error>(())
    })
    .await;

    match read_result {
        Ok(Ok(())) => { /* pipes closed normally */ }
        Ok(Err(e)) => {
            info!("Killing process group (pgid={pgid}) due to: {e}");
            kill_process_group(pgid).await;
            drain_pipes(&mut stdout_lines, &mut stderr_lines, &mut log_lines).await;
            let _ = child.wait().await;
            return Err(e);
        }
        Err(_elapsed) => {
            timed_out = true;
            info!(
                "Build timed out after {}s, killing process group (pgid={pgid})",
                config.timeout_seconds
            );
            kill_process_group(pgid).await;
            drain_pipes(&mut stdout_lines, &mut stderr_lines, &mut log_lines).await;
        }
    }

    // Must be read before child.wait(); see SystemdScopeCgroup.
    let scope_cgroup =
        scope_name
            .as_deref()
            .and_then(|name| match SystemdScopeCgroup::from_scope_name(name) {
                Ok(cg) => Some(cg),
                Err(e) => {
                    debug!("Could not access scope cgroup for OOM detection: {e}");
                    None
                }
            });

    let exit_status = child.wait().await.context("Failed to wait for sbuild")?;
    let log = log_lines.join("\n");
    // time wrote its banner and metrics to the file, not stderr; unknown
    // lines in it (the banner) are ignored by the parser.
    let time_output = std::fs::read_to_string(metrics_file.path()).with_context(|| {
        format!(
            "Failed to read time metrics {}",
            metrics_file.path().display()
        )
    })?;
    let metrics = parse_time_output(&time_output);
    let compiler_detected = detect_compiler_from_log(&log, config.compiler_type);

    let exit_code = metrics.exit_status.or_else(|| exit_status.code());
    let status = if timed_out {
        BuildStatus::Timeout
    } else {
        infer_status(&log, exit_code)
    };

    let (final_status, final_memory_limit_mb) = if let Some(cg) = scope_cgroup {
        let oom_killed = cg.read_oom_kill().unwrap_or(false);
        let limit = Some(config.memory_limit_mb);
        let status = if oom_killed {
            BuildStatus::OomKilled
        } else {
            status
        };
        (status, limit)
    } else if scope_name.is_some() {
        // Scope reaped before memory.events could be read: detection lost,
        // enforcement not.
        (status, Some(config.memory_limit_mb))
    } else {
        (status, None)
    };

    Ok(SbuildResult {
        status: final_status,
        log,
        duration_seconds: metrics.wall_time_seconds,
        peak_memory_mb: metrics.peak_memory_kb.map(|kb| kb / 1024),
        compiler_detected: Some(compiler_detected),
        memory_limit_mb: final_memory_limit_mb,
    })
}

// The unshare chroot doesn't inherit http_proxy/https_proxy from the
// environment; REBUILD_HTTP_PROXY forwards one into apt's config.
fn http_proxy_for_chroot() -> String {
    std::env::var("REBUILD_HTTP_PROXY").unwrap_or_default()
}

// memory_limit_mb > 0 wraps the command in systemd-run --user --scope
// --property=MemoryMax: the scope lives under user@UID.service (Delegate=yes),
// so memory limiting works without cgroup delegation on our own process.
fn build_command(
    config: &SbuildConfig,
    build_id: Uuid,
) -> Result<(
    Command,
    tempfile::NamedTempFile,
    tempfile::NamedTempFile,
    Option<String>,
)> {
    let dsc_dir = config.dsc_path.parent().context("Invalid .dsc path")?;

    let sbuild_config_file = generate_sbuild_config(
        config.jobs,
        config.run_tests,
        &config.build_env,
        config.chroot_mode,
        &config.series,
        &config.arch,
    )?;

    // time's banner ("Command being timed: <full multi-line command>")
    // must never reach the captured stderr; -o routes it here instead.
    let metrics_file = tempfile::Builder::new()
        .prefix("rebuild-time-")
        .suffix(".txt")
        .tempfile()
        .context("Failed to create time metrics tempfile")?;
    let time_args = ["-v", "-o"];

    let mut sbuild_args: Vec<String> = vec![
        "--verbose".into(),
        "--batch".into(),
        format!("--dist={}", config.series),
        format!("--arch={}", config.arch),
    ];

    let mut tmpdir: Option<PathBuf> = None;

    match config.chroot_mode {
        ChrootMode::Unshare => {
            // unshare extracts chroots into $TMPDIR (default /tmp, tmpfs).
            let scratch_dir = PathBuf::from("/var/tmp/rebuild-builds");
            std::fs::create_dir_all(&scratch_dir)
                .context("Failed to create /var/tmp/rebuild-builds")?;
            sbuild_args.push("--chroot-mode=unshare".into());
            sbuild_args.push("--purge=always".into());
            tmpdir = Some(scratch_dir);
        }
        ChrootMode::Schroot => {
            // schroot is the default; purge lives in the generated config so
            // $purge_build_deps = 'never' isn't overridden.
        }
    }

    // Every profile gets the apt proxy config in the chroot; clang
    // profiles additionally install the target version there.
    let proxy = http_proxy_for_chroot();
    let clang_version = match config.compiler_type {
        CompilerType::Clang => config.compiler_version.as_str(),
        CompilerType::Gcc => "",
    };
    let setup_cmd = wrap_in_heredoc(
        "chroot-setup.sh",
        "REBUILD_CHROOT_SETUP_EOF",
        &CHROOT_SETUP_SCRIPT
            .replace("__CLANG_VERSION__", clang_version)
            .replace("__HTTP_PROXY__", &proxy),
    );
    sbuild_args.push(format!("--chroot-setup-commands={setup_cmd}"));

    match config.compiler_type {
        CompilerType::Clang => {
            let starting_cmd = wrap_in_heredoc(
                "clang-wrapper-setup.sh",
                "CLANG_WRAPPER_EOF",
                &STARTING_BUILD_SCRIPT.replace("__CLANG_VERSION__", &config.compiler_version),
            );
            sbuild_args.push(format!("--starting-build-commands={starting_cmd}"));
        }
        CompilerType::Gcc => {
            let starting_cmd =
                wrap_in_heredoc("gcc-verify.sh", "GCC_VERIFY_EOF", GCC_VERIFY_SCRIPT);
            sbuild_args.push(format!("--starting-build-commands={starting_cmd}"));
        }
    }

    sbuild_args.push("--no-clean-source".into());
    sbuild_args.push(config.dsc_path.to_string_lossy().into_owned());

    let scope_name = if config.memory_limit_mb > 0 {
        Some(format!("rebuild-{build_id}.scope"))
    } else {
        None
    };

    let mut cmd = if config.memory_limit_mb > 0 {
        let limit_bytes = config.memory_limit_mb * 1024 * 1024;
        let unit = scope_name.as_ref().unwrap();
        let mut cmd = Command::new("systemd-run");
        cmd.arg("--user")
            .arg("--scope")
            .arg("--quiet")
            .arg(format!("--unit={unit}"))
            .arg(format!("--property=MemoryMax={limit_bytes}"))
            .arg("--")
            .arg("/usr/bin/time")
            .args(time_args)
            .arg(metrics_file.path())
            .arg("sbuild");
        for a in &sbuild_args {
            cmd.arg(a);
        }
        cmd
    } else {
        let mut cmd = Command::new("/usr/bin/time");
        cmd.args(time_args).arg(metrics_file.path()).arg("sbuild");
        for a in &sbuild_args {
            cmd.arg(a);
        }
        cmd
    };

    cmd.current_dir(dsc_dir)
        .env("SBUILD_CONFIG", sbuild_config_file.path())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    if let Some(td) = tmpdir {
        cmd.env("TMPDIR", td);
    }

    // Own process group so killpg reaches the whole tree.
    // SAFETY: setpgid is async-signal-safe (POSIX.1-2017 §2.4.3), the only
    // requirement for pre_exec.
    unsafe {
        cmd.pre_exec(|| {
            nix::unistd::setpgid(Pid::from_raw(0), Pid::from_raw(0))
                .map_err(std::io::Error::other)?;
            Ok(())
        });
    }

    Ok((cmd, sbuild_config_file, metrics_file, scope_name))
}

// sbuild external commands receive multi-line scripts via this heredoc form.
fn wrap_in_heredoc(filename: &str, delimiter: &str, body: &str) -> String {
    format!(
        "cat > /tmp/{filename} << '{delimiter}'\n\
         {body}\n\
         {delimiter}\n\
         chmod +x /tmp/{filename} && /tmp/{filename}"
    )
}

// SBUILD_CONFIG is loaded after ~/.sbuildrc, so it wins. Purge: unshare
// purges always (ephemeral), schroot never (deps accumulate).
fn generate_sbuild_config(
    jobs: usize,
    run_tests: bool,
    build_env: &[(String, String)],
    chroot_mode: ChrootMode,
    series: &str,
    arch: &str,
) -> Result<tempfile::NamedTempFile> {
    let nocheck = if run_tests { "" } else { " nocheck" };

    let mut env_entries = vec![format!(
        "'DEB_BUILD_OPTIONS' => 'parallel={jobs}{nocheck}',"
    )];
    for (var, value) in build_env {
        // Perl single-quote escaping.
        let escaped = value.replace('\'', "'\\''");
        env_entries.push(format!("'{var}' => '{escaped}',"));
    }
    let env_block = env_entries.join("\n    ");

    let purge_build_deps = match chroot_mode {
        ChrootMode::Unshare => "always",
        ChrootMode::Schroot => "never",
    };

    let config = SBUILD_CONFIG_TEMPLATE
        .replace("__ENV_BLOCK__", &env_block)
        .replace("__PURGE_BUILD_DEPS__", purge_build_deps)
        .replace("__DIST__", series)
        // http, not https: the proxy may MITM TLS (internal CA) and buildd
        // chroots ship without ca-certificates, so https apt sources fail
        // inside the chroot.
        .replace(
            "__MIRROR__",
            &crate::fetcher::default_mirror_for_arch(arch).replace("https://", "http://"),
        );

    let mut file = tempfile::Builder::new()
        .prefix("rebuild-sbuild-")
        .suffix(".conf")
        .tempfile()
        .context("Failed to create temporary sbuild config")?;

    file.write_all(config.as_bytes())
        .context("Failed to write sbuild config")?;

    debug!("Generated sbuild config at {:?}", file.path());
    Ok(file)
}

async fn kill_process_group(pgid: Pid) {
    if let Err(e) = killpg(pgid, Signal::SIGTERM) {
        warn!("Failed to SIGTERM process group {pgid}: {e}");
        return;
    }
    tokio::time::sleep(Duration::from_secs(10)).await;
    if killpg(pgid, Signal::SIGKILL).is_ok() {
        debug!("Sent SIGKILL to process group {pgid}");
    }
}

// The final lines of a killed build are the most diagnostic; keep them.
async fn drain_pipes(
    stdout: &mut tokio::io::Lines<BufReader<tokio::process::ChildStdout>>,
    stderr: &mut tokio::io::Lines<BufReader<tokio::process::ChildStderr>>,
    log_lines: &mut Vec<String>,
) {
    let _ = tokio::time::timeout(Duration::from_secs(5), async {
        let mut stdout_done = false;
        let mut stderr_done = false;
        loop {
            if stdout_done && stderr_done {
                break;
            }
            tokio::select! {
                r = stdout.next_line(), if !stdout_done => {
                    match r {
                        Ok(Some(line)) => log_lines.push(line),
                        _ => stdout_done = true,
                    }
                }
                r = stderr.next_line(), if !stderr_done => {
                    match r {
                        Ok(Some(line)) => log_lines.push(line),
                        _ => stderr_done = true,
                    }
                }
            }
        }
    })
    .await;
}

// sbuild echoes the full script source before executing it, so markers
// inside `echo "..."` lines are skipped: only real output counts. If
// verification never ran, a chroot_setup REBUILD-ERROR (typically an
// apt/proxy failure) is surfaced instead of "UNKNOWN".
fn detect_compiler_from_log(log: &str, compiler_type: CompilerType) -> String {
    let mut success = false;
    let mut failed = false;
    let mut version_line: Option<&str> = None;
    let mut chroot_setup_error: Option<&str> = None;

    for line in log.lines() {
        let trimmed = line.trim();

        if trimmed.starts_with("echo ") || trimmed.starts_with('"') || trimmed.starts_with('\'') {
            continue;
        }

        // "FAILED -" is verification failure, handled below; anything else
        // is a chroot-setup error.
        if chroot_setup_error.is_none()
            && trimmed.starts_with("REBUILD-ERROR:")
            && !trimmed.starts_with("REBUILD-ERROR: FAILED -")
        {
            chroot_setup_error = Some(trimmed);
        }

        match compiler_type {
            CompilerType::Clang => {
                if trimmed == "REBUILD: SUCCESS - gcc is now clang" {
                    success = true;
                }
                if trimmed.starts_with("REBUILD-ERROR: FAILED -") {
                    failed = true;
                }
                if trimmed.starts_with("REBUILD:   gcc --version:") && trimmed.contains("clang") {
                    version_line = Some(trimmed);
                }
            }
            CompilerType::Gcc => {
                if trimmed == "REBUILD: SUCCESS - gcc confirmed" {
                    success = true;
                }
                if trimmed.starts_with("REBUILD-ERROR: FAILED -") {
                    failed = true;
                }
                if trimmed.starts_with("REBUILD:   gcc --version:") && trimmed.contains("gcc") {
                    version_line = Some(trimmed);
                }
            }
        }
    }

    if failed && !success {
        return match compiler_type {
            CompilerType::Clang => "ERROR: gcc wrapper setup FAILED - built with real GCC".into(),
            CompilerType::Gcc => {
                "ERROR: gcc verification FAILED - gcc --version did not report gcc".into()
            }
        };
    }

    if success {
        if let Some(vline) = version_line {
            let version = vline
                .split("gcc --version:")
                .nth(1)
                .map(str::trim)
                .unwrap_or("version unknown");
            let label = compiler_type.as_str();
            return format!("{label} confirmed: {version}");
        }
        let label = compiler_type.as_str();
        return format!("{label} confirmed");
    }

    if let Some(err) = chroot_setup_error {
        return format!("ERROR: chroot setup failed - {err}");
    }

    "UNKNOWN: no compiler verification markers found in log".into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chroot_setup_substitutes_version() {
        let script = CHROOT_SETUP_SCRIPT
            .replace("__CLANG_VERSION__", "19")
            .replace("__HTTP_PROXY__", "");
        assert!(script.contains(r#"CLANG_VERSION="19""#));
        assert!(!script.contains("__CLANG_VERSION__"));
        assert!(!script.contains("__HTTP_PROXY__"));
        // Both guards exist; with empty proxy the proxy branch is dead.
        assert!(script.contains("if [ -n \"\" ]; then"));
    }

    // Regression: gcc profiles must get a chroot-setup script too (apt
    // proxy); an empty version leaves the clang-install branch dead instead
    // of running apt-get with an empty package name.
    #[test]
    fn chroot_setup_empty_version_skips_clang_install() {
        let script = CHROOT_SETUP_SCRIPT
            .replace("__CLANG_VERSION__", "")
            .replace("__HTTP_PROXY__", "http://proxy.example:3128");
        assert!(script.contains(r#"CLANG_VERSION="""#));
        // The clang install stays inside the dead branch.
        let guard = script.find("if [ -n \"\" ]; then").unwrap();
        let install = script.find("apt-get install").unwrap();
        assert!(guard < install);
    }

    #[test]
    fn chroot_setup_substitutes_proxy() {
        let script = CHROOT_SETUP_SCRIPT
            .replace("__CLANG_VERSION__", "19")
            .replace("__HTTP_PROXY__", "http://proxy.example:3128");
        assert!(script.contains("Acquire::http::Proxy"));
        assert!(script.contains("Acquire::https::Proxy"));
        assert!(script.contains("http://proxy.example:3128"));
        assert!(script.contains("if [ -n \"http://proxy.example:3128\" ]; then"));
        assert!(!script.contains("__HTTP_PROXY__"));
        assert!(!script.contains("__CLANG_VERSION__"));
    }

    #[test]
    fn starting_build_substitutes_version() {
        let script = STARTING_BUILD_SCRIPT.replace("__CLANG_VERSION__", "19");
        assert!(script.contains(r#"CLANG_VERSION="19""#));
        assert!(!script.contains("__CLANG_VERSION__"));
    }

    #[test]
    fn starting_build_contains_verification_markers() {
        assert!(STARTING_BUILD_SCRIPT.contains("REBUILD: SUCCESS"));
        assert!(STARTING_BUILD_SCRIPT.contains("REBUILD-ERROR: FAILED"));
    }

    #[test]
    fn starting_build_wraps_versioned_compilers_dynamically() {
        assert!(STARTING_BUILD_SCRIPT.contains("gcc-[0-9]*"));
        assert!(STARTING_BUILD_SCRIPT.contains("g++-[0-9]*"));
        assert!(STARTING_BUILD_SCRIPT.contains("-gcc-[0-9]*"));
        assert!(!STARTING_BUILD_SCRIPT.contains("for v in 9 10 11 12 13 14"));
    }

    #[test]
    fn starting_build_verifies_every_replaced_compiler() {
        assert!(STARTING_BUILD_SCRIPT.contains("for name in $(all_names)"));
        assert!(STARTING_BUILD_SCRIPT.contains("compiler verification failed"));
    }

    #[test]
    fn starting_build_avoids_sbuild_command_mangling() {
        // Regression: sbuild flattens array references to empty strings and
        // expands bare percent sequences; the script must use neither.
        assert!(
            !STARTING_BUILD_SCRIPT.contains("[@]"),
            "array references do not survive sbuild command processing"
        );
        assert!(
            !STARTING_BUILD_SCRIPT.contains("%%s\n\"\n"),
            "unexpected doubled-percent literal"
        );
    }

    #[test]
    fn starting_build_no_placeholders_remain() {
        let script = STARTING_BUILD_SCRIPT.replace("__CLANG_VERSION__", "18");
        assert!(!script.contains("__CLANG_VERSION__"));
    }

    #[test]
    fn gcc_verify_script_contains_markers() {
        assert!(GCC_VERIFY_SCRIPT.contains("REBUILD: SUCCESS - gcc confirmed"));
    }

    #[test]
    fn gcc_verify_script_fails_honestly() {
        assert!(GCC_VERIFY_SCRIPT.contains("REBUILD-ERROR: FAILED"));
        assert!(GCC_VERIFY_SCRIPT.contains("exit 1"));
        assert_eq!(
            GCC_VERIFY_SCRIPT.matches("REBUILD: SUCCESS").count(),
            1,
            "SUCCESS marker must appear exactly once (no unconditional fallback)"
        );
    }

    #[test]
    fn detects_gcc_verification_failure() {
        let log = "REBUILD:   gcc --version: Ubuntu clang version 18.1.3\n\
                    REBUILD-ERROR: FAILED - gcc is not reporting as gcc: Ubuntu clang version 18.1.3\n";
        let result = detect_compiler_from_log(log, CompilerType::Gcc);
        assert!(
            result.contains("ERROR: gcc verification FAILED"),
            "got: {result}"
        );
        assert!(!result.contains("chroot setup failed"), "got: {result}");
    }

    #[test]
    fn detects_gcc_missing_from_chroot() {
        let log = "REBUILD-ERROR: FAILED - gcc not found in chroot\n";
        let result = detect_compiler_from_log(log, CompilerType::Gcc);
        assert!(
            result.contains("ERROR: gcc verification FAILED"),
            "got: {result}"
        );
    }

    #[test]
    fn heredoc_wraps_script() {
        let cmd = wrap_in_heredoc("test.sh", "EOF", "echo hello");
        assert!(cmd.starts_with("cat > /tmp/test.sh << 'EOF'"));
        assert!(cmd.contains("echo hello"));
        assert!(cmd.ends_with("chmod +x /tmp/test.sh && /tmp/test.sh"));
    }

    #[test]
    fn detects_clang_confirmed() {
        let log = "REBUILD:   gcc --version: Ubuntu clang version 18.1.3\n\
                   REBUILD: SUCCESS - gcc is now clang\n";
        let result = detect_compiler_from_log(log, CompilerType::Clang);
        assert!(result.starts_with("clang confirmed"), "got: {result}");
        assert!(result.contains("18.1.3"), "got: {result}");
    }

    #[test]
    fn detects_gcc_confirmed() {
        let log = "REBUILD:   gcc --version: gcc (Ubuntu 13.3.0-6ubuntu2) 13.3.0\n\
                   REBUILD: SUCCESS - gcc confirmed\n";
        let result = detect_compiler_from_log(log, CompilerType::Gcc);
        assert!(result.starts_with("gcc confirmed"), "got: {result}");
        assert!(result.contains("13.3.0"), "got: {result}");
    }

    #[test]
    fn detects_wrapper_failure() {
        let log = "REBUILD-ERROR: FAILED - gcc is NOT reporting as clang!\n";
        assert!(detect_compiler_from_log(log, CompilerType::Clang).contains("ERROR"));
    }

    #[test]
    fn detects_missing_markers() {
        assert!(
            detect_compiler_from_log("some build output\n", CompilerType::Clang)
                .contains("UNKNOWN")
        );
    }

    #[test]
    fn ignores_echoed_script_source() {
        let log = concat!(
            "    echo \"REBUILD: SUCCESS - gcc is now clang\"\n",
            "    echo \"REBUILD-ERROR: FAILED - gcc is NOT reporting as clang!\" >&2\n",
            "REBUILD: SUCCESS - gcc is now clang\n",
        );
        let result = detect_compiler_from_log(log, CompilerType::Clang);
        assert!(result.contains("clang confirmed"), "got: {result}");
    }

    #[test]
    fn real_failure_not_masked_by_echoed_success() {
        let log = concat!(
            "    echo \"REBUILD: SUCCESS - gcc is now clang\"\n",
            "    echo \"REBUILD-ERROR: FAILED - gcc is NOT reporting as clang!\" >&2\n",
            "REBUILD-ERROR: FAILED - gcc is NOT reporting as clang!\n",
        );
        let result = detect_compiler_from_log(log, CompilerType::Clang);
        assert!(result.contains("ERROR"), "got: {result}");
    }

    #[test]
    fn chroot_setup_apt_failure_is_surfaced() {
        let log = "=== REBUILD: Installing Clang 18 ===\n\
                    REBUILD-ERROR: Failed to install clang-18 (check proxy / archive reachability)\n";
        let result = detect_compiler_from_log(log, CompilerType::Clang);
        assert!(
            result.starts_with("ERROR: chroot setup failed"),
            "got: {result}"
        );
        assert!(
            result.contains("Failed to install clang-18"),
            "got: {result}"
        );
    }

    #[test]
    fn chroot_setup_error_does_not_override_wrapper_failure() {
        let log = concat!(
            "REBUILD-ERROR: Failed to install clang-18 (check proxy / archive reachability)\n",
            "REBUILD-ERROR: FAILED - gcc is NOT reporting as clang!\n",
        );
        let result = detect_compiler_from_log(log, CompilerType::Clang);
        assert!(result.contains("wrapper setup FAILED"), "got: {result}");
    }

    #[test]
    fn chroot_setup_error_marker_not_confused_with_wrapper_error() {
        let log = "REBUILD-ERROR: FAILED - gcc is NOT reporting as clang!\n";
        let result = detect_compiler_from_log(log, CompilerType::Clang);
        assert!(result.contains("wrapper setup FAILED"), "got: {result}");
    }

    #[test]
    fn generic_verification_failure_marker_detected() {
        let log = concat!(
            "REBUILD-ERROR: FAILED - gcc-15 is NOT reporting as clang!\n",
            "REBUILD-ERROR: FAILED - compiler verification failed: gcc-15\n",
            "REBUILD-ERROR: Build would use GCC, not Clang. Aborting.\n",
        );
        let result = detect_compiler_from_log(log, CompilerType::Clang);
        assert!(result.contains("wrapper setup FAILED"), "got: {result}");
        assert!(!result.contains("chroot setup failed"), "got: {result}");
    }

    #[test]
    fn chroot_mode_parse_unshare() {
        let mode: ChrootMode = "unshare".parse().unwrap();
        assert_eq!(mode, ChrootMode::Unshare);
        assert_eq!(mode.as_str(), "unshare");
        assert_eq!(format!("{mode}"), "unshare");
    }

    #[test]
    fn chroot_mode_parse_schroot() {
        let mode: ChrootMode = "schroot".parse().unwrap();
        assert_eq!(mode, ChrootMode::Schroot);
        assert_eq!(mode.as_str(), "schroot");
        assert_eq!(format!("{mode}"), "schroot");
    }

    #[test]
    fn chroot_mode_parse_invalid() {
        assert!("docker".parse::<ChrootMode>().is_err());
    }

    #[test]
    fn sbuild_config_unshare_purges_deps_always() {
        let file =
            generate_sbuild_config(4, false, &[], ChrootMode::Unshare, "noble", "amd64").unwrap();
        let config = std::fs::read_to_string(file.path()).unwrap();
        assert!(
            config.contains("$purge_build_deps = 'always';"),
            "unshare mode must purge deps always, got:\n{config}"
        );
        assert!(!config.contains("__PURGE_BUILD_DEPS__"));
    }

    #[test]
    fn sbuild_config_schroot_purges_deps_never() {
        let file =
            generate_sbuild_config(4, false, &[], ChrootMode::Schroot, "noble", "amd64").unwrap();
        let config = std::fs::read_to_string(file.path()).unwrap();
        assert!(
            config.contains("$purge_build_deps = 'never';"),
            "schroot mode must purge deps never, got:\n{config}"
        );
        assert!(!config.contains("__PURGE_BUILD_DEPS__"));
    }

    #[test]
    fn sbuild_config_no_placeholders_remain() {
        for mode in [ChrootMode::Unshare, ChrootMode::Schroot] {
            let file = generate_sbuild_config(4, true, &[], mode, "noble", "amd64").unwrap();
            let config = std::fs::read_to_string(file.path()).unwrap();
            assert!(
                !config.contains("__ENV_BLOCK__"),
                "mode {mode:?}: __ENV_BLOCK__ remains"
            );
            assert!(
                !config.contains("__PURGE_BUILD_DEPS__"),
                "mode {mode:?}: __PURGE_BUILD_DEPS__ remains"
            );
        }
    }

    // Regression: the chroot mirror must be http. https fails inside the
    // chroot (proxy MITM with an internal CA + no ca-certificates in the
    // builld variant).
    #[test]
    fn sbuild_config_chroot_mirror_is_http() {
        for (series, arch) in [("noble", "amd64"), ("noble", "arm64")] {
            let file =
                generate_sbuild_config(4, false, &[], ChrootMode::Unshare, series, arch).unwrap();
            let config = std::fs::read_to_string(file.path()).unwrap();
            assert!(
                config.contains("http://"),
                "{series}/{arch}: no http mirror in config:\n{config}"
            );
            assert!(
                !config.contains("https://"),
                "{series}/{arch}: https mirror leaked into chroot config:\n{config}"
            );
        }
    }
}
