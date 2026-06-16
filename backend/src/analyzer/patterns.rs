//! Error pattern definitions for Clang build failures and observations.
//!
//! Patterns are grouped by semantic error class rather than by how they might
//! be addressed.  Each group covers a distinct failure mechanism at the
//! compiler, linker, or build-system level.
//!
//! Two pattern sets exist:
//!
//! - [`ERROR_PATTERNS`] — matched on failed builds.  Every entry represents
//!   something that (contributed to) the build failure.
//!
//! - [`OBSERVATION_PATTERNS`] — matched on succeeded builds.  The build
//!   completed, but the log contains something worth noting for toolchain
//!   analysis (e.g. compiler flags that were silently ignored).

/// An error or observation pattern.
pub struct ErrorPattern {
    /// Unique key identifying this category.
    pub key: &'static str,
    /// Short human-readable description shown in the viewer.
    pub description: &'static str,
    /// Substrings to search for in a log line.  Any match triggers the pattern.
    pub patterns: &'static [&'static str],
    /// If set, the log line must *also* contain this prefix to match.
    /// Used to reduce false positives (e.g. require `"fatal error:"` for
    /// missing-header patterns so that incidental "No such file" messages
    /// from build-system cleanup don't fire).
    pub require_prefix: Option<&'static str>,
    /// If set, the log line must *not* contain any of these strings.
    /// Used to exclude configure-probe lines, cleanup commands, etc.
    pub exclude_if_contains: &'static [&'static str],
    /// When true, the specific identifier is extracted from the matching line
    /// and used as part of the deduplication key, so that multiple distinct
    /// symbols / flags / headers each produce a separate finding (up to the
    /// per-category cap in scan_log).
    pub dedup_by_extracted_key: bool,
}

// ---------------------------------------------------------------------------
// ── Group 1: GNU C / C++ Extensions ──────────────────────────────────────
//
// Code that relies on GCC-specific language extensions Clang does not
// implement.  These are per-package source issues; there is no global
// compiler flag that suppresses them correctly.
//
// Observed packages: bogl (NESTED_FUNCTIONS), ppp (NESTED_FUNCTIONS via
// eap-tls.c), and various packages with GNU asm extensions.
// ---------------------------------------------------------------------------

/// GCC extension: nested function definitions inside other functions.
/// bogl, ppp, and others use this.
const GNU_NESTED_FUNCTIONS: ErrorPattern = ErrorPattern {
    key: "GNU_NESTED_FUNCTIONS",
    description: "Nested function definition (GNU extension not supported by Clang)",
    patterns: &["function definition is not allowed here"],
    require_prefix: None,
    exclude_if_contains: &[],
    dedup_by_extracted_key: false,
};

/// GCC extension: variable-length arrays as struct members.
const GNU_VLA_IN_STRUCT: ErrorPattern = ErrorPattern {
    key: "GNU_VLA_IN_STRUCT",
    description: "Variable-length array in struct (GNU extension not supported by Clang)",
    patterns: &["variable length array in structure"],
    require_prefix: None,
    exclude_if_contains: &[],
    dedup_by_extracted_key: false,
};

/// GCC extension: hardware register assignment via `register` keyword.
const GNU_GLOBAL_REGISTER_VAR: ErrorPattern = ErrorPattern {
    key: "GNU_GLOBAL_REGISTER_VAR",
    description: "Global register variable (GNU extension not supported by Clang)",
    patterns: &["global register variables are not supported"],
    require_prefix: None,
    exclude_if_contains: &[],
    dedup_by_extracted_key: false,
};

/// GCC extension: `asm goto` — jumps from inline assembly to C labels.
const GNU_ASM_GOTO: ErrorPattern = ErrorPattern {
    key: "GNU_ASM_GOTO",
    description: "asm goto construct (unsupported or differently handled by Clang)",
    patterns: &["'asm goto' constructs are not supported"],
    require_prefix: None,
    exclude_if_contains: &[],
    dedup_by_extracted_key: false,
};

/// Inline assembly syntax incompatibilities not covered by ASM_GOTO.
/// Includes 16-bit mode directives and invalid instruction mnemonics.
const GNU_ASM_SYNTAX: ErrorPattern = ErrorPattern {
    key: "GNU_ASM_SYNTAX",
    description: "Inline assembly syntax not accepted by Clang's integrated assembler",
    patterns: &[
        "invalid instruction mnemonic",
        ".code16 not supported",
        "invalid operand",
    ],
    require_prefix: None,
    exclude_if_contains: &[],
    dedup_by_extracted_key: false,
};

// ---------------------------------------------------------------------------
// ── Group 2: Implicit Declarations ──────────────────────────────────────
//
// Code that relies on GCC's historically permissive C behaviour — calling
// undeclared functions or omitting return types — which C99 forbids and
// Clang rejects as errors.
//
// Per-package source issues; no global workaround is appropriate.
// ---------------------------------------------------------------------------

/// Calling a function that has not been declared.
/// Clang rejects implicit function declarations in C99 mode.
/// Observed: libxkbcommon, ppp, integrit.
const IMPLICIT_FUNCTION_DECLARATION: ErrorPattern = ErrorPattern {
    key: "IMPLICIT_FUNCTION_DECLARATION",
    description: "Implicit function declaration (not permitted in C99; Clang rejects)",
    patterns: &[
        "implicit declaration of function",
        "Wimplicit-function-declaration",
    ],
    require_prefix: None,
    exclude_if_contains: &[
        // Exclude configure-probe lines that test for this flag — they are not failures.
        "checking whether",
        "supports compile flag",
        "compiler handles",
    ],
    dedup_by_extracted_key: true, // key = function name
};

/// Use of an identifier that has not been declared in scope.
/// Often a consequence of a missing #include or implicit declaration.
/// Observed: libxkbcommon (fmt), ppp (writer).
const UNDECLARED_IDENTIFIER: ErrorPattern = ErrorPattern {
    key: "UNDECLARED_IDENTIFIER",
    description: "Use of undeclared identifier",
    patterns: &["use of undeclared identifier"],
    require_prefix: None,
    exclude_if_contains: &["use of undeclared identifier '__builtin_"],
    dedup_by_extracted_key: true, // key = identifier name
};

/// Use of an undeclared GCC builtin.
/// Separate from UNDECLARED_IDENTIFIER because the fix is different
/// (add -fno-builtin or provide a declaration) and the pattern is specific.
const MISSING_BUILTIN: ErrorPattern = ErrorPattern {
    key: "MISSING_BUILTIN",
    description: "GCC built-in function not available in Clang",
    patterns: &["use of undeclared identifier '__builtin_"],
    require_prefix: None,
    exclude_if_contains: &[],
    dedup_by_extracted_key: true, // key = builtin name
};

// ---------------------------------------------------------------------------
// ── Group 3: C++ Type Strictness ─────────────────────────────────────────
//
// Errors arising from C++ code that GCC accepts under looser rules but
// Clang enforces strictly per the standard.
//
// Per-package source issues.
// ---------------------------------------------------------------------------

/// C++11 narrowing conversions in initializer lists.
/// Clang treats these as errors; GCC allows them with a warning.
/// Observed: clucene-core (constant narrowing), nullmailer (size_t → unsigned).
const CXX11_NARROWING: ErrorPattern = ErrorPattern {
    key: "CXX11_NARROWING",
    description: "C++11 narrowing conversion in initializer list",
    patterns: &["Wc++11-narrowing", "cannot be narrowed"],
    require_prefix: None,
    exclude_if_contains: &[],
    dedup_by_extracted_key: false,
};

/// No matching function or constructor for a call.
const CXX_NO_MATCHING_FUNCTION: ErrorPattern = ErrorPattern {
    key: "CXX_NO_MATCHING_FUNCTION",
    description: "No matching function or constructor for call",
    patterns: &[
        "no matching function for call",
        "no matching member function for call",
        "no matching constructor",
    ],
    require_prefix: None,
    exclude_if_contains: &[],
    dedup_by_extracted_key: false,
};

/// Access to private or protected class members.
const CXX_ACCESS_VIOLATION: ErrorPattern = ErrorPattern {
    key: "CXX_ACCESS_VIOLATION",
    description: "Access to private or protected class member",
    patterns: &["is a private member of", "is a protected member of"],
    require_prefix: None,
    exclude_if_contains: &[],
    dedup_by_extracted_key: false,
};

/// Code requires C++11 or later but is not being compiled with it.
const CXX_STD_REQUIREMENT: ErrorPattern = ErrorPattern {
    key: "CXX_STD_REQUIREMENT",
    description: "Feature requires C++11 or later; compile with -std=c++11",
    patterns: &[
        "enabled with the -std=c++11",
        "enabled with the -std=gnu++11",
    ],
    require_prefix: None,
    exclude_if_contains: &[],
    dedup_by_extracted_key: false,
};

/// Implicit instantiation of an undefined template.
const CXX_IMPLICIT_INSTANTIATION: ErrorPattern = ErrorPattern {
    key: "CXX_IMPLICIT_INSTANTIATION",
    description: "Implicit instantiation of undefined template",
    patterns: &["implicit instantiation of undefined template"],
    require_prefix: None,
    exclude_if_contains: &[],
    dedup_by_extracted_key: false,
};

/// Type redefinition or conflicting type declarations.
const TYPE_REDEFINITION: ErrorPattern = ErrorPattern {
    key: "TYPE_REDEFINITION",
    description: "Redefinition or conflicting type declarations",
    patterns: &[
        "redefinition of",
        "macro redefined",
        "error: conflicting types for",
    ],
    require_prefix: None,
    exclude_if_contains: &[],
    dedup_by_extracted_key: false,
};

/// Unknown type name — often caused by a missing typedef or include.
const UNKNOWN_TYPE_NAME: ErrorPattern = ErrorPattern {
    key: "UNKNOWN_TYPE_NAME",
    description: "Unknown type name; possibly missing typedef or #include",
    patterns: &["unknown type name"],
    require_prefix: None,
    exclude_if_contains: &[],
    dedup_by_extracted_key: true,
};

// ---------------------------------------------------------------------------
// ── Group 4: Linker Failures ──────────────────────────────────────────────
//
// Failures at the link stage.  Clang's stricter link semantics compared to
// GCC (e.g. requiring explicit -lm, not auto-linking DSOs) expose latent
// issues in packages that relied on GCC's implicit behaviour.
//
// LINK_MISSING_SYMBOL is deduplicated per symbol name so that each distinct
// undefined reference produces a separate finding (up to the cap).
//
// Observed root causes grouped here:
//   - rpl_calloc (barcode): gnulib replacement-alloc issue with Clang LTO
//   - sqrt (gettext): missing -lm, exposed by Clang's strict DSO linking
//   - bcmp (integrit): obsolete POSIX function not in Clang libc interface
//   - _Block_object_assign (chipmunk): Blocks runtime not linked
// ---------------------------------------------------------------------------

/// Undefined symbol at link time.  The most informative linker finding —
/// deduplicated per symbol so distinct undefined references are visible.
const LINK_MISSING_SYMBOL: ErrorPattern = ErrorPattern {
    key: "LINK_MISSING_SYMBOL",
    description: "Undefined symbol at link time",
    patterns: &["undefined reference to"],
    require_prefix: None,
    // Exclude lines that are clearly the preceding context (command lines),
    // not the actual linker error.
    exclude_if_contains: &["libtool:", "gcc -", "g++ -", "clang-", "clang "],
    dedup_by_extracted_key: true, // key = symbol name
};

/// Apple Blocks runtime symbols missing.  Packages using Clang Blocks
/// extension (chipmunk) need -lBlocksRuntime at link time.
const BLOCKS_RUNTIME_MISSING: ErrorPattern = ErrorPattern {
    key: "BLOCKS_RUNTIME_MISSING",
    description: "Apple Blocks runtime symbols not found; package needs -lBlocksRuntime",
    patterns: &["_Block_object_assign", "_Block_object_dispose"],
    require_prefix: None,
    exclude_if_contains: &[],
    dedup_by_extracted_key: false,
};

/// Multiple definition of a symbol — usually a header included in multiple TUs
/// without an include guard, exposed by Clang's stricter one-definition rule.
const LINK_MULTIPLE_DEFINITION: ErrorPattern = ErrorPattern {
    key: "LINK_MULTIPLE_DEFINITION",
    description: "Multiple definition of symbol at link time",
    patterns: &["multiple definition of"],
    require_prefix: None,
    exclude_if_contains: &[],
    dedup_by_extracted_key: true,
};

/// Required library not found during linking.
const LINK_MISSING_LIBRARY: ErrorPattern = ErrorPattern {
    key: "LINK_MISSING_LIBRARY",
    description: "Required library not found during linking",
    patterns: &["cannot find -l"],
    require_prefix: None,
    exclude_if_contains: &[],
    dedup_by_extracted_key: true, // key = library name
};

/// Generic linker command failure not covered by a more specific category.
/// Intentionally placed last in the error list so more specific patterns
/// match first.
const LINK_FAILURE: ErrorPattern = ErrorPattern {
    key: "LINK_FAILURE",
    description: "Linker command failed (see other findings for specific cause)",
    patterns: &[
        "linker command failed",
        "collect2: error: ld",
        "ld returned 1 exit status",
    ],
    require_prefix: None,
    exclude_if_contains: &[],
    dedup_by_extracted_key: false,
};

// ---------------------------------------------------------------------------
// ── Group 5: LTO / Debug-Info Interaction ────────────────────────────────
//
// Failures at the intersection of LTO (Link-Time Optimisation) and debug
// format.  Clang produces DWARF5 by default; older versions of `dwz` (the
// Ubuntu debug-info compressor) can't process DWARF5 LTO objects.
//
// This is the root cause that the -gdwarf-4 profile flag addresses.
//
// Observed packages: chipmunk, libdrm (DWARF error during LTO link).
// ---------------------------------------------------------------------------

/// Linker or dwz fails to process DWARF5 output from Clang LTO objects.
/// The -gdwarf-4 flag in the profile addresses this at the whole-archive level.
///
/// Two distinct failure modes both caught here:
///   1. LTO link-time: "DWARF error: invalid or unhandled FORM value: 0x23"
///      (seen in chipmunk, libdrm with LTO-compiled objects)
///   2. Post-build dwz packaging: "Unknown debugging section .debug_addr"
///      (seen in hello, bc, patch etc. on resolute/noble without -gdwarf-4)
///      dwz 0.15/0.16 cannot process Clang's DWARF5 .debug_addr section.
const LTO_DWARF_MISMATCH: ErrorPattern = ErrorPattern {
    key: "LTO_DWARF_MISMATCH",
    description: "DWARF5 format incompatibility; dwz cannot process Clang output — use -gdwarf-4 profile",
    patterns: &[
        "DWARF error: invalid or unhandled FORM value",
        "DWARF error: can't find",
        "DWARF error: offset",
        // dwz post-build packaging failure: Clang's DWARF5 .debug_addr
        // section is not understood by dwz 0.15/0.16 on noble/resolute.
        // This is the primary failure mode for standard (non-dwarf4) profiles.
        "Unknown debugging section .debug_addr",
        "dh_dwz: error: dwz",
    ],
    require_prefix: None,
    exclude_if_contains: &[],
    dedup_by_extracted_key: false,
};

// ---------------------------------------------------------------------------
// ── Group 6: Unsupported / Unknown Compiler Flags ────────────────────────
//
// GCC-specific flags passed via package CFLAGS or Ubuntu's hardening
// infrastructure that Clang does not accept or does not implement.
//
// UNSUPPORTED_COMPILER_FLAG fires on failed builds only; the observation
// equivalent for succeeded builds is LTO_FAT_OBJECTS_IGNORED and
// UNKNOWN_WARNING_FLAG in the observation set below.
// ---------------------------------------------------------------------------

/// A compiler flag (non-warning) that Clang does not support.
/// Observed: libffi (-print-multi-os-directory), ppp (--print-sysroot).
/// Excludes configure probe lines where the failure is expected and handled.
const UNSUPPORTED_COMPILER_FLAG: ErrorPattern = ErrorPattern {
    key: "UNSUPPORTED_COMPILER_FLAG",
    description: "Compiler flag not supported by Clang",
    patterns: &[
        "unsupported option",
        "unknown argument:",
        "error: unsupported argument",
        "the clang compiler does not support",
    ],
    require_prefix: None,
    // These patterns appear in configure probe output where the error is
    // intentional — autoconf tests flag support by trying it and checking
    // the exit code.
    exclude_if_contains: &["conftest", "ac_ext", "checking for", "checking whether"],
    dedup_by_extracted_key: true, // key = flag name
};

// ---------------------------------------------------------------------------
// ── Group 7: Warnings Promoted to Errors (-Werror) ───────────────────────
//
// Packages (or Ubuntu's build infrastructure) that pass -Werror promote
// warnings to hard errors.  Clang sometimes produces warnings where GCC
// would not, or produces them in slightly different forms.
// ---------------------------------------------------------------------------

/// Format-string warning promoted to error via -Werror,-Wformat-security.
/// Observed: cairo.
const WERROR_FORMAT_STRING: ErrorPattern = ErrorPattern {
    key: "WERROR_FORMAT_STRING",
    description: "Format-string warning promoted to error via -Werror",
    patterns: &[
        "format string is not a string literal",
        "-Werror,-Wformat",
        "format string discouraged",
    ],
    require_prefix: None,
    exclude_if_contains: &[],
    dedup_by_extracted_key: false,
};

/// Unused-variable/parameter/function warning promoted to error.
/// Observed: file, openssh, time, usbutils, util-linux (via -Wunused-*).
/// Excludes configure probe lines where the compiler tests flag support.
const WERROR_UNUSED: ErrorPattern = ErrorPattern {
    key: "WERROR_UNUSED",
    description: "Unused variable/parameter/function warning promoted to error via -Werror",
    patterns: &[
        "-Werror,-Wunused",
        "error: unused",
    ],
    require_prefix: None,
    exclude_if_contains: &["checking whether", "supports compile flag", "compiler handles"],
    dedup_by_extracted_key: false,
};

/// Any other warning promoted to error via -Werror not covered above.
const WERROR_OTHER: ErrorPattern = ErrorPattern {
    key: "WERROR_OTHER",
    description: "Warning promoted to error via -Werror",
    patterns: &["-Werror,-W", "error: -Werror"],
    require_prefix: None,
    exclude_if_contains: &[
        // Exclude the format and unused subcategories already captured above.
        "-Werror,-Wformat",
        "-Werror,-Wunused",
    ],
    dedup_by_extracted_key: false,
};

// ---------------------------------------------------------------------------
// ── Group 8: Build System / Configure Failures ───────────────────────────
//
// The build system itself fails to recognise or work with Clang, preventing
// the build from starting.
// ---------------------------------------------------------------------------

/// Configure script cannot create executables with Clang.
/// Observed: readline, recode — configure's compiler sanity check fails.
const CONFIGURE_COMPILER_TEST_FAILED: ErrorPattern = ErrorPattern {
    key: "CONFIGURE_COMPILER_TEST_FAILED",
    description: "Configure script cannot compile a test program with this compiler",
    patterns: &[
        "compiler cannot create executables",
        "C compiler cannot create executables",
        "Can't run the compiler",
    ],
    require_prefix: None,
    exclude_if_contains: &[],
    dedup_by_extracted_key: false,
};

/// Build system (autoconf, cmake, etc.) explicitly rejects Clang or requires GCC.
const BUILD_SYSTEM_MISDETECTS_COMPILER: ErrorPattern = ErrorPattern {
    key: "BUILD_SYSTEM_MISDETECTS_COMPILER",
    description: "Build system requires GCC or does not recognise Clang",
    patterns: &[
        "g++ was not found",
        "gcc >= 3.0 is needed",
        "could not configure a C compiler",
        "GCC too old",
        "Gcc version error",
        "clang: not found",
        "clang++: not found",
    ],
    require_prefix: None,
    exclude_if_contains: &[],
    dedup_by_extracted_key: false,
};

// ---------------------------------------------------------------------------
// ── Group 9: Missing Build Dependencies ──────────────────────────────────
// ---------------------------------------------------------------------------

/// Required header file not found.
/// Tightly scoped to `fatal error:` lines to avoid matching incidental
/// "No such file or directory" messages from build-system cleanup commands.
/// Observed: ncurses (aclocal missing header), flex (scan.l missing — build
/// system issue, not a real header), gettext (Java SSL cert paths).
/// The require_prefix + exclude_if_contains combination eliminates the
/// false positives seen in the current data.
const MISSING_HEADER: ErrorPattern = ErrorPattern {
    key: "MISSING_HEADER",
    description: "Required header file not found",
    patterns: &["file not found", "No such file or directory"],
    require_prefix: Some("fatal error:"),
    exclude_if_contains: &[
        // autoconf probe headers are intentionally absent
        "ac_nonexistent.h",
        "conftest",
    ],
    dedup_by_extracted_key: true, // key = header name
};

/// OpenMP not available — Clang requires explicit -fopenmp and the libomp
/// package; it is not installed by default in the build chroot.
const MISSING_OPENMP: ErrorPattern = ErrorPattern {
    key: "MISSING_OPENMP",
    description: "OpenMP not available; Clang requires explicit -fopenmp and libomp",
    patterns: &[
        "'omp.h' file not found",
        "We need OpenMP",
        "know how to enable OpenMP",
    ],
    require_prefix: None,
    exclude_if_contains: &[],
    dedup_by_extracted_key: false,
};

// ---------------------------------------------------------------------------
// ── Group 10: Build Infrastructure ───────────────────────────────────────
// ---------------------------------------------------------------------------

/// The build process was killed because it exceeded the time limit.
/// (Also detected by infer_status; recorded as a finding for completeness.)
const BUILD_TIMEOUT: ErrorPattern = ErrorPattern {
    key: "BUILD_TIMEOUT",
    description: "Build killed because it exceeded the time limit",
    patterns: &["Build killed with signal", "Timed out"],
    require_prefix: None,
    exclude_if_contains: &[],
    dedup_by_extracted_key: false,
};

/// The compiler itself segfaulted — a Clang bug, not a package issue.
const SEGFAULT_IN_COMPILER: ErrorPattern = ErrorPattern {
    key: "SEGFAULT_IN_COMPILER",
    description: "Compiler process crashed (segmentation fault — likely a Clang bug)",
    patterns: &[
        "Segmentation fault (core dumped)",
        "LLVM ERROR: ",
        "clang: error: unable to execute command: Segmentation fault",
    ],
    require_prefix: None,
    exclude_if_contains: &[],
    dedup_by_extracted_key: false,
};

/// The build process ran out of memory.
const OUT_OF_MEMORY: ErrorPattern = ErrorPattern {
    key: "OUT_OF_MEMORY",
    description: "Build process ran out of memory",
    patterns: &["Cannot allocate memory", "out of memory", "memory exhausted"],
    require_prefix: None,
    exclude_if_contains: &[],
    dedup_by_extracted_key: false,
};

/// Library symbol changes detected by dpkg-gensymbols.
/// Not strictly a compiler error, but it indicates ABI differences between
/// Clang and GCC builds that affect packaging.
const SYMBOL_ABI_CHANGE: ErrorPattern = ErrorPattern {
    key: "SYMBOL_ABI_CHANGE",
    description: "Library symbol changes detected by dpkg-gensymbols",
    patterns: &[
        "dh_makeshlibs: dpkg-gensymbols",
        "some new symbols appeared",
        "some symbols or patterns disappeared",
    ],
    require_prefix: None,
    exclude_if_contains: &[],
    dedup_by_extracted_key: false,
};

// ===========================================================================
// Ordered error pattern list — more specific patterns first.
// The scanner stops at the first pattern that matches a line.
// ===========================================================================

pub static ERROR_PATTERNS: &[&ErrorPattern] = &[
    // LTO/DWARF — very specific, check early
    &LTO_DWARF_MISMATCH,
    // Blocks runtime — subset of LINK_MISSING_SYMBOL, must come first
    &BLOCKS_RUNTIME_MISSING,
    // GNU extensions
    &GNU_NESTED_FUNCTIONS,
    &GNU_VLA_IN_STRUCT,
    &GNU_GLOBAL_REGISTER_VAR,
    &GNU_ASM_GOTO,
    &GNU_ASM_SYNTAX,
    // Implicit declarations
    &IMPLICIT_FUNCTION_DECLARATION,
    &MISSING_BUILTIN,          // before UNDECLARED_IDENTIFIER (more specific)
    &UNDECLARED_IDENTIFIER,
    // C++ type strictness
    &CXX11_NARROWING,
    &CXX_NO_MATCHING_FUNCTION,
    &CXX_ACCESS_VIOLATION,
    &CXX_STD_REQUIREMENT,
    &CXX_IMPLICIT_INSTANTIATION,
    &TYPE_REDEFINITION,
    &UNKNOWN_TYPE_NAME,
    // Linker — specific causes before generic failure
    &LINK_MISSING_SYMBOL,
    &LINK_MULTIPLE_DEFINITION,
    &LINK_MISSING_LIBRARY,
    &LINK_FAILURE,             // catch-all, last among linker patterns
    // Compiler flags
    &UNSUPPORTED_COMPILER_FLAG,
    // -Werror promotions
    &WERROR_FORMAT_STRING,
    &WERROR_UNUSED,
    &WERROR_OTHER,
    // Build system
    &CONFIGURE_COMPILER_TEST_FAILED,
    &BUILD_SYSTEM_MISDETECTS_COMPILER,
    // Missing deps — MISSING_OPENMP before MISSING_HEADER (more specific)
    &MISSING_OPENMP,
    &MISSING_HEADER,
    // Infrastructure
    &BUILD_TIMEOUT,
    &SEGFAULT_IN_COMPILER,
    &OUT_OF_MEMORY,
    &SYMBOL_ABI_CHANGE,
];

// ===========================================================================
// Observation patterns — matched on *succeeded* builds only.
//
// These represent non-fatal issues that are worth noting for toolchain
// analysis.  They do not indicate build failure, but they may indicate
// configuration mismatches or future compatibility concerns.
// ===========================================================================

/// Ubuntu's build infrastructure injects -ffat-lto-objects into CFLAGS for
/// all packages.  Clang does not implement fat LTO objects (it has a different
/// LTO model) and silently ignores this flag with a warning.  Packages still
/// build successfully, but the flag is not having its intended effect.
///
/// Observed on 68 packages across all series/profiles in current data.
/// The appropriate response is a global profile flag to suppress the warning,
/// or an Ubuntu infrastructure change to strip -ffat-lto-objects when building
/// with Clang.
const LTO_FAT_OBJECTS_IGNORED: ErrorPattern = ErrorPattern {
    key: "LTO_FAT_OBJECTS_IGNORED",
    description: "Ubuntu's -ffat-lto-objects flag is silently ignored by Clang (different LTO model)",
    patterns: &["ignored-optimization-argument", "-ffat-lto-objects"],
    require_prefix: None,
    exclude_if_contains: &[],
    dedup_by_extracted_key: false,
};

/// A GCC-specific warning flag (-Wlogical-op, -Wsync-nand, etc.) that Clang
/// does not recognise.  The flag is ignored with a warning but does not cause
/// the build to fail.  Per-package issue; Clang has different warning flag names.
///
/// Observed packages: iptables, iw, lvm2, make-dfsg, tar.
const UNKNOWN_WARNING_FLAG: ErrorPattern = ErrorPattern {
    key: "UNKNOWN_WARNING_FLAG",
    description: "GCC-specific warning flag not recognised by Clang (ignored, not a failure)",
    patterns: &["unknown warning option"],
    require_prefix: None,
    // Exclude configure probe lines that intentionally test for flag support.
    exclude_if_contains: &["checking whether", "supports compile flag", "compiler handles"],
    dedup_by_extracted_key: true, // key = flag name
};

pub static OBSERVATION_PATTERNS: &[&ErrorPattern] = &[
    &LTO_FAT_OBJECTS_IGNORED,
    &UNKNOWN_WARNING_FLAG,
];

// ---------------------------------------------------------------------------
// Pattern matching helper
// ---------------------------------------------------------------------------

/// Find the first matching pattern for a log line.
///
/// Returns `Some(pattern)` if the line matches any needle in the pattern,
/// the optional `require_prefix` is satisfied, and none of the
/// `exclude_if_contains` strings are present.
pub fn match_pattern<'a>(
    line: &str,
    patterns: &'a [&'a ErrorPattern],
) -> Option<&'a ErrorPattern> {
    for pattern in patterns {
        // Check prefix requirement first (fast rejection).
        if let Some(prefix) = pattern.require_prefix {
            if !line.contains(prefix) {
                continue;
            }
        }
        // Check exclusions.
        if pattern.exclude_if_contains.iter().any(|exc| line.contains(exc)) {
            continue;
        }
        // Check needles.
        if pattern.patterns.iter().any(|needle| line.contains(needle)) {
            return Some(pattern);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn match_error(line: &str) -> Option<&'static ErrorPattern> {
        match_pattern(line, ERROR_PATTERNS)
    }

    fn match_obs(line: &str) -> Option<&'static ErrorPattern> {
        match_pattern(line, OBSERVATION_PATTERNS)
    }

    #[test]
    fn nested_function() {
        let p = match_error("bogl-font.c:84:3: error: function definition is not allowed here");
        assert!(p.is_some());
        assert_eq!(p.unwrap().key, "GNU_NESTED_FUNCTIONS");
    }

    #[test]
    fn lto_dwarf_mismatch() {
        let p = match_error("/usr/bin/ld: DWARF error: invalid or unhandled FORM value: 0x23");
        assert!(p.is_some());
        assert_eq!(p.unwrap().key, "LTO_DWARF_MISMATCH");
    }

    #[test]
    fn blocks_runtime() {
        let p = match_error("ld-temp.o: undefined reference to `_Block_object_assign'");
        assert!(p.is_some());
        assert_eq!(p.unwrap().key, "BLOCKS_RUNTIME_MISSING");
    }

    #[test]
    fn missing_header_requires_fatal_error_prefix() {
        // omp.h missing is caught by MISSING_OPENMP (which has no prefix requirement)
        // before MISSING_HEADER, which is correct.
        let p = match_error("fatal error: 'omp.h' file not found");
        assert!(p.is_some());
        assert_eq!(p.unwrap().key, "MISSING_OPENMP");

        // A non-OpenMP header missing should match MISSING_HEADER.
        let p2 = match_error("fatal error: 'foo/bar.h' file not found");
        assert!(p2.is_some());
        assert_eq!(p2.unwrap().key, "MISSING_HEADER");

        // "No such file or directory" without "fatal error:" — should NOT match MISSING_HEADER.
        let p3 = match_error("rm: cannot remove 'libtoolT': No such file or directory");
        assert!(p3.is_none() || p3.unwrap().key != "MISSING_HEADER");

        // SSL cert path — should NOT match.
        let p4 = match_error("Warning: /etc/ssl/certs/NetLock.pem (No such file or directory)");
        assert!(p4.is_none() || p4.unwrap().key != "MISSING_HEADER");
    }

    #[test]
    fn link_missing_symbol_not_matching_command_line() {
        // The actual linker error line.
        let p = match_error("/usr/bin/ld: .libs/libbarcode.so: undefined reference to `rpl_calloc'");
        assert!(p.is_some());
        assert_eq!(p.unwrap().key, "LINK_MISSING_SYMBOL");
    }

    #[test]
    fn configure_probe_not_a_flag_error() {
        // Configure is testing for flag support — should not fire UNSUPPORTED_COMPILER_FLAG.
        let p = match_error("checking whether C compiler handles -Wunused-parameter... yes");
        // Either no match or not the flag error.
        assert!(p.is_none() || p.unwrap().key != "UNSUPPORTED_COMPILER_FLAG");
    }

    #[test]
    fn implicit_function_configure_probe_excluded() {
        let p = match_error("checking whether compiler handles -Wimplicit-function-declaration... yes");
        assert!(p.is_none() || p.unwrap().key != "IMPLICIT_FUNCTION_DECLARATION");
    }

    #[test]
    fn lto_fat_objects_is_observation_not_error() {
        let line = "clang: warning: optimization flag '-ffat-lto-objects' is not supported [-Wignored-optimization-argument]";
        // Must NOT fire as an error.
        assert!(match_error(line).is_none());
        // Must fire as an observation.
        let p = match_obs(line);
        assert!(p.is_some());
        assert_eq!(p.unwrap().key, "LTO_FAT_OBJECTS_IGNORED");
    }

    #[test]
    fn unknown_warning_flag_is_observation() {
        let line = "warning: unknown warning option '-Wlogical-op'; did you mean '-Wlong-long'?";
        assert!(match_error(line).is_none());
        let p = match_obs(line);
        assert!(p.is_some());
        assert_eq!(p.unwrap().key, "UNKNOWN_WARNING_FLAG");
    }

    #[test]
    fn cxx11_narrowing() {
        let p = match_error("error: constant expression evaluates to 18446744073709551615 which cannot be narrowed to type 'int64_t' [-Wc++11-narrowing]");
        assert!(p.is_some());
        assert_eq!(p.unwrap().key, "CXX11_NARROWING");
    }
}
