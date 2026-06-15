#!/bin/bash
# Run a build batch for each profile and export results to the viewer.
#
# Usage:
#   ./scripts/run-test-matrix.sh [--packages FILE] [--jobs N] [--smoke]
#
# Options:
#   --packages FILE   Package list to use (default: packages-medium.txt)
#   --jobs N          Parallel make jobs per build (default: CPU count)
#   --smoke           Use packages-smoke.txt (quick sanity check)

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PIPELINE_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
PROFILES_DIR="$(cd "$SCRIPT_DIR/../../profiles" && pwd)"
VIEWER_DIR="$(cd "$SCRIPT_DIR/../../viewer" && pwd)"

CARGO_RUN="cargo run --manifest-path $PIPELINE_DIR/Cargo.toml --quiet --"
PACKAGES="$PIPELINE_DIR/packages-medium.txt"
JOBS=""

# Pin the database to a fixed absolute path so the location is consistent
# regardless of which directory the script is invoked from.
export REBUILD_DB="$PIPELINE_DIR/rebuild-experiments.db"

# ---------------------------------------------------------------------------
# Argument parsing
# ---------------------------------------------------------------------------

while [[ $# -gt 0 ]]; do
    case "$1" in
        --packages) PACKAGES="$2"; shift 2 ;;
        --jobs)     JOBS="$2";     shift 2 ;;
        --smoke)    PACKAGES="$PIPELINE_DIR/packages-smoke.txt"; shift ;;
        *) echo "Unknown option: $1" >&2; exit 1 ;;
    esac
done

# ---------------------------------------------------------------------------
# Pre-flight
# ---------------------------------------------------------------------------

if [ ! -f "$PACKAGES" ]; then
    echo "Package list not found: $PACKAGES" >&2
    exit 1
fi

PKG_COUNT=$(grep -v "^#" "$PACKAGES" | grep -v "^$" | wc -l)

echo "=== Rebuild test matrix ==="
echo "Database:  $REBUILD_DB"
echo "Packages:  $PACKAGES ($PKG_COUNT packages)"
echo "Profiles:  $PROFILES_DIR"
echo "Output:    $VIEWER_DIR/data"
echo ""

# ---------------------------------------------------------------------------
# Profiles to run (in a sensible order: GCC baselines first, then Clang)
# ---------------------------------------------------------------------------

PROFILES=(
    gcc-13-noble
    gcc-14-noble
    clang-18-noble
    clang-19-noble
    clang-20-noble
)

JOBS_ARG=""
[ -n "$JOBS" ] && JOBS_ARG="--jobs $JOBS"

FAILED_PROFILES=()

for profile in "${PROFILES[@]}"; do
    profile_file="$PROFILES_DIR/${profile}.toml"
    if [ ! -f "$profile_file" ]; then
        echo "SKIP: $profile (profile file not found)"
        continue
    fi

    echo "--- $profile ---"
    if $CARGO_RUN build \
        --profile "$profile_file" \
        --packages "$PACKAGES" \
        $JOBS_ARG; then
        echo ""
    else
        echo "WARNING: $profile exited non-zero" >&2
        FAILED_PROFILES+=("$profile")
        echo ""
    fi
done

# ---------------------------------------------------------------------------
# Export
# ---------------------------------------------------------------------------

echo "--- Exporting to viewer ---"
$CARGO_RUN export --output-dir "$VIEWER_DIR/data"
echo ""

# ---------------------------------------------------------------------------
# Summary
# ---------------------------------------------------------------------------

echo "=== Done ==="
$CARGO_RUN list

if [ ${#FAILED_PROFILES[@]} -gt 0 ]; then
    echo ""
    echo "WARNING: The following profiles reported errors during the run:" >&2
    for p in "${FAILED_PROFILES[@]}"; do echo "  $p" >&2; done
fi
