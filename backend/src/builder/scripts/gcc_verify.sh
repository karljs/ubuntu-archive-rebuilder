#!/bin/bash
# Records the GCC version for baseline builds via the REBUILD: marker
# protocol. Runs as --starting-build-commands; replaces nothing.
set -e

echo "=== REBUILD: GCC baseline verification ==="
echo "REBUILD:   /usr/bin/gcc -> $(readlink -f /usr/bin/gcc 2>/dev/null || echo 'NOT FOUND')"
echo "REBUILD:   /usr/bin/g++ -> $(readlink -f /usr/bin/g++ 2>/dev/null || echo 'NOT FOUND')"

# A leftover clang wrapper still exits 0; only the output proves identity.
if ! command -v gcc >/dev/null 2>&1; then
    echo "REBUILD-ERROR: FAILED - gcc not found in chroot" >&2
    exit 1
fi

GCC_VERSION_OUTPUT=$(gcc --version 2>&1 | head -1)
echo "REBUILD:   gcc --version: $GCC_VERSION_OUTPUT"

if echo "$GCC_VERSION_OUTPUT" | grep -qi gcc; then
    echo "REBUILD: SUCCESS - gcc confirmed"
else
    echo "REBUILD-ERROR: FAILED - gcc is not reporting as gcc: $GCC_VERSION_OUTPUT" >&2
    exit 1
fi

echo "=== REBUILD: GCC baseline verification complete ==="
