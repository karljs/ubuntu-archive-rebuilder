# Ubuntu Archive Rebuilder

A tool to rebuild packages from the Ubuntu archive with the option to
swap out the default `gcc`-based C and C++ toolchain for arbitrary
versions of `clang`.

For each package in a user-defined batch, the builder will:

1. Fetch the source from the Ubuntu archive via `pull-lp-source`.
2. Run `sbuild --chroot-mode=unshare` with the profile's compiler and flags.
3. For `clang` profiles, install the target version inside the build
   environment and replace `/usr/bin/gcc`, `g++`, `cc`, `c++` and all
   versioned or architecture-prefixed variants (`gcc-15`,
   `x86_64-linux-gnu-gcc`, ...) with wrapper scripts that exec `clang`.
   A verification step confirms every wrapped compiler reports `clang`
   before the build starts. This is intentionally brutish, because
   packages can invoke `gcc` in a number of unexpected ways.
4. Scan build logs for known error patterns, recording structured findings.
5. Store results in a local SQLite database.

A static frontend is included for browsing and analyzing results.


## Setup

```bash
sudo apt install sbuild ubuntu-dev-tools
sbuild-adduser $USER   # then log out and back in
```

The backend uses `--chroot-mode=unshare`, so no persistent chroot setup is needed.

## Usage

```bash
cd backend
cargo build --release
BIN=./target/release/rebuilder

# Run a batch
$BIN build --profile ../profiles/clang-18-noble.toml --packages packages-smoke.txt

# Check progress
$BIN status --latest

# Export for the frontend
$BIN export --output-dir ../frontend/data

# Serve the frontend
python3 -m http.server 8000 --directory ../frontend
```

## Profiles

A profile is a TOML file declaring the compiler and any flag overrides:

```toml
[compiler]
type = "clang"   # "clang" or "gcc"
version = "18"

[target]
series = "noble"

[[flags]]
var = "DEB_CFLAGS_APPEND"
flag = "-gdwarf-4"
reason = "Noble's dwz 0.15 doesn't support DWARF5"
```

Each `[[flags]]` entry includes a `reason` field to track why the
workaround exists. Profiles are snapshotted into the database at build
time.

## Static build for remote machines

The "Release musl artifact" workflow (Actions tab, or
`gh workflow run release-musl.yml --ref main`) builds a static musl
`rebuilder` on GitHub's runners and publishes it to the
`artifacts/rebuilder-x86_64-musl` branch (always `main` plus one binary
commit, force-updated per run).

Fetch it on the target machine with a single allowed URL (github.com):

```bash
git fetch origin artifacts/rebuilder-x86_64-musl
git show origin/artifacts/rebuilder-x86_64-musl:backend/target/x86_64-unknown-linux-musl/release/rebuilder > rebuilder
chmod +x rebuilder
```

## Package list format

One source package name per line; blank lines and `#` comments are
ignored. Several small sets are provided, useful for testing.
