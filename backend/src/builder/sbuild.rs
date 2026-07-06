//! sbuild invocation, process management, and build log analysis.
//!
//! Wraps each build with `/usr/bin/time -v` for resource metrics and handles
//! timeout / Ctrl+C cancellation via process-group isolation (`setpgid` /
//! `killpg`) so the entire sbuild process tree is cleaned up reliably.
//!
//! For **clang** profiles, compiler substitution happens in two phases
//! injected into sbuild's external-command hooks:
//!
//! 1. **chroot-setup-commands** (before dep installation) — installs the
//!    target clang version.
//! 2. **starting-build-commands** (after dep installation, before
//!    dpkg-buildpackage) — diverts gcc/g++/cc/c++ to clang wrappers and
//!    verifies the substitution succeeded.
//!
//! For **gcc** profiles, the chroot setup is skipped (gcc is already
//! present via build-deps) and a lightweight verification script records
//! the gcc version.
//!
//! Profile flags are injected into the starting-build script as
//! `DEB_*_APPEND` exports.

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

use crate::builder::time_parser::parse_time_output;
use crate::analyzer::infer_status;
use crate::models::BuildStatus;
use crate::profile::CompilerType;

// ---------------------------------------------------------------------------
// Shell script templates — loaded at compile time from external files so they
// can be linted with shellcheck independently.  Placeholders are substituted
// at runtime.
// ---------------------------------------------------------------------------

const CHROOT_SETUP_SCRIPT: &str = include_str!("scripts/chroot_setup.sh");
const STARTING_BUILD_SCRIPT: &str = include_str!("scripts/starting_build.sh");
const GCC_VERIFY_SCRIPT: &str = include_str!("scripts/gcc_verify.sh");
const SBUILD_CONFIG_TEMPLATE: &str = include_str!("scripts/sbuild_config.pl.tmpl");

/// Configuration for a single sbuild invocation.
pub struct SbuildConfig {
    pub dsc_path: PathBuf,
    pub series: String,
    /// Target build architecture (e.g. "amd64").  Passed to sbuild as
    /// `--arch=<arch>`.  Defaults to "amd64" at the CLI layer.
    pub arch: String,
    pub compiler_type: CompilerType,
    pub compiler_version: String,
    /// Extra environment variables for the build (from profile flags).
    pub build_env: Vec<(String, String)>,
    pub timeout_seconds: u64,
    pub verbose: bool,
    pub run_tests: bool,
    pub jobs: usize,
    pub cancel_token: CancellationToken,
}

/// Outcome of a single sbuild run, before database insertion.
pub struct SbuildResult {
    pub status: BuildStatus,
    pub log: String,
    pub duration_seconds: Option<f64>,
    pub peak_memory_mb: Option<i64>,
    pub compiler_detected: Option<String>,
}

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

/// Build a single `.dsc` with sbuild, capturing output and resource metrics.
///
/// The build is wrapped with `/usr/bin/time -v` for wall-time and peak-RSS
/// measurement.  Timeout and cancellation are handled in Rust (not via the
/// `timeout(1)` command) to avoid process-hierarchy issues that caused
/// orphaned chroot processes in earlier iterations.
pub async fn run_sbuild(config: &SbuildConfig) -> Result<SbuildResult> {
    let (mut cmd, _config_file) = build_command(config)?;

    debug!("Spawning: {:?}", cmd);

    let mut child = cmd.spawn().context("Failed to spawn sbuild")?;
    let child_pid = child.id().context("Failed to get child PID")?;
    let pgid = Pid::from_raw(child_pid as i32);

    let stdout = child.stdout.take().context("Failed to capture stdout")?;
    let stderr = child.stderr.take().context("Failed to capture stderr")?;
    let mut stdout_lines = BufReader::new(stdout).lines();
    let mut stderr_lines = BufReader::new(stderr).lines();

    let mut log_lines: Vec<String> = Vec::new();
    let mut time_output = String::new();
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
                            if is_time_output(&line) {
                                time_output.push_str(&line);
                                time_output.push('\n');
                            } else {
                                log_lines.push(line);
                            }
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
            drain_pipes(&mut stdout_lines, &mut stderr_lines).await;
            let _ = child.wait().await;
            return Err(e);
        }
        Err(_elapsed) => {
            timed_out = true;
            info!("Build timed out after {}s, killing process group (pgid={pgid})", config.timeout_seconds);
            kill_process_group(pgid).await;
            drain_pipes(&mut stdout_lines, &mut stderr_lines).await;
        }
    }

    let exit_status = child.wait().await.context("Failed to wait for sbuild")?;
    let log = log_lines.join("\n");
    let metrics = parse_time_output(&time_output);
    let compiler_detected = detect_compiler_from_log(&log, config.compiler_type);

    let exit_code = metrics.exit_status.or_else(|| exit_status.code());
    let status = if timed_out {
        BuildStatus::Timeout
    } else {
        infer_status(&log, exit_code)
    };

    Ok(SbuildResult {
        status,
        log,
        duration_seconds: metrics.wall_time_seconds,
        peak_memory_mb: metrics.peak_memory_kb.map(|kb| kb / 1024),
        compiler_detected: Some(compiler_detected),
    })
}

// ---------------------------------------------------------------------------
// Command construction
// ---------------------------------------------------------------------------

/// HTTP(S) proxy URL injected into the sbuild chroot's apt configuration (see
/// `chroot_setup.sh`).  sbuild's unshare chroot does not inherit the outer
/// shell's `http_proxy` / `https_proxy` env vars, so the pipeline forwards
/// the proxy explicitly via `REBUILD_HTTP_PROXY`.  Empty/unset leaves apt's
/// default config untouched.
fn http_proxy_for_chroot() -> String {
    std::env::var("REBUILD_HTTP_PROXY").unwrap_or_default()
}

/// Assemble the full `Command` for `/usr/bin/time -v sbuild ...`.
///
/// For clang profiles, injects chroot-setup (clang install) and
/// starting-build (gcc-to-clang wrapper) scripts. For gcc profiles,
/// only injects a lightweight verification script.
///
/// Returns the command together with the temporary sbuild config file.
/// The caller must keep the file handle alive until the child process exits,
/// otherwise the file is deleted before sbuild reads it.
fn build_command(config: &SbuildConfig) -> Result<(Command, tempfile::NamedTempFile)> {
    let dsc_dir = config.dsc_path.parent().context("Invalid .dsc path")?;

    let sbuild_config_file = generate_sbuild_config(config.jobs, config.run_tests, &config.build_env)?;

    // sbuild's unshare mode extracts chroots into $TMPDIR which defaults to
    // /tmp. On this machine /tmp is a 44 GB tmpfs and large builds exhaust it,
    // so redirect to real disk instead.
    let scratch_dir = PathBuf::from("/var/tmp/rebuild-builds");
    std::fs::create_dir_all(&scratch_dir)
        .context("Failed to create /var/tmp/rebuild-builds")?;

    let mut cmd = Command::new("/usr/bin/time");
    cmd.arg("-v")
        .arg("sbuild")
        .arg("--verbose")
        .arg("--batch")
        .arg("--purge=always")
        .arg("--chroot-mode=unshare")
        .arg(format!("--dist={}", config.series))
        .arg(format!("--arch={}", config.arch));

    match config.compiler_type {
        CompilerType::Clang => {
            let proxy = http_proxy_for_chroot();
            let setup_cmd = wrap_in_heredoc(
                "clang-install.sh",
                "CLANG_INSTALL_EOF",
                &CHROOT_SETUP_SCRIPT
                    .replace("__CLANG_VERSION__", &config.compiler_version)
                    .replace("__HTTP_PROXY__", &proxy),
            );
            let starting_cmd = wrap_in_heredoc(
                "clang-wrapper-setup.sh",
                "CLANG_WRAPPER_EOF",
                &STARTING_BUILD_SCRIPT
                    .replace("__CLANG_VERSION__", &config.compiler_version),
            );
            cmd.arg(format!("--chroot-setup-commands={setup_cmd}"));
            cmd.arg(format!("--starting-build-commands={starting_cmd}"));
        }
        CompilerType::Gcc => {
            let starting_cmd = wrap_in_heredoc(
                "gcc-verify.sh",
                "GCC_VERIFY_EOF",
                GCC_VERIFY_SCRIPT,
            );
            cmd.arg(format!("--starting-build-commands={starting_cmd}"));
        }
    }

    cmd.arg("--no-clean-source")
        .arg(&config.dsc_path)
        .current_dir(dsc_dir)
        .env("SBUILD_CONFIG", sbuild_config_file.path())
        .env("TMPDIR", &scratch_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    // Spawn in its own process group so we can `killpg` the entire tree.
    // SAFETY: `setpgid` is async-signal-safe (POSIX.1-2017 §2.4.3), which is
    // the only requirement for code inside `pre_exec`.
    unsafe {
        cmd.pre_exec(|| {
            nix::unistd::setpgid(Pid::from_raw(0), Pid::from_raw(0))
                .map_err(std::io::Error::other)?;
            Ok(())
        });
    }

    Ok((cmd, sbuild_config_file))
}

/// Wrap a shell script body in a heredoc that writes it to a temp file inside
/// the chroot and then executes it.  This is how sbuild external commands
/// receive multi-line scripts.
fn wrap_in_heredoc(filename: &str, delimiter: &str, body: &str) -> String {
    format!(
        "cat > /tmp/{filename} << '{delimiter}'\n\
         {body}\n\
         {delimiter}\n\
         chmod +x /tmp/{filename} && /tmp/{filename}"
    )
}

/// Generate a Perl config file that overrides the user's `~/.sbuildrc`.
///
/// Loaded via `SBUILD_CONFIG` which sbuild evaluates after system and user
/// configs, so all assignments here take precedence. Profile flags are
/// injected into `$build_environment` so they reach `dpkg-buildpackage`.
fn generate_sbuild_config(
    jobs: usize,
    run_tests: bool,
    build_env: &[(String, String)],
) -> Result<tempfile::NamedTempFile> {
    let nocheck = if run_tests { "" } else { " nocheck" };

    // Build the Perl hash entries for $build_environment. Each entry is a
    // bare key-value pair; the template provides the surrounding indentation.
    let mut env_entries = vec![
        format!("'DEB_BUILD_OPTIONS' => 'parallel={jobs}{nocheck}',"),
    ];
    for (var, value) in build_env {
        // Perl single-quote escaping: ' becomes '\''
        let escaped = value.replace('\'', "'\\''");
        env_entries.push(format!("'{var}' => '{escaped}',"));
    }
    let env_block = env_entries.join("\n    ");

    let config = SBUILD_CONFIG_TEMPLATE.replace("__ENV_BLOCK__", &env_block);

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

// ---------------------------------------------------------------------------
// Process management
// ---------------------------------------------------------------------------

/// Kill an entire process group: SIGTERM, wait 10 s, then SIGKILL.
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

/// Drain remaining pipe data so a killed child doesn't block on a full buffer.
async fn drain_pipes(
    stdout: &mut tokio::io::Lines<BufReader<tokio::process::ChildStdout>>,
    stderr: &mut tokio::io::Lines<BufReader<tokio::process::ChildStderr>>,
) {
    let _ = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            tokio::select! {
                r = stdout.next_line() => { if !matches!(r, Ok(Some(_))) { break; } }
                r = stderr.next_line() => { if !matches!(r, Ok(Some(_))) { break; } }
            }
        }
    })
    .await;
}

/// Return `true` if this stderr line looks like `/usr/bin/time -v` output
/// rather than sbuild output that should go into the build log.
fn is_time_output(line: &str) -> bool {
    // /usr/bin/time -v prefixes every line with a recognisable label
    line.contains("time (seconds):")
        || line.contains("Maximum resident set size")
        || line.contains("Exit status:")
        || line.contains("Elapsed (wall clock)")
        || line.contains("Command being timed")
        || line.contains("Percent of CPU")
        || line.contains("Major (requiring I/O) page faults")
        || line.contains("Minor (reclaiming a frame) page faults")
        || line.contains("Voluntary context switches")
        || line.contains("Involuntary context switches")
}

// ---------------------------------------------------------------------------
// Build log analysis
// ---------------------------------------------------------------------------

/// Examine the build log for `REBUILD:` verification markers to confirm
/// the expected compiler was used.
///
/// For clang builds, checks that gcc was successfully replaced with clang.
/// For gcc builds, checks that gcc was confirmed present.
///
/// sbuild echoes the full script source before executing it, so markers
/// appearing inside `echo "…"` lines are skipped — only actual output lines
/// (those starting at column 0 with the marker prefix) are considered.
///
/// If neither the success nor the wrapper-failure markers are present, the
/// chroot-setup phase (which installs clang inside the unshare chroot) is
/// suspected of having failed before the verification script could run.
/// Network/proxy errors during `apt-get install clang-NN` are the usual
/// cause; those emit `REBUILD-ERROR: ...` markers from `chroot_setup.sh`
/// which we surface here rather than reporting the opaque "UNKNOWN".
fn detect_compiler_from_log(log: &str, compiler_type: CompilerType) -> String {
    let mut success = false;
    let mut failed = false;
    let mut version_line: Option<&str> = None;
    // First `REBUILD-ERROR:` line emitted by chroot_setup.sh, captured so we
    // can report the underlying cause (e.g. apt failure) rather than a bare
    // "no verification markers found".
    let mut chroot_setup_error: Option<&str> = None;

    for line in log.lines() {
        let trimmed = line.trim();

        // Skip lines that are part of the echoed script source
        if trimmed.starts_with("echo ")
            || trimmed.starts_with('"')
            || trimmed.starts_with('\'')
        {
            continue;
        }

        // Capture chroot-setup failures regardless of compiler type — they
        // abort the build before the verification phase runs.
        if chroot_setup_error.is_none()
            && trimmed.starts_with("REBUILD-ERROR:")
            && !trimmed.contains("gcc is NOT reporting as clang")
        {
            chroot_setup_error = Some(trimmed);
        }

        match compiler_type {
            CompilerType::Clang => {
                if trimmed == "REBUILD: SUCCESS - gcc is now clang" {
                    success = true;
                }
                if trimmed.starts_with("REBUILD-ERROR: FAILED - gcc is NOT reporting as clang") {
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
                if trimmed.starts_with("REBUILD:   gcc --version:") && trimmed.contains("gcc") {
                    version_line = Some(trimmed);
                }
            }
        }
    }

    if compiler_type == CompilerType::Clang && failed && !success {
        return "ERROR: gcc wrapper setup FAILED - built with real GCC".into();
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

    // The verification script never ran.  If the chroot-setup phase emitted
    // an error marker (e.g. apt couldn't install clang through a proxy),
    // surface that as the proximate cause.
    if let Some(err) = chroot_setup_error {
        return format!("ERROR: chroot setup failed - {err}");
    }

    "UNKNOWN: no compiler verification markers found in log".into()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- shell script generation --------------------------------------------

    #[test]
    fn chroot_setup_substitutes_version() {
        let script = CHROOT_SETUP_SCRIPT
            .replace("__CLANG_VERSION__", "19")
            .replace("__HTTP_PROXY__", "");
        assert!(script.contains(r#"CLANG_VERSION="19""#));
        assert!(!script.contains("__CLANG_VERSION__"));
        assert!(!script.contains("__HTTP_PROXY__"));
        // Empty proxy must leave the if-branch dead: `if [ -n "" ]` is false.
        assert!(script.contains("if [ -n \"\" ]; then"));
    }

    #[test]
    fn chroot_setup_substitutes_proxy() {
        let script = CHROOT_SETUP_SCRIPT
            .replace("__CLANG_VERSION__", "19")
            .replace("__HTTP_PROXY__", "http://proxy.example:3128");
        // The script source keeps its backslash-escaped quotes literally, so
        // check the substituted URL appears in both Proxy lines.
        assert!(script.contains("Acquire::http::Proxy"));
        assert!(script.contains("Acquire::https::Proxy"));
        assert!(script.contains("http://proxy.example:3128"));
        assert!(script.contains("if [ -n \"http://proxy.example:3128\" ]; then"));
        assert!(!script.contains("__HTTP_PROXY__"));
        assert!(!script.contains("__CLANG_VERSION__"));
    }

    #[test]
    fn starting_build_substitutes_version() {
        let script = STARTING_BUILD_SCRIPT
            .replace("__CLANG_VERSION__", "19");
        assert!(script.contains(r#"CLANG_VERSION="19""#));
        assert!(!script.contains("__CLANG_VERSION__"));
    }

    #[test]
    fn starting_build_contains_verification_markers() {
        assert!(STARTING_BUILD_SCRIPT.contains("REBUILD: SUCCESS"));
        assert!(STARTING_BUILD_SCRIPT.contains("REBUILD-ERROR: FAILED"));
    }

    #[test]
    fn starting_build_no_placeholders_remain() {
        let script = STARTING_BUILD_SCRIPT
            .replace("__CLANG_VERSION__", "18");
        assert!(!script.contains("__CLANG_VERSION__"));
    }

    #[test]
    fn gcc_verify_script_contains_markers() {
        assert!(GCC_VERIFY_SCRIPT.contains("REBUILD: SUCCESS - gcc confirmed"));
    }

    #[test]
    fn heredoc_wraps_script() {
        let cmd = wrap_in_heredoc("test.sh", "EOF", "echo hello");
        assert!(cmd.starts_with("cat > /tmp/test.sh << 'EOF'"));
        assert!(cmd.contains("echo hello"));
        assert!(cmd.ends_with("chmod +x /tmp/test.sh && /tmp/test.sh"));
    }

    // -- detect_compiler_from_log -------------------------------------------

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
        assert!(detect_compiler_from_log("some build output\n", CompilerType::Clang).contains("UNKNOWN"));
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
        // apt-get install clang-NN failed inside the chroot (e.g. no proxy).
        // chroot_setup.sh emits REBUILD-ERROR and exits 1 before the
        // starting-build verification script ever runs.
        let log = "=== REBUILD: Installing Clang 18 ===\n\
                   REBUILD-ERROR: Failed to install clang-18 (check proxy / archive reachability)\n";
        let result = detect_compiler_from_log(log, CompilerType::Clang);
        assert!(result.starts_with("ERROR: chroot setup failed"), "got: {result}");
        assert!(result.contains("Failed to install clang-18"), "got: {result}");
    }

    #[test]
    fn chroot_setup_error_does_not_override_wrapper_failure() {
        // If the wrapper setup also failed, that's the more specific failure
        // and should win over the earlier chroot-setup warning.
        let log = concat!(
            "REBUILD-ERROR: Failed to install clang-18 (check proxy / archive reachability)\n",
            "REBUILD-ERROR: FAILED - gcc is NOT reporting as clang!\n",
        );
        let result = detect_compiler_from_log(log, CompilerType::Clang);
        assert!(result.contains("wrapper setup FAILED"), "got: {result}");
    }

    #[test]
    fn chroot_setup_error_marker_not_confused_with_wrapper_error() {
        // The wrapper-failure marker also starts with REBUILD-ERROR: but must
        // not be mis-attributed to chroot setup.
        let log = "REBUILD-ERROR: FAILED - gcc is NOT reporting as clang!\n";
        let result = detect_compiler_from_log(log, CompilerType::Clang);
        assert!(result.contains("wrapper setup FAILED"), "got: {result}");
    }
}
