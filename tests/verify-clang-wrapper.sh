#!/bin/bash
# Integration test for the Clang compiler-substitution scripts: a real sbuild
# of 'hello' through the same chroot-setup/starting-build pipeline production
# uses. Asserts on the captured log, not the build outcome.
# Needs: sbuild, pull-lp-source, clang-18 in the archive.

set -euo pipefail

CLANG_VERSION="${1:-18}"
SERIES="${2:-noble}"
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PIPELINE_SCRIPTS="$SCRIPT_DIR/../backend/src/builder/scripts"

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

FAILURES=0

WORK_DIR=$(mktemp -d /var/tmp/rebuild-verify-XXXXXX)
LOG_FILE="$WORK_DIR/sbuild.log"
trap 'rm -rf "$WORK_DIR"' EXIT

echo "=== Clang wrapper integration test (clang-$CLANG_VERSION, $SERIES) ==="
echo "Working directory: $WORK_DIR"
echo ""

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

CHROOT_SETUP_SCRIPT=$(sed "s/__CLANG_VERSION__/$CLANG_VERSION/g" \
    "$PIPELINE_SCRIPTS/chroot_setup.sh")

STARTING_BUILD_SCRIPT=$(sed "s/__CLANG_VERSION__/$CLANG_VERSION/g" \
    "$PIPELINE_SCRIPTS/starting_build.sh")

wrap_in_heredoc() {
    local filename="$1"
    local delimiter="$2"
    local body="$3"
    printf "cat > /tmp/%s << '%s'\n%s\n%s\nchmod +x /tmp/%s && /tmp/%s" \
        "$filename" "$delimiter" "$body" "$delimiter" "$filename" "$filename"
}

CHROOT_CMD=$(wrap_in_heredoc "clang-install.sh" "CLANG_INSTALL_EOF" "$CHROOT_SETUP_SCRIPT")
STARTING_CMD=$(wrap_in_heredoc "clang-wrapper-setup.sh" "CLANG_WRAPPER_EOF" "$STARTING_BUILD_SCRIPT")

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

SCRATCH_DIR=/var/tmp/rebuild-builds
mkdir -p "$SCRATCH_DIR"

echo "--- Running sbuild (this will take a few minutes) ---"

# Build exit code is irrelevant; the wrapper mechanism is what's tested.
set +e
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

echo "--- Assertions ---"

assert_in_log \
    "clang-$CLANG_VERSION was installed" \
    "REBUILD: Clang installed:"

assert_in_log \
    "wrapper file contains correct shebang" \
    "#!/bin/sh"

assert_in_log \
    "wrapper file exec line names clang-$CLANG_VERSION" \
    "exec /usr/bin/clang-$CLANG_VERSION"

# sbuild eating the %s before the shell sees it would produce 'exec  "$@"'.
assert_not_in_log \
    "wrapper exec line is not broken (no bare 'exec  \"\$@\"')" \
    'exec  "$@"'

assert_in_log \
    "gcc symlink replacement logged" \
    "REBUILD:   Replaced /usr/bin/gcc"

assert_in_log \
    "gcc --version reports clang after substitution" \
    "REBUILD:   gcc --version: Ubuntu clang version"

assert_in_log \
    "success marker present" \
    "REBUILD: SUCCESS - gcc is now clang"

assert_in_log \
    "wrapper setup script ran to completion" \
    "REBUILD: Clang $CLANG_VERSION substitution complete"

assert_not_in_log \
    "no compiler verification failures" \
    "REBUILD-ERROR: FAILED -"

# Pre-setup output is excluded by the anchored pattern.
if grep -E '^REBUILD:   [^ ]+ --version:' "$LOG_FILE" | grep -qv clang; then
    fail "all wrapped compilers report as clang (some --version line lacks clang)"
else
    pass "all wrapped compilers report as clang"
fi

echo ""
if [ "$FAILURES" -eq 0 ]; then
    echo "=== ALL ASSERTIONS PASSED ==="
    exit 0
else
    echo "=== $FAILURES ASSERTION(S) FAILED ===" >&2
    echo "Full log: $LOG_FILE (preserved)" >&2
    trap - EXIT
    exit 1
fi
