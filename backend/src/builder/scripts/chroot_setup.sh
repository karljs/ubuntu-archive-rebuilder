#!/bin/bash
# Installs the target Clang version in the sbuild chroot, as
# --chroot-setup-commands (before build-deps). __CLANG_VERSION__ is
# substituted at runtime.
set -e

# The pipeline's setpgid makes us a background group; a /dev/tty read
# would SIGTTIN and stop the install.
export DEBIAN_FRONTEND=noninteractive

# The unshare chroot doesn't inherit proxy env vars; REBUILD_HTTP_PROXY
# forwards one into apt's config. SC2157: placeholder, substituted before
# the shell sees it.
# shellcheck disable=SC2157
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
