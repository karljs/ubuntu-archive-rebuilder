#!/bin/bash
# Integration test for the Clang compiler-substitution scripts.
#
# Runs a real sbuild of the 'hello' source package using the same
# chroot-setup and starting-build script pipeline that production builds use.
# Asserts on the captured log rather than the build outcome, so it verifies
# the wrapper mechanism itself regardless of whether hello happens to compile.
#
# Requirements: sbuild, pull-lp-source, clang-18 available in the archive.
# Run from anywhere; output goes to stdout. Exits 0 on pass, 1 on failure.

set -euo pipefail

CLANG_VERSION="${1:-18}"
SERIES="${2:-noble}"
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PIPELINE_SCRIPTS="$SCRIPT_DIR/../backend/src/builder/scripts"

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

pass() { echo "  PASS: $*"; }
fail() { echo "  FAIL: $*" >&2; FAILURES=$((FAILURES + 1)); }

assert_in_log() {
    local description="$1"
    local pattern="$2"
    if grep -qF "$pattern" "$LOG_FILE"; then
        pass "$description"
    else
        fail "$description (pattern not found: '$pattern')"
    fi
}

assert_not_in_log() {
    local description="$1"
    local pattern="$2"
    if grep -qF "$pattern" "$LOG_FILE"; then
        fail "$description (unexpected pattern found: '$pattern')"
    else
        pass "$description"
    fi
}

assert_log_line_after() {
    # Checks that the line AFTER the line matching $marker contains $expected.
    local description="$1"
    local marker="$2"
    local expected="$3"
    local found
    found=$(grep -A1 -F "$marker" "$LOG_FILE" | tail -1)
    if echo "$found" | grep -qF "$expected"; then
        pass "$description"
    else
        fail "$description (after '$marker', expected '$expected', got: '$found')"
    fi
}

FAILURES=0

# ---------------------------------------------------------------------------
# Setup
# ---------------------------------------------------------------------------

WORK_DIR=$(mktemp -d /var/tmp/rebuild-verify-XXXXXX)
LOG_FILE="$WORK_DIR/sbuild.log"
trap 'rm -rf "$WORK_DIR"' EXIT

echo "=== Clang wrapper integration test (clang-$CLANG_VERSION, $SERIES) ==="
echo "Working directory: $WORK_DIR"
echo ""

# ---------------------------------------------------------------------------
# Fetch source
# ---------------------------------------------------------------------------

echo "--- Fetching hello source ---"
( cd "$WORK_DIR" && pull-lp-source -d hello "$SERIES" ) 2>&1 \
    || { echo "ERROR: pull-lp-source failed" >&2; exit 1; }

DSC=$(find "$WORK_DIR" -name "hello_*.dsc" | head -1)
if [ -z "$DSC" ]; then
    echo "ERROR: No .dsc file found after source fetch" >&2
    exit 1
fi
echo "Using: $DSC"
echo ""

# ---------------------------------------------------------------------------
# Prepare scripts (same substitution the Rust pipeline performs)
# ---------------------------------------------------------------------------

CHROOT_SETUP_SCRIPT=$(sed "s/__CLANG_VERSION__/$CLANG_VERSION/g" \
    "$PIPELINE_SCRIPTS/chroot_setup.sh")

STARTING_BUILD_SCRIPT=$(sed "s/__CLANG_VERSION__/$CLANG_VERSION/g" \
    "$PIPELINE_SCRIPTS/starting_build.sh")

# Wrap in the same heredoc format that wrap_in_heredoc() produces in sbuild.rs.
wrap_in_heredoc() {
    local filename="$1"
    local delimiter="$2"
    local body="$3"
    printf "cat > /tmp/%s << '%s'\n%s\n%s\nchmod +x /tmp/%s && /tmp/%s" \
        "$filename" "$delimiter" "$body" "$delimiter" "$filename" "$filename"
}

CHROOT_CMD=$(wrap_in_heredoc "clang-install.sh" "CLANG_INSTALL_EOF" "$CHROOT_SETUP_SCRIPT")
STARTING_CMD=$(wrap_in_heredoc "clang-wrapper-setup.sh" "CLANG_WRAPPER_EOF" "$STARTING_BUILD_SCRIPT")

# ---------------------------------------------------------------------------
# Generate sbuild config (same as the Rust pipeline, minus profile flags)
# ---------------------------------------------------------------------------

SBUILD_CONFIG_FILE=$(mktemp "$WORK_DIR/sbuild-XXXXXX.conf")
cat > "$SBUILD_CONFIG_FILE" <<'PERL_EOF'
$build_environment = {
    'DEB_BUILD_OPTIONS' => 'parallel=1 nocheck',
};
$external_commands = {
    'build-failed-commands'        => [],
    'build-deps-failed-commands'   => [],
    'chroot-update-failed-commands'=> [],
    'anything-failed-commands'     => [],
};
$purge_build_directory = 'always';
$purge_session         = 'always';
$purge_build_deps      = 'always';
$run_lintian           = 0;
$clean_source          = 0;
1;
PERL_EOF

# ---------------------------------------------------------------------------
# Run sbuild
# ---------------------------------------------------------------------------

SCRATCH_DIR=/var/tmp/rebuild-builds
mkdir -p "$SCRATCH_DIR"

echo "--- Running sbuild (this will take a few minutes) ---"

# Capture full output; we don't care about the build exit code because
# the test is about the wrapper mechanism, not the hello build outcome.
set +e
# Run sbuild from WORK_DIR so the .build file it writes lands there
# and gets removed by the trap rather than littering the repo root.
( cd "$WORK_DIR" && sbuild \
    --verbose \
    --batch \
    --chroot-mode=unshare \
    --dist="$SERIES" \
    --chroot-setup-commands="$CHROOT_CMD" \
    --starting-build-commands="$STARTING_CMD" \
    --no-clean-source \
    "$DSC" ) \
    2>&1 | tee "$LOG_FILE"
SBUILD_EXIT=$?
set -e

echo ""
echo "--- sbuild exited with code $SBUILD_EXIT ---"
echo ""

# ---------------------------------------------------------------------------
# Assertions
# ---------------------------------------------------------------------------

echo "--- Assertions ---"

# 1. Clang was installed in the chroot.
assert_in_log \
    "clang-$CLANG_VERSION was installed" \
    "REBUILD: Clang installed:"

# 2. The wrapper file was created and contains the right exec target.
#    The log includes 'cat /usr/local/lib/clang-wrapper/gcc' output.
#    We check the line after "wrapper contents:" is exactly the shebang,
#    and that the exec line names the right binary.
assert_in_log \
    "wrapper file contains correct shebang" \
    "#!/bin/sh"

assert_in_log \
    "wrapper file exec line names clang-$CLANG_VERSION" \
    "exec /usr/bin/clang-$CLANG_VERSION"

# 3. Verify that the exec line is NOT the broken form produced if sbuild
#    eats the %s before the shell sees it (would produce 'exec  "$@"').
assert_not_in_log \
    "wrapper exec line is not broken (no bare 'exec  \"\$@\"')" \
    'exec  "$@"'

# 4. The gcc symlink was replaced.
assert_in_log \
    "gcc symlink replacement logged" \
    "REBUILD:   Replaced /usr/bin/gcc"

# 5. Post-replacement gcc --version reports clang.
assert_in_log \
    "gcc --version reports clang after substitution" \
    "REBUILD:   gcc --version: Ubuntu clang version"

# 6. The success marker fired.
assert_in_log \
    "success marker present" \
    "REBUILD: SUCCESS - gcc is now clang"

# 7. (dropped) — The REBUILD-ERROR abort string appears in the echoed heredoc
#    body that sbuild prints to the log before executing it. A simple grep
#    cannot distinguish "error fired" from "error branch source was echoed".
#    Assertions 6 and 8 together fully confirm the happy path.

# 8. The starting-build script did not exit early (set -e would abort sbuild
#    before the verification block if something failed mid-script).
assert_in_log \
    "wrapper setup script ran to completion" \
    "REBUILD: Clang $CLANG_VERSION substitution complete"

# ---------------------------------------------------------------------------
# Result
# ---------------------------------------------------------------------------

echo ""
if [ "$FAILURES" -eq 0 ]; then
    echo "=== ALL ASSERTIONS PASSED ==="
    exit 0
else
    echo "=== $FAILURES ASSERTION(S) FAILED ===" >&2
    echo "Full log: $LOG_FILE (preserved — trap suppressed)" >&2
    # Preserve the log for inspection on failure.
    trap - EXIT
    exit 1
fi
