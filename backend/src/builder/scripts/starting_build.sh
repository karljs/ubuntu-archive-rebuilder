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
echo "REBUILD:   gcc --version: $(gcc --version 2>/dev/null | head -1 || echo 'NOT FOUND')"

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

create_wrapper gcc  "/usr/bin/$CLANG_BIN"
create_wrapper g++  "/usr/bin/$CLANGXX_BIN"
create_wrapper cc   "/usr/bin/$CLANG_BIN"
create_wrapper c++  "/usr/bin/$CLANGXX_BIN"

for v in 9 10 11 12 13 14; do
    [ -e "/usr/bin/gcc-$v" ] && create_wrapper "gcc-$v" "/usr/bin/$CLANG_BIN"
    [ -e "/usr/bin/g++-$v" ] && create_wrapper "g++-$v" "/usr/bin/$CLANGXX_BIN"
done

ARCH=$(dpkg-architecture -qDEB_HOST_GNU_TYPE 2>/dev/null || echo "")
if [ -n "$ARCH" ]; then
    create_wrapper "$ARCH-gcc" "/usr/bin/$CLANG_BIN"
    create_wrapper "$ARCH-g++" "/usr/bin/$CLANGXX_BIN"
fi

# Replace a /usr/bin compiler binary with a symlink to our wrapper.
# Handles both real files (dpkg-divert) and symlinks (rm + ln).
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
    echo "REBUILD:   Replaced /usr/bin/$name -> $WRAPPER_DIR/$name"
}

for name in gcc g++ cc c++; do
    replace_compiler "$name"
done

for v in 9 10 11 12 13 14; do
    replace_compiler "gcc-$v"
    replace_compiler "g++-$v"
done

if [ -n "$ARCH" ]; then
    replace_compiler "$ARCH-gcc"
    replace_compiler "$ARCH-g++"
fi

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

GCC_VERSION_OUTPUT=$(gcc --version 2>&1 | head -1)
echo "REBUILD:   gcc --version: $GCC_VERSION_OUTPUT"

if echo "$GCC_VERSION_OUTPUT" | grep -qi clang; then
    echo "REBUILD: SUCCESS - gcc is now clang"
else
    echo "REBUILD-ERROR: FAILED - gcc is NOT reporting as clang!" >&2
    echo "REBUILD-ERROR: gcc resolves to: $(which gcc) -> $(readlink -f "$(which gcc)")" >&2
    echo "REBUILD-ERROR: Build will use GCC, not Clang. Aborting." >&2
    exit 1
fi

echo "=== REBUILD: Clang $CLANG_VERSION substitution complete ==="
