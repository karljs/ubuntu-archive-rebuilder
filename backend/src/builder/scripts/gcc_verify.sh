#!/bin/bash
# Records the GCC version for baseline builds.
#
# Runs as --starting-build-commands (AFTER build-dependency installation,
# BEFORE dpkg-buildpackage).  Unlike the clang wrapper script, this does
# not replace anything — it just logs the compiler version using the same
# REBUILD: marker protocol so compiler detection works consistently.
set -e

echo "=== REBUILD: GCC baseline verification ==="
echo "REBUILD:   /usr/bin/gcc -> $(readlink -f /usr/bin/gcc 2>/dev/null || echo 'NOT FOUND')"
echo "REBUILD:   /usr/bin/g++ -> $(readlink -f /usr/bin/g++ 2>/dev/null || echo 'NOT FOUND')"

if ! command -v gcc >/dev/null 2>&1; then
    echo "REBUILD-ERROR: FAILED - gcc not found in chroot" >&2
    exit 1
fi

GCC_VERSION_OUTPUT=$(gcc --version 2>&1 | head -1)
echo "REBUILD:   gcc --version: $GCC_VERSION_OUTPUT"

# grep on the output is the discriminator: a leftover clang wrapper (possible
# in persistent schroot chroots) still exits 0, so only the output text can
# prove gcc is really gcc.
if echo "$GCC_VERSION_OUTPUT" | grep -qi gcc; then
    echo "REBUILD: SUCCESS - gcc confirmed"
else
    echo "REBUILD-ERROR: FAILED - gcc is not reporting as gcc: $GCC_VERSION_OUTPUT" >&2
    exit 1
fi

echo "=== REBUILD: GCC baseline verification complete ==="
