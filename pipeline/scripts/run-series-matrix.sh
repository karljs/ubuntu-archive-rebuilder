#!/bin/bash
# Run default GCC and Clang builds across Jammy, Noble, and Resolute.
#
# Uses the largest package set (packages.txt) by default.
#
# Usage:
#   ./scripts/run-series-matrix.sh [--packages FILE] [--jobs N] [--smoke]
#
# Options:
#   --packages FILE   Package list to use (default: packages.txt)
#   --jobs N          Parallel make jobs per build (default: CPU count)
#   --smoke           Use packages-smoke.txt (quick sanity check)

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PIPELINE_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
PROFILES_DIR="$(cd "$SCRIPT_DIR/../../profiles" && pwd)"
VIEWER_DIR="$(cd "$SCRIPT_DIR/../../viewer" && pwd)"

CARGO_RUN="cargo run --manifest-path $PIPELINE_DIR/Cargo.toml --quiet --"
PACKAGES="$PIPELINE_DIR/packages.txt"
JOBS=""

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

PKG_COUNT=$(grep -cv "^\(#\|$\)" "$PACKAGES" || true)

echo "=== Series matrix: Jammy / Noble / Resolute ==="
echo "Database:  $REBUILD_DB"
echo "Packages:  $PACKAGES ($PKG_COUNT packages)"
echo "Profiles:  $PROFILES_DIR"
echo "Output:    $VIEWER_DIR/data"
echo ""

# ---------------------------------------------------------------------------
# Profiles: GCC baseline first per series, then Clang vanilla, then Clang
# with dwarf workaround.
#
#   Jammy:    gcc-11  / clang-14 vanilla / clang-14 with -gdwarf-4
#   Noble:    gcc-13  / clang-18 vanilla / clang-18 with -gdwarf-4
#   Resolute: gcc-15  / clang-21 vanilla / clang-21 with -gdwarf-4
#
# All three series need the -gdwarf-4 workaround: their dwz versions
# (0.14, 0.15, 0.16) don't fully support Clang's DWARF5 output.
# Each gets vanilla + dwarf-fix profiles to visualize the impact.
# ---------------------------------------------------------------------------

PROFILES=(
    gcc-11-jammy
    clang-14-jammy-vanilla
    clang-14-jammy
    gcc-13-noble
    clang-18-noble-vanilla
    clang-18-noble
    gcc-15-resolute
    clang-21-resolute-vanilla
    clang-21-resolute
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
