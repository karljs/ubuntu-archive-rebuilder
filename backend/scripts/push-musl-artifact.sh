#!/bin/bash
# Update artifacts/rebuilder-x86_64-musl: HEAD's tree plus one commit that
# adds the freshly built musl binary at
# backend/target/x86_64-unknown-linux-musl/release/rebuilder.
# Called by the "Release musl artifact" workflow; also usable manually
# after a local musl build.
#
# Usage: push-musl-artifact.sh <path-to-rebuilder-binary>
set -euo pipefail

BRANCH=artifacts/rebuilder-x86_64-musl
BIN_PATH=backend/target/x86_64-unknown-linux-musl/release/rebuilder

BIN="${1:?usage: push-musl-artifact.sh <path-to-rebuilder-binary>}"
[ -f "$BIN" ] || { echo "binary not found: $BIN" >&2; exit 1; }
BIN="$(realpath "$BIN")"

ROOT="$(git rev-parse --show-toplevel)"
cd "$ROOT"

[ -z "$(git status --porcelain)" ] || { echo "working tree not clean" >&2; exit 1; }

# commit-tree needs an identity; keep an existing one.
git config user.name  >/dev/null 2>&1 || git config user.name  "github-actions[bot]"
git config user.email >/dev/null 2>&1 || git config user.email "41898282+github-actions[bot]@users.noreply.github.com"

# Plumbing only: no checkout, no worktree. Checking the branch out first
# would write the old binary over the new build.
GIT_INDEX_FILE="$(mktemp)"
export GIT_INDEX_FILE
trap 'rm -f "$GIT_INDEX_FILE"' EXIT
git read-tree HEAD
blob="$(git hash-object -w "$BIN")"
git update-index --add --cacheinfo 100755,"$blob","$BIN_PATH"
tree="$(git write-tree)"
commit="$(git commit-tree "$tree" -p HEAD -m "musl build of $(git rev-parse --short HEAD)")"

# ls-remote instead of fetch: the old 10MB binary never crosses the network.
expect="$(git ls-remote origin "refs/heads/$BRANCH" | cut -f1)"
if [ -n "$expect" ]; then
    git push --force-with-lease="refs/heads/$BRANCH:$expect" origin "$commit:refs/heads/$BRANCH"
else
    git push origin "$commit:refs/heads/$BRANCH"
fi

echo ""
echo "Updated $BRANCH -> $commit ($(git rev-parse --short HEAD) + binary)"
echo "Fetch on the target machine:"
echo "  git fetch origin $BRANCH"
echo "  git show origin/$BRANCH:$BIN_PATH > rebuilder && chmod +x rebuilder"
