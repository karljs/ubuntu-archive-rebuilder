#!/bin/bash
# Replaces gcc/g++/cc/c++ (and versioned or triple-prefixed variants) with
# clang wrappers, as --starting-build-commands (after build-deps, before
# dpkg-buildpackage). __CLANG_VERSION__ is substituted at runtime.
#
# sbuild mangles external command strings: bare percent sequences are
# expanded (double them for a literal), and array references are flattened
# to nothing. This script uses neither.
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

# Every gcc/g++ name present in the chroot, one per line: plain, versioned
# (gcc-15, ...), and target-triple-prefixed. Globs require a digit after
# the dash so binutils front-ends (gcc-ar, gcc-nm, gcc-ranlib) are not
# wrapped; .distrib names are the dpkg-diverted originals and are skipped.
# A hardcoded version list would miss gcc-15+ and silently compile with
# real GCC inside a Clang batch.
all_names() {
    echo "gcc g++ cc c++"
    for p in /usr/bin/gcc-[0-9]* /usr/bin/g++-[0-9]*; do
        [ -e "$p" ] || continue
        case "$p" in *.distrib) continue ;; esac
        basename "$p"
    done
    ARCH=$(dpkg-architecture -qDEB_HOST_GNU_TYPE 2>/dev/null || echo "")
    if [ -n "$ARCH" ]; then
        echo "$ARCH-gcc $ARCH-g++"
        for p in /usr/bin/"$ARCH"-gcc-[0-9]* /usr/bin/"$ARCH"-g++-[0-9]*; do
            [ -e "$p" ] || continue
            case "$p" in *.distrib) continue ;; esac
            basename "$p"
        done
    fi
}

# The doubled percent in the format string reaches the shell as one.
# shellcheck disable=SC2182
create_wrapper() {
    local name="$1"
    local target="$2"
    printf '#!/bin/sh\nexec %%s "$@"\n' "$target" > "$WRAPPER_DIR/$name"
    chmod +x "$WRAPPER_DIR/$name"
    echo "REBUILD:   Created wrapper: $name -> $target"
}

# Symlinks: rm plus ln. Real files: dpkg-divert.
replace_compiler() {
    local name="$1"
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

ANY_REPLACED=0
for name in $(all_names); do
    [ -e "/usr/bin/$name" ] || continue
    case "$name" in
        *g++*|*c++*) create_wrapper "$name" "/usr/bin/$CLANGXX_BIN" ;;
        *)           create_wrapper "$name" "/usr/bin/$CLANG_BIN" ;;
    esac
    replace_compiler "$name"
    ANY_REPLACED=1
done

if [ "$ANY_REPLACED" -eq 0 ]; then
    echo "REBUILD-ERROR: FAILED - no gcc-family compiler found to wrap; clang substitution cannot work" >&2
    exit 1
fi

# Packages can invoke versioned or triple-prefixed names directly; every
# replaced compiler must report as clang.
echo ""
echo "=== REBUILD: Verification ==="
echo "REBUILD:   wrapper contents:"
cat /usr/local/lib/clang-wrapper/gcc 2>/dev/null || echo "REBUILD:   (could not read wrapper)"
echo "REBUILD:   clang-$CLANG_VERSION direct test: $(/usr/bin/$CLANG_BIN --version 2>&1 | head -1 || echo 'FAILED')"

VERIFY_FAILED=""
for name in $(all_names); do
    [ -e "/usr/bin/$name" ] || continue
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
