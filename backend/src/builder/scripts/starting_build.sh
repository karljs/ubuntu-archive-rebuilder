#!/bin/bash
# Replaces gcc/g++/cc/c++ with clang wrappers inside the sbuild chroot.
#
# Runs as --starting-build-commands (AFTER build-dependency installation,
# BEFORE dpkg-buildpackage). By this point gcc is fully installed via
# build-deps, so we can reliably divert and replace it.
#
# Placeholder __CLANG_VERSION__ is replaced at runtime by the pipeline.
#
# IMPORTANT: sbuild processes percent escapes in external command strings.
# Any literal '%' in this script must be doubled ('%%') because sbuild
# interprets sequences like %s before the shell sees the command.
# See sbuild(1) § "OPTION STRING PERCENT ESCAPES".
set -e

CLANG_VERSION="__CLANG_VERSION__"
CLANG_BIN="clang-$CLANG_VERSION"
CLANGXX_BIN="clang++-$CLANG_VERSION"
WRAPPER_DIR="/usr/local/lib/clang-wrapper"

echo "=== REBUILD: Setting up Clang $CLANG_VERSION compiler wrappers ==="
echo "REBUILD: Pre-setup state:"
echo "REBUILD:   /usr/bin/gcc -> $(readlink -f /usr/bin/gcc 2>/dev/null || echo 'NOT FOUND')"
echo "REBUILD:   /usr/bin/g++ -> $(readlink -f /usr/bin/g++ 2>/dev/null || echo 'NOT FOUND')"
echo "REBUILD:   /usr/bin/cc  -> $(readlink -f /usr/bin/cc 2>/dev/null || echo 'NOT FOUND')"
echo "REBUILD:   gcc --version (pre-setup): $(gcc --version 2>/dev/null | head -1 || echo 'NOT FOUND')"

mkdir -p "$WRAPPER_DIR"

# Create a wrapper script that execs the given compiler.
# The '%%s' is intentional — sbuild expands '%s' before the shell runs,
# so we double-escape to get a literal '%s' for printf.
create_wrapper() {
    local name="$1"
    local target="$2"
    printf '#!/bin/sh\nexec %%s "$@"\n' "$target" > "$WRAPPER_DIR/$name"
    chmod +x "$WRAPPER_DIR/$name"
    echo "REBUILD:   Created wrapper: $name -> $target"
}

# Collect every gcc/g++ name present in the chroot: the plain names, all
# versioned variants (gcc-15, g++-12, ...), and the target-triple-prefixed
# forms (x86_64-linux-gnu-gcc[-15]).  Globs require a digit after the dash
# so binutils front-ends (gcc-ar, gcc-nm, gcc-ranlib) are not wrapped.  A
# hardcoded version list would silently leave newer compilers (gcc-15+ on
# resolute) unwrapped — packages invoking them directly would compile with
# real GCC inside a Clang batch.
ARCH=$(dpkg-architecture -qDEB_HOST_GNU_TYPE 2>/dev/null || echo "")
NAMES=(gcc g++ cc c++)
for p in /usr/bin/gcc-[0-9]* /usr/bin/g++-[0-9]*; do
    [ -e "$p" ] || continue
    NAMES+=("$(basename "$p")")
done
if [ -n "$ARCH" ]; then
    NAMES+=("$ARCH-gcc" "$ARCH-g++")
    for p in /usr/bin/"$ARCH"-gcc-[0-9]* /usr/bin/"$ARCH"-g++-[0-9]*; do
        [ -e "$p" ] || continue
        NAMES+=("$(basename "$p")")
    done
fi

for name in "${NAMES[@]}"; do
    case "$name" in
        *g++*|*c++*) create_wrapper "$name" "/usr/bin/$CLANGXX_BIN" ;;
        *)           create_wrapper "$name" "/usr/bin/$CLANG_BIN" ;;
    esac
done

# Replace a /usr/bin compiler binary with a symlink to our wrapper.
# Handles both real files (dpkg-divert) and symlinks (rm + ln).
# Appends to REPLACED so the verification pass only checks what exists.
REPLACED=()
replace_compiler() {
    local name="$1"
    if [ ! -e "/usr/bin/$name" ]; then
        return
    fi
    if [ -L "/usr/bin/$name" ]; then
        rm -f "/usr/bin/$name"
    else
        dpkg-divert --local --rename --add "/usr/bin/$name" || {
            echo "REBUILD-WARN: dpkg-divert failed for $name, forcing overwrite" >&2
            rm -f "/usr/bin/$name"
        }
    fi
    ln -sf "$WRAPPER_DIR/$name" "/usr/bin/$name"
    REPLACED+=("$name")
    echo "REBUILD:   Replaced /usr/bin/$name -> $WRAPPER_DIR/$name"
}

for name in "${NAMES[@]}"; do
    replace_compiler "$name"
done

# --- Verification ---
echo ""
echo "=== REBUILD: Verification ==="
echo "REBUILD:   /usr/bin/gcc -> $(readlink -f /usr/bin/gcc 2>/dev/null || echo 'NOT FOUND')"
echo "REBUILD:   /usr/bin/g++ -> $(readlink -f /usr/bin/g++ 2>/dev/null || echo 'NOT FOUND')"
echo "REBUILD:   /usr/bin/cc  -> $(readlink -f /usr/bin/cc 2>/dev/null || echo 'NOT FOUND')"
echo "REBUILD:   wrapper contents:"
cat /usr/local/lib/clang-wrapper/gcc 2>/dev/null || echo "REBUILD:   (could not read wrapper)"
echo "REBUILD:   clang-$CLANG_VERSION direct test: $(/usr/bin/$CLANG_BIN --version 2>&1 | head -1 || echo 'FAILED')"
echo "REBUILD:   ls -la /usr/bin/clang*:"
ls -la /usr/bin/clang* 2>/dev/null || echo "REBUILD:   no clang binaries found"

# Every compiler we replaced must report as clang.  Packages can invoke
# versioned or triple-prefixed names directly, so checking plain gcc alone
# is not enough — an unwrapped binary would produce a silently-mixed
# GCC/Clang build.
if [ "${#REPLACED[@]}" -eq 0 ]; then
    echo "REBUILD-ERROR: FAILED - no gcc-family compiler found to wrap; clang substitution cannot work" >&2
    exit 1
fi

VERIFY_FAILED=""
for name in "${REPLACED[@]}"; do
    out=$("$name" --version 2>&1 | head -1)
    echo "REBUILD:   $name --version: $out"
    if ! echo "$out" | grep -qi clang; then
        VERIFY_FAILED="$VERIFY_FAILED $name"
        echo "REBUILD-ERROR: FAILED - $name is NOT reporting as clang!" >&2
    fi
done

if [ -n "$VERIFY_FAILED" ]; then
    echo "REBUILD-ERROR: FAILED - compiler verification failed:$VERIFY_FAILED" >&2
    echo "REBUILD-ERROR: Build would use GCC, not Clang. Aborting." >&2
    exit 1
fi

echo "REBUILD: SUCCESS - gcc is now clang"
echo "=== REBUILD: Clang $CLANG_VERSION substitution complete ==="
