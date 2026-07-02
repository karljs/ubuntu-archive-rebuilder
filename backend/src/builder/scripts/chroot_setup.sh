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

# Inject an HTTP(S) proxy into apt's configuration if the pipeline supplied
# one via REBUILD_HTTP_PROXY.  sbuild's unshare chroot does not inherit the
# outer shell's http_proxy / https_proxy env vars, so without this apt-get
# update / install inside the chroot cannot reach the archive on hosts that
# require a proxy.  An empty value leaves apt's default config untouched.
if [ -n "__HTTP_PROXY__" ]; then
    echo "Acquire::http::Proxy  \"__HTTP_PROXY__\";"  >  /etc/apt/apt.conf.d/99proxy
    echo "Acquire::https::Proxy \"__HTTP_PROXY__\";"  >> /etc/apt/apt.conf.d/99proxy
    echo "REBUILD: apt proxy configured via /etc/apt/apt.conf.d/99proxy"
fi

CLANG_VERSION="__CLANG_VERSION__"
echo "=== REBUILD: Installing Clang $CLANG_VERSION ==="

apt-get update -qq || {
    echo "REBUILD-ERROR: apt-get update failed (check proxy / archive reachability)" >&2
    exit 1
}
apt-get install -y -qq "clang-$CLANG_VERSION" || {
    echo "REBUILD-ERROR: Failed to install clang-$CLANG_VERSION (check proxy / archive reachability)" >&2
    exit 1
}

command -v "clang-$CLANG_VERSION" > /dev/null || {
    echo "REBUILD-ERROR: clang-$CLANG_VERSION not found after install" >&2
    exit 1
}

echo "REBUILD: Clang installed: $(clang-"$CLANG_VERSION" --version | head -1)"
