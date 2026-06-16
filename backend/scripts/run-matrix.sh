#!/bin/bash
# Run rebuilder builds across any combination of profiles and package sets.
#
# Discovers profiles automatically from profiles/*.toml. Filters narrow the
# set before execution. Use --dry-run to see what would run without building.
#
# Usage:
#   ./scripts/run-matrix.sh [OPTIONS]
#
# Options:
#   --packages FILE    Package list to build (default: packages.txt)
#   --series SERIES    Only run profiles targeting this Ubuntu series
#   --compiler TYPE    Only run profiles with this compiler type (clang|gcc)
#   --profile GLOB     Only run profiles whose name matches this glob pattern
#                      May be given multiple times (any match wins)
#   --jobs N           Parallel make jobs per build (default: CPU count)
#   --no-export        Skip the frontend export step after all builds finish
#   --dry-run          Print what would run without executing anything
#
# Examples:
#   # Run every profile against the default package set
#   ./scripts/run-matrix.sh
#
#   # Quick test: all noble profiles against a small package list
#   ./scripts/run-matrix.sh --series noble --packages packages-smoke.txt
#
#   # Single profile
#   ./scripts/run-matrix.sh --profile clang-21-resolute
#
#   # All Clang profiles, any series, medium package set
#   ./scripts/run-matrix.sh --compiler clang --packages packages-medium.txt
#
#   # See what a full run would do without building
#   ./scripts/run-matrix.sh --dry-run

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PIPELINE_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
PROFILES_DIR="$(cd "$SCRIPT_DIR/../../profiles" && pwd)"
FRONTEND_DIR="$(cd "$SCRIPT_DIR/../../frontend" && pwd)"

CARGO_BIN="$PIPELINE_DIR/target/release/rebuilder"
PACKAGES="$PIPELINE_DIR/packages.txt"
FILTER_SERIES=""
FILTER_COMPILER=""
FILTER_PROFILES=()   # array; empty = no filter
JOBS=""
DO_EXPORT=1
DRY_RUN=0

export REBUILD_DB="$PIPELINE_DIR/rebuilder.db"

# ---------------------------------------------------------------------------
# Argument parsing
# ---------------------------------------------------------------------------

while [[ $# -gt 0 ]]; do
    case "$1" in
        --packages)  PACKAGES="$2";          shift 2 ;;
        --series)    FILTER_SERIES="$2";     shift 2 ;;
        --compiler)  FILTER_COMPILER="$2";   shift 2 ;;
        --profile)   FILTER_PROFILES+=("$2"); shift 2 ;;
        --jobs)      JOBS="$2";              shift 2 ;;
        --no-export) DO_EXPORT=0;            shift ;;
        --dry-run)   DRY_RUN=1;              shift ;;
        -h|--help)
            sed -n '3,/^set -/{ /^set -/d; s/^# \{0,1\}//; p }' "$0"
            exit 0
            ;;
        *) echo "Unknown option: $1" >&2; exit 1 ;;
    esac
done

# ---------------------------------------------------------------------------
# Pre-flight
# ---------------------------------------------------------------------------

if [[ "$DRY_RUN" -eq 0 && ! -f "$CARGO_BIN" ]]; then
    echo "Pipeline binary not found: $CARGO_BIN" >&2
    echo "Run: cd backend && cargo build --release" >&2
    exit 1
fi

if [[ ! -f "$PACKAGES" ]]; then
    echo "Package list not found: $PACKAGES" >&2
    exit 1
fi

# ---------------------------------------------------------------------------
# Parse a TOML profile file — extract series and compiler type.
# The TOML is simple enough that line-oriented grep/awk is sufficient.
# Returns 1 if either field is missing (malformed profile).
# ---------------------------------------------------------------------------

parse_profile() {
    local toml="$1"
    local varname_series="$2"
    local varname_compiler="$3"

    local _series _compiler
    _series=$(awk -F'"' '/^series[[:space:]]*=/{print $2}' "$toml")
    _compiler=$(awk -F'"' '/^type[[:space:]]*=/{print $2}' "$toml")

    if [[ -z "$_series" || -z "$_compiler" ]]; then
        return 1
    fi
    printf -v "$varname_series"   '%s' "$_series"
    printf -v "$varname_compiler" '%s' "$_compiler"
}

# ---------------------------------------------------------------------------
# Profile discovery and filtering
# ---------------------------------------------------------------------------

SELECTED_PROFILES=()   # array of profile stems that passed all filters
SKIP_REASONS=()        # parallel array of skip reasons for dry-run

discover_profiles() {
    local toml stem series compiler
    local matched_glob

    for toml in "$PROFILES_DIR"/*.toml; do
        [[ -f "$toml" ]] || continue
        stem="$(basename "$toml" .toml)"

        # Parse TOML — skip malformed files.
        if ! parse_profile "$toml" series compiler; then
            echo "WARNING: Could not parse $toml — skipping" >&2
            continue
        fi

        # Apply --series filter.
        if [[ -n "$FILTER_SERIES" && "$series" != "$FILTER_SERIES" ]]; then
            SKIP_REASONS+=("series $series != $FILTER_SERIES")
            continue
        fi

        # Apply --compiler filter.
        if [[ -n "$FILTER_COMPILER" && "$compiler" != "$FILTER_COMPILER" ]]; then
            SKIP_REASONS+=("compiler $compiler != $FILTER_COMPILER")
            continue
        fi

        # Apply --profile glob filters (union: any match wins).
        if [[ "${#FILTER_PROFILES[@]}" -gt 0 ]]; then
            matched_glob=0
            for pat in "${FILTER_PROFILES[@]}"; do
                # shellcheck disable=SC2254
                case "$stem" in
                    $pat) matched_glob=1; break ;;
                esac
            done
            if [[ "$matched_glob" -eq 0 ]]; then
                SKIP_REASONS+=("no --profile glob matched")
                continue
            fi
        fi

        SELECTED_PROFILES+=("$stem")
    done
}

discover_profiles

PKG_COUNT=$(grep -cv '^\(#\|[[:space:]]*$\)' "$PACKAGES" || true)

# ---------------------------------------------------------------------------
# Summary header
# ---------------------------------------------------------------------------

echo "=== Rebuild matrix ==="
echo "Profiles:  $PROFILES_DIR"
echo "Packages:  $PACKAGES ($PKG_COUNT packages)"
echo "Database:  $REBUILD_DB"
echo "Frontend:  $FRONTEND_DIR/data"
[[ -n "$FILTER_SERIES"   ]] && echo "Filter:    series=$FILTER_SERIES"
[[ -n "$FILTER_COMPILER" ]] && echo "Filter:    compiler=$FILTER_COMPILER"
for pat in "${FILTER_PROFILES[@]}"; do
    echo "Filter:    profile glob=$pat"
done
echo ""

if [[ "${#SELECTED_PROFILES[@]}" -eq 0 ]]; then
    echo "No profiles match the given filters." >&2
    exit 1
fi

# Sort profiles for deterministic output: GCC first (baseline), then Clang;
# within each type, alphabetically.
mapfile -t SELECTED_PROFILES < <(
    for stem in "${SELECTED_PROFILES[@]}"; do
        toml="$PROFILES_DIR/${stem}.toml"
        _discard_series=""
        compiler_sort=""
        parse_profile "$toml" _discard_series compiler_sort 2>/dev/null || compiler_sort="zzz"
        # Put gcc before clang so baseline runs first.
        [[ "$compiler_sort" == "gcc" ]] && prefix="0" || prefix="1"
        echo "${prefix}${stem}"
    done | sort | sed 's/^.//'
)

TOTAL="${#SELECTED_PROFILES[@]}"
echo "Will run $TOTAL profile(s):"
for stem in "${SELECTED_PROFILES[@]}"; do
    toml="$PROFILES_DIR/${stem}.toml"
    parse_profile "$toml" p_series p_compiler 2>/dev/null || { p_series="?"; p_compiler="?"; }
    printf "  %-36s  %s / %s\n" "$stem" "$p_compiler" "$p_series"
done
echo ""

# ---------------------------------------------------------------------------
# Dry-run exit
# ---------------------------------------------------------------------------

if [[ "$DRY_RUN" -eq 1 ]]; then
    echo "[DRY RUN] No builds were executed."
    exit 0
fi

# ---------------------------------------------------------------------------
# Build loop
# ---------------------------------------------------------------------------

JOBS_ARG=()
[[ -n "$JOBS" ]] && JOBS_ARG=(--jobs "$JOBS")

FAILED=()
IDX=0

for stem in "${SELECTED_PROFILES[@]}"; do
    IDX=$((IDX + 1))
    toml="$PROFILES_DIR/${stem}.toml"

    echo "--- [$IDX/$TOTAL] $stem ---"

    if "$CARGO_BIN" build \
            --profile "$toml" \
            --packages "$PACKAGES" \
            "${JOBS_ARG[@]}"; then
        echo ""
    else
        echo "WARNING: $stem exited non-zero" >&2
        FAILED+=("$stem")
        echo ""
    fi
done

# ---------------------------------------------------------------------------
# Export
# ---------------------------------------------------------------------------

if [[ "$DO_EXPORT" -eq 1 ]]; then
    echo "--- Exporting to frontend ---"
    "$CARGO_BIN" export --output-dir "$FRONTEND_DIR/data"
    echo ""
fi

# ---------------------------------------------------------------------------
# Summary
# ---------------------------------------------------------------------------

echo "=== Done ==="
"$CARGO_BIN" list

NFAILED="${#FAILED[@]}"
NSKIPPED=0   # reserved for future skip-existing implementation
NRAN=$((TOTAL - NSKIPPED))

echo ""
echo "  Ran:     $NRAN"
[[ "$NSKIPPED" -gt 0 ]] && echo "  Skipped: $NSKIPPED"
[[ "$NFAILED"  -gt 0 ]] && echo "  Failed:  $NFAILED"

if [[ "$NFAILED" -gt 0 ]]; then
    echo ""
    echo "Profiles that reported errors:" >&2
    for p in "${FAILED[@]}"; do
        echo "  $p" >&2
    done
    exit 1
fi
