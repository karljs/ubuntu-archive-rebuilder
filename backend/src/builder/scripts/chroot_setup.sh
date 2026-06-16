#!/bin/bash
# Installs the target Clang version inside the sbuild chroot.
#
# Runs as --chroot-setup-commands (BEFORE build-dependency installation).
# Only installs clang here; wrapper setup is deferred to starting_build.sh
# which runs AFTER deps are installed so it can reliably intercept gcc.
#
# Placeholder __CLANG_VERSION__ is replaced at runtime by the pipeline.
set -e

# Prevent dpkg-preconfigure/debconf from trying to open /dev/tty.  The
# pipeline puts sbuild in its own process group (for killpg), which makes it
# a background group on the terminal.  A background read on /dev/tty triggers
# SIGTTIN and stops the entire install.
export DEBIAN_FRONTEND=noninteractive

CLANG_VERSION="__CLANG_VERSION__"
echo "=== REBUILD: Installing Clang $CLANG_VERSION ==="

apt-get update -qq
apt-get install -y -qq "clang-$CLANG_VERSION" || {
    echo "REBUILD-ERROR: Failed to install clang-$CLANG_VERSION" >&2
    exit 1
}

command -v "clang-$CLANG_VERSION" > /dev/null || {
    echo "REBUILD-ERROR: clang-$CLANG_VERSION not found after install" >&2
    exit 1
}

echo "REBUILD: Clang installed: $(clang-"$CLANG_VERSION" --version | head -1)"
