# Ubuntu Archive Rebuilder

A tool to rebuild packages from the Ubuntu archive with alternative compilers
(initially Clang, but designed to be broadly useful for any compiler evaluation).
It supports adjusting the build environment with global flag overrides — for
example, forcing DWARF4 debug info for toolchains whose output `dwz` cannot
yet process.

For each package in a batch the backend will:

1. Fetch the source from the Ubuntu archive via `pull-lp-source`.
2. Run `sbuild --chroot-mode=unshare` with the profile's compiler and flags.
3. For `clang` profiles, install the target version inside the build environment
   and replace `/usr/bin/gcc` (and `g++`, `cpp`) with a wrapper script that execs
   Clang. A verification step confirms `gcc --version` reports Clang before the
   build starts. This is intentionally direct, because packages can invoke gcc in
   a number of unexpected ways.
4. Scan build logs for known error patterns, recording structured findings.
5. Store results in a local SQLite database.

A static frontend is included for browsing and analysing results.


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

Each `[[flags]]` entry includes a `reason` field to track why the workaround
exists. Profiles are snapshotted into the database at build time, so results
are always tied to the exact configuration used.


## Package list format

One source package name per line; blank lines and `#` comments are ignored.
