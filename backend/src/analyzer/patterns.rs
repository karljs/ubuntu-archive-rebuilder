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
    /// Categories that, if present elsewhere in the same build's findings,
    /// suppress this finding.  Used to make generic catch-all patterns (e.g.
    /// LINK_FAILURE) fall back: they are emitted only when no more-specific
    /// finding explains the failure.  Empty for most patterns.
    pub suppressed_by: &'static [&'static str],
    /// Whether this pattern reflects a toolchain (compiler) issue or an
    /// environmental/infrastructure artifact unrelated to GCC-vs-Clang.
    /// Defaults conceptually to Toolchain; only a few infra patterns set
    /// Environmental.
    pub class: crate::models::FindingClass,
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
    suppressed_by: &[],
    class: crate::models::FindingClass::Toolchain,
};

/// GCC extension: variable-length arrays as struct members.
const GNU_VLA_IN_STRUCT: ErrorPattern = ErrorPattern {
    key: "GNU_VLA_IN_STRUCT",
    description: "Variable-length array in struct (GNU extension not supported by Clang)",
    patterns: &["variable length array in structure"],
    require_prefix: None,
    exclude_if_contains: &[],
    dedup_by_extracted_key: false,
    suppressed_by: &[],
    class: crate::models::FindingClass::Toolchain,
};

/// GCC extension: hardware register assignment via `register` keyword.
const GNU_GLOBAL_REGISTER_VAR: ErrorPattern = ErrorPattern {
    key: "GNU_GLOBAL_REGISTER_VAR",
    description: "Global register variable (GNU extension not supported by Clang)",
    patterns: &["global register variables are not supported"],
    require_prefix: None,
    exclude_if_contains: &[],
    dedup_by_extracted_key: false,
    suppressed_by: &[],
    class: crate::models::FindingClass::Toolchain,
};

/// GCC extension: `asm goto` — jumps from inline assembly to C labels.
const GNU_ASM_GOTO: ErrorPattern = ErrorPattern {
    key: "GNU_ASM_GOTO",
    description: "asm goto construct (unsupported or differently handled by Clang)",
    patterns: &["'asm goto' constructs are not supported"],
    require_prefix: None,
    exclude_if_contains: &[],
    dedup_by_extracted_key: false,
    suppressed_by: &[],
    class: crate::models::FindingClass::Toolchain,
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
    suppressed_by: &[],
    class: crate::models::FindingClass::Toolchain,
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
    suppressed_by: &[],
    class: crate::models::FindingClass::Toolchain,
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
    suppressed_by: &[],
    class: crate::models::FindingClass::Toolchain,
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
    suppressed_by: &[],
    class: crate::models::FindingClass::Toolchain,
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
    suppressed_by: &[],
    class: crate::models::FindingClass::Toolchain,
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
    suppressed_by: &[],
    class: crate::models::FindingClass::Toolchain,
};

/// Access to private or protected class members.
const CXX_ACCESS_VIOLATION: ErrorPattern = ErrorPattern {
    key: "CXX_ACCESS_VIOLATION",
    description: "Access to private or protected class member",
    patterns: &["is a private member of", "is a protected member of"],
    require_prefix: None,
    exclude_if_contains: &[],
    dedup_by_extracted_key: false,
    suppressed_by: &[],
    class: crate::models::FindingClass::Toolchain,
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
    suppressed_by: &[],
    class: crate::models::FindingClass::Toolchain,
};

/// Implicit instantiation of an undefined template.
const CXX_IMPLICIT_INSTANTIATION: ErrorPattern = ErrorPattern {
    key: "CXX_IMPLICIT_INSTANTIATION",
    description: "Implicit instantiation of undefined template",
    patterns: &["implicit instantiation of undefined template"],
    require_prefix: None,
    exclude_if_contains: &[],
    dedup_by_extracted_key: false,
    suppressed_by: &[],
    class: crate::models::FindingClass::Toolchain,
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
    suppressed_by: &[],
    class: crate::models::FindingClass::Toolchain,
};

/// Unknown type name — often caused by a missing typedef or include.
const UNKNOWN_TYPE_NAME: ErrorPattern = ErrorPattern {
    key: "UNKNOWN_TYPE_NAME",
    description: "Unknown type name; possibly missing typedef or #include",
    patterns: &["unknown type name"],
    require_prefix: None,
    exclude_if_contains: &[],
    dedup_by_extracted_key: true,
    suppressed_by: &[],
    class: crate::models::FindingClass::Toolchain,
};

/// Implicit integer-to-pointer (or pointer-to-integer) conversion.
/// GCC and older Clang accept this with a warning; Clang 15+ promotes
/// -Wint-conversion to an error by default, breaking C code that relied on
/// the laxer behaviour.  Observed: curl (x509asn1.c returning int as char*).
const CXX_INT_CONVERSION: ErrorPattern = ErrorPattern {
    key: "CXX_INT_CONVERSION",
    description: "Implicit integer/pointer conversion rejected (Clang 15+ treats -Wint-conversion as an error)",
    patterns: &["-Wint-conversion"],
    require_prefix: None,
    exclude_if_contains: &[],
    dedup_by_extracted_key: false,
    suppressed_by: &[],
    class: crate::models::FindingClass::Toolchain,
};

/// Incompatible function pointer types.  Clang enforces stricter function
/// pointer type compatibility than GCC and promotes this to an error by
/// default in recent versions.  Observed: gettext (obstack.c initializing a
/// noreturn function pointer from a non-noreturn function).
const INCOMPATIBLE_FUNCTION_POINTER: ErrorPattern = ErrorPattern {
    key: "INCOMPATIBLE_FUNCTION_POINTER",
    description: "Incompatible function pointer types (Clang enforces stricter typing than GCC)",
    patterns: &["-Wincompatible-function-pointer-types"],
    require_prefix: None,
    exclude_if_contains: &[],
    dedup_by_extracted_key: false,
    suppressed_by: &[],
    class: crate::models::FindingClass::Toolchain,
};

/// Use of the `register` storage-class specifier, removed in C++17.  Clang
/// rejects it as an error under C++17; GCC only warns.  Per-package source
/// issue.  Observed: lshw (partitions.cc) across every Clang version/series.
const CXX17_REGISTER_REMOVED: ErrorPattern = ErrorPattern {
    key: "CXX17_REGISTER_REMOVED",
    description: "Use of 'register' storage class specifier, removed in C++17 (Clang errors; GCC warns)",
    patterns: &[
        "'register' storage class specifier",
        "-Wregister",
    ],
    require_prefix: None,
    exclude_if_contains: &[],
    dedup_by_extracted_key: false,
    suppressed_by: &[],
    class: crate::models::FindingClass::Toolchain,
};

/// Clang's checked-arithmetic builtins (__builtin_*_overflow) require an
/// integer type; Clang rejects plain `char`/`bool`/enum operands that GCC
/// accepted.  Observed: coreutils (lib/posixtm.c).
const CXX_CHECKED_INT_TYPE: ErrorPattern = ErrorPattern {
    key: "CXX_CHECKED_INT_TYPE",
    description: "Checked integer builtin requires a proper integer type (Clang is stricter than GCC)",
    patterns: &["checked integer operation must be an integer type"],
    require_prefix: None,
    exclude_if_contains: &[],
    dedup_by_extracted_key: false,
    suppressed_by: &[],
    class: crate::models::FindingClass::Toolchain,
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
    suppressed_by: &[],
    class: crate::models::FindingClass::Toolchain,
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
    suppressed_by: &[],
    class: crate::models::FindingClass::Toolchain,
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
    suppressed_by: &[],
    class: crate::models::FindingClass::Toolchain,
};

/// Required library not found during linking.
const LINK_MISSING_LIBRARY: ErrorPattern = ErrorPattern {
    key: "LINK_MISSING_LIBRARY",
    description: "Required library not found during linking",
    patterns: &["cannot find -l"],
    require_prefix: None,
    exclude_if_contains: &[],
    dedup_by_extracted_key: true, // key = library name
    suppressed_by: &[],
    class: crate::models::FindingClass::Toolchain,
};

/// Generic linker command failure not covered by a more specific category.
/// Intentionally placed last in the error list so more specific patterns
/// match first.
///
/// This is a catch-all: a failed link almost always also produces a specific
/// diagnostic (an undefined symbol, a missing library, a multiple definition,
/// etc.) on a *different* log line.  Since the scanner matches each line
/// independently, both the specific finding and this generic one would be
/// recorded for the same failure.  `suppressed_by` drops this finding whenever
/// a more-specific linker cause is present, so LINK_FAILURE only surfaces when
/// it is genuinely the only information available.
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
    suppressed_by: &[
        "LINK_MISSING_SYMBOL",
        "BLOCKS_RUNTIME_MISSING",
        "LINK_MULTIPLE_DEFINITION",
        "LINK_MISSING_LIBRARY",
    ],
    class: crate::models::FindingClass::Toolchain,
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
    suppressed_by: &[],
    class: crate::models::FindingClass::Toolchain,
};

/// Clang rejects `-flto=auto`.  GCC and Clang 12+ accept the `auto` argument
/// to `-flto` (meaning "use $(nproc) link jobs"), but Clang 11 does not — it
/// errors out.  Ubuntu injects `-flto=auto` into CFLAGS/LDFLAGS for every
/// package, so on Clang 11 essentially every build fails at the first
/// compile.  This is the dominant failure cause for the clang-11 batches.
///
/// Distinct from UNSUPPORTED_COMPILER_FLAG (which catches "unknown argument"
/// / "unsupported option"); this is a *valid* flag with an argument value
/// that this Clang version cannot parse, so it gets its own category.
const UNSUPPORTED_LTO_AUTO: ErrorPattern = ErrorPattern {
    key: "UNSUPPORTED_LTO_AUTO",
    description: "Clang does not accept '-flto=auto' (Clang 11; Ubuntu injects this flag globally)",
    patterns: &["invalid value 'auto' in '-flto=auto'"],
    require_prefix: None,
    exclude_if_contains: &[],
    dedup_by_extracted_key: false,
    suppressed_by: &[],
    class: crate::models::FindingClass::Toolchain,
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
/// Observed: libffi (-print-multi-os-directory), ppp (--print-sysroot),
/// gmp (--debug-prefix-map, which Clang spells -fdebug-prefix-map).
/// Excludes configure probe lines where the failure is expected and handled.
const UNSUPPORTED_COMPILER_FLAG: ErrorPattern = ErrorPattern {
    key: "UNSUPPORTED_COMPILER_FLAG",
    description: "Compiler flag not supported by Clang",
    patterns: &[
        "unsupported option",
        "unknown argument:",
        // Clang's "unknown argument '<flag>'; did you mean ...?" form. The flag
        // name is extracted as the dedup key from the single-quoted token.
        "unknown argument '",
        "error: unsupported argument",
        "the clang compiler does not support",
    ],
    require_prefix: None,
    // These patterns appear in configure probe output where the error is
    // intentional — autoconf tests flag support by trying it and checking
    // the exit code.
    exclude_if_contains: &[
        "conftest",
        "ac_ext",
        "checking for",
        "checking whether",
        // Autoconf/libtool compiler-identification probes: it runs the compiler
        // with each of these version/ident flags to detect the toolchain, and
        // the "unknown argument" error is expected and ignored.  These are never
        // a real build failure (every affected build also has a genuine finding).
        "'-qversion'",
        "'-version'",
        "'-V'",
        "'--version'",
        "'-qversion;",
        "'--ec++'",
        "'--c++'",
    ],
    dedup_by_extracted_key: true, // key = flag name
    suppressed_by: &[],
    class: crate::models::FindingClass::Toolchain,
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
    suppressed_by: &[],
    class: crate::models::FindingClass::Toolchain,
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
    suppressed_by: &[],
    class: crate::models::FindingClass::Toolchain,
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
    suppressed_by: &[],
    class: crate::models::FindingClass::Toolchain,
};

// ---------------------------------------------------------------------------
// ── Group 8: Build System / Configure Failures ───────────────────────────
//
// The build system itself fails to recognise or work with Clang, preventing
// the build from starting.
// ---------------------------------------------------------------------------

/// Configure script cannot create executables with Clang.
/// Observed: readline, recode — configure's compiler sanity check fails.
/// Also catches downstream sanity-check failures when the toolchain is broken
/// (e.g. Clang 11 + -flto=auto): gmp ("could not find a working compiler"),
/// cmake bootstrap ("Cannot find appropriate C compiler"), zlib
/// ("reporting is too harsh for ./configure").
const CONFIGURE_COMPILER_TEST_FAILED: ErrorPattern = ErrorPattern {
    key: "CONFIGURE_COMPILER_TEST_FAILED",
    description: "Configure/bootstrap cannot compile a test program with this compiler",
    patterns: &[
        "compiler cannot create executables",
        "C compiler cannot create executables",
        "Can't run the compiler",
        "could not find a working compiler",
        "Cannot find appropriate C compiler",
        "reporting is too harsh",
    ],
    require_prefix: None,
    exclude_if_contains: &[],
    dedup_by_extracted_key: false,
    suppressed_by: &[],
    class: crate::models::FindingClass::Toolchain,
};

/// CMake feature-detection probe failed.  CMake compiles small programs to
/// test for language features; under Clang these probes can fail where they
/// passed under GCC, aborting configuration with "CMake Error".  Distinct from
/// CXX_STD_REQUIREMENT (an in-source `-std=` requirement message) because the
/// failure originates in CMake's own feature checks.
/// Observed: cmake ("The C++ compiler does not support C++11").
const CMAKE_FEATURE_PROBE_FAILED: ErrorPattern = ErrorPattern {
    key: "CMAKE_FEATURE_PROBE_FAILED",
    description: "CMake compiler feature probe failed (a required language feature was not detected under Clang)",
    patterns: &[
        "does not support C++11",
        "compiler does not support C++",
    ],
    require_prefix: None,
    exclude_if_contains: &[],
    dedup_by_extracted_key: false,
    suppressed_by: &[],
    class: crate::models::FindingClass::Toolchain,
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
    suppressed_by: &[],
    class: crate::models::FindingClass::Toolchain,
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
    suppressed_by: &[],
    class: crate::models::FindingClass::Toolchain,
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
    suppressed_by: &[],
    class: crate::models::FindingClass::Toolchain,
};

// ---------------------------------------------------------------------------
// ── Group 10: Build Infrastructure ───────────────────────────────────────
// ---------------------------------------------------------------------------

/// Parallel `make install` race condition.  Under high `-j`, multiple `make`
/// subprocesses invoke `install -d` (or `install -D`) on the same target
/// directory concurrently; one wins the mkdir and the others fail with
/// "install: cannot create directory".  Compilation and linking succeed — only
/// the install phase races.
///
/// This is build-infrastructure flakiness, NOT a toolchain incompatibility: it
/// is unrelated to whether GCC or Clang was used.  Observed on lvm2 and mdadm,
/// exclusively on the resolute series, under -j parallelism.
///
/// The needle is the interleaving-robust substring `cannot create directory`
/// rather than the full `install: cannot create directory`: under -j the
/// stderr of several concurrent `install` processes interleaves into forms
/// like `installinstall: : cannot create directory`, which would not match a
/// needle containing the `: ` separator.  Error patterns only scan *failed*
/// builds, so succeeded builds that recovered from the race are unaffected.
const PARALLEL_INSTALL_RACE: ErrorPattern = ErrorPattern {
    key: "PARALLEL_INSTALL_RACE",
    description: "Parallel `make install` race: concurrent `install -d` failed to create a directory (build-infrastructure flakiness, not a toolchain issue)",
    patterns: &["cannot create directory"],
    require_prefix: None,
    exclude_if_contains: &[],
    dedup_by_extracted_key: false,
    suppressed_by: &[],
    class: crate::models::FindingClass::Environmental,
};

/// The build process was killed because it exceeded the time limit.
/// (Also detected by infer_status; recorded as a finding for completeness.)
const BUILD_TIMEOUT: ErrorPattern = ErrorPattern {
    key: "BUILD_TIMEOUT",
    description: "Build killed because it exceeded the time limit",
    patterns: &["Build killed with signal", "Timed out"],
    require_prefix: None,
    exclude_if_contains: &[],
    dedup_by_extracted_key: false,
    suppressed_by: &[],
    class: crate::models::FindingClass::Toolchain,
};

/// The compiler itself segfaulted — a Clang bug, not a package issue.
/// Includes hard frontend crashes that abort with a non-zero exit and a
/// "PLEASE submit a bug report" banner.  Observed: openssl (clang-17 frontend
/// command failed with exit code 139 — a SIGSEGV).
const SEGFAULT_IN_COMPILER: ErrorPattern = ErrorPattern {
    key: "SEGFAULT_IN_COMPILER",
    description: "Compiler process crashed (segmentation fault or frontend crash — likely a Clang bug)",
    patterns: &[
        "Segmentation fault (core dumped)",
        "LLVM ERROR: ",
        "clang: error: unable to execute command: Segmentation fault",
        "frontend command failed with exit code",
        "PLEASE submit a bug report",
    ],
    require_prefix: None,
    exclude_if_contains: &[],
    dedup_by_extracted_key: false,
    suppressed_by: &[],
    class: crate::models::FindingClass::Toolchain,
};

/// The build process ran out of memory.
const OUT_OF_MEMORY: ErrorPattern = ErrorPattern {
    key: "OUT_OF_MEMORY",
    description: "Build process ran out of memory",
    patterns: &["Cannot allocate memory", "out of memory", "memory exhausted"],
    require_prefix: None,
    exclude_if_contains: &[],
    dedup_by_extracted_key: false,
    suppressed_by: &[],
    class: crate::models::FindingClass::Toolchain,
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
    suppressed_by: &[],
    class: crate::models::FindingClass::Toolchain,
};

/// Source package could not be fetched, so no build ever ran.  `pull-lp-source`
/// failed (missing/unverifiable signing key, network, or archive issue).  This
/// is NOT a toolchain result — it is an infrastructure/setup failure and should
/// be excluded from compiler-comparison analysis.  Observed: attr ("Public key
/// not found, could not verify signature").
const SOURCE_FETCH_FAILED: ErrorPattern = ErrorPattern {
    key: "SOURCE_FETCH_FAILED",
    description: "Source fetch failed before the build started (pull-lp-source error; not a toolchain result)",
    patterns: &[
        "pull-lp-source failed",
        "Public key not found, could not verify signature",
    ],
    require_prefix: None,
    exclude_if_contains: &[],
    dedup_by_extracted_key: false,
    suppressed_by: &[],
    class: crate::models::FindingClass::Environmental,
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
    &CXX_INT_CONVERSION,
    &INCOMPATIBLE_FUNCTION_POINTER,
    &CXX17_REGISTER_REMOVED,
    &CXX_CHECKED_INT_TYPE,
    // Linker — specific causes before generic failure
    &LINK_MISSING_SYMBOL,
    &LINK_MULTIPLE_DEFINITION,
    &LINK_MISSING_LIBRARY,
    &LINK_FAILURE,             // catch-all, last among linker patterns
    // LTO argument unsupported by this Clang (before generic flag pattern)
    &UNSUPPORTED_LTO_AUTO,
    // Compiler flags
    &UNSUPPORTED_COMPILER_FLAG,
    // -Werror promotions
    &WERROR_FORMAT_STRING,
    &WERROR_UNUSED,
    &WERROR_OTHER,
    // Build system
    &CONFIGURE_COMPILER_TEST_FAILED,
    &CMAKE_FEATURE_PROBE_FAILED,
    &BUILD_SYSTEM_MISDETECTS_COMPILER,
    // Missing deps — MISSING_OPENMP before MISSING_HEADER (more specific)
    &MISSING_OPENMP,
    &MISSING_HEADER,
    // Infrastructure
    &SOURCE_FETCH_FAILED,
    &PARALLEL_INSTALL_RACE,
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
/// The appropriate response is a global profile flag to suppress the warning,
/// or an Ubuntu infrastructure change to strip -ffat-lto-objects when building
/// with Clang.
///
/// IMPORTANT: the only reliable signal is Clang's warning, identified by the
/// `-Wignored-optimization-argument` diagnostic name.  Matching on the bare
/// `-ffat-lto-objects` string is wrong: that flag appears verbatim on every
/// compile/configure command line (Ubuntu injects it into CFLAGS/LDFLAGS for
/// every package), so it fires on essentially every successful build —
/// including GCC builds, which fully support the flag and never ignore it.
const LTO_FAT_OBJECTS_IGNORED: ErrorPattern = ErrorPattern {
    key: "LTO_FAT_OBJECTS_IGNORED",
    description: "Ubuntu's -ffat-lto-objects flag is silently ignored by Clang (different LTO model)",
    patterns: &["ignored-optimization-argument"],
    require_prefix: None,
    exclude_if_contains: &[],
    dedup_by_extracted_key: false,
    suppressed_by: &[],
    class: crate::models::FindingClass::Toolchain,
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
    suppressed_by: &[],
    class: crate::models::FindingClass::Toolchain,
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
    fn lto_fat_objects_does_not_match_plain_command_line() {
        // A plain compile command line carries -ffat-lto-objects (Ubuntu injects
        // it into CFLAGS for every package).  This is NOT an "ignored flag"
        // warning and must not produce a finding — neither for GCC nor Clang.
        let gcc_line = "gcc -DHAVE_CONFIG_H -I. -g -O2 -flto=auto -ffat-lto-objects -flto=auto -ffat-lto-objects -fstack-protector-strong -Wformat -c -o foo.o foo.c";
        assert!(match_obs(gcc_line).is_none());
        assert!(match_error(gcc_line).is_none());

        let clang_line = "clang -DHAVE_CONFIG_H -I. -g -O2 -flto=auto -ffat-lto-objects -fstack-protector-strong -c -o foo.o foo.c";
        assert!(match_obs(clang_line).is_none());
        assert!(match_error(clang_line).is_none());
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

    #[test]
    fn parallel_install_race() {
        let p = match_error("install: cannot create directory '/build/x/usr/lib/udev/rules.d'");
        assert!(p.is_some());
        assert_eq!(p.unwrap().key, "PARALLEL_INSTALL_RACE");
    }

    #[test]
    fn parallel_install_race_interleaved() {
        // Under -j the install stderr interleaves; the colon-separated form is
        // mangled but `cannot create directory` survives and must still match.
        let p = match_error("installinstallinstall: : : cannot create directory '/build/x/usr/lib/x86_64-linux-gnu/device-mapper'");
        assert!(p.is_some());
        assert_eq!(p.unwrap().key, "PARALLEL_INSTALL_RACE");
    }

    #[test]
    fn unsupported_lto_auto() {
        let p = match_error("error: invalid value 'auto' in '-flto=auto'");
        assert!(p.is_some());
        assert_eq!(p.unwrap().key, "UNSUPPORTED_LTO_AUTO");
    }

    #[test]
    fn cxx17_register_removed() {
        let p = match_error("partitions.cc:631:3: error: ISO C++17 does not allow 'register' storage class specifier [-Wregister]");
        assert!(p.is_some());
        assert_eq!(p.unwrap().key, "CXX17_REGISTER_REMOVED");
    }

    #[test]
    fn int_conversion() {
        let p = match_error("vtls/x509asn1.c:569:14: error: incompatible integer to pointer conversion returning 'int' from a function with result type 'const char *' [-Wint-conversion]");
        assert!(p.is_some());
        assert_eq!(p.unwrap().key, "CXX_INT_CONVERSION");
    }

    #[test]
    fn incompatible_function_pointer() {
        let p = match_error("obstack.c:351:31: error: incompatible function pointer types initializing 'void (*)(void)' [-Wincompatible-function-pointer-types]");
        assert!(p.is_some());
        assert_eq!(p.unwrap().key, "INCOMPATIBLE_FUNCTION_POINTER");
    }

    #[test]
    fn checked_int_type() {
        let p = match_error("lib/posixtm.c:194:15: error: operand argument to checked integer operation must be an integer type other than plain 'char', 'bool', bit-precise, or an enumeration ('bool' invalid)");
        assert!(p.is_some());
        assert_eq!(p.unwrap().key, "CXX_CHECKED_INT_TYPE");
    }

    #[test]
    fn unknown_argument_double_dash_flag() {
        // gmp: GCC-style --debug-prefix-map rejected by Clang. The flag name is
        // extracted as the dedup key.
        let p = match_error("clang-18: error: unknown argument '--debug-prefix-map=/build/x=.'; did you mean '-fdebug-prefix-map=/build/x=.'?");
        assert!(p.is_some());
        assert_eq!(p.unwrap().key, "UNSUPPORTED_COMPILER_FLAG");
    }

    #[test]
    fn lto_auto_before_unsupported_flag() {
        // The -flto=auto error must classify as UNSUPPORTED_LTO_AUTO, not the
        // generic UNSUPPORTED_COMPILER_FLAG.
        let p = match_error("error: invalid value 'auto' in '-flto=auto'");
        assert_eq!(p.unwrap().key, "UNSUPPORTED_LTO_AUTO");
    }

    #[test]
    fn cmake_feature_probe_failed() {
        let p = match_error("  The C++ compiler does not support C++11 (e.g.  std::unique_ptr).");
        assert!(p.is_some());
        assert_eq!(p.unwrap().key, "CMAKE_FEATURE_PROBE_FAILED");
    }

    #[test]
    fn configure_working_compiler() {
        let p = match_error("configure: error: could not find a working compiler, see config.log for details");
        assert!(p.is_some());
        assert_eq!(p.unwrap().key, "CONFIGURE_COMPILER_TEST_FAILED");
    }

    #[test]
    fn segfault_frontend_crash() {
        let p = match_error("clang-17: error: clang frontend command failed with exit code 139 (use -v to see invocation)");
        assert!(p.is_some());
        assert_eq!(p.unwrap().key, "SEGFAULT_IN_COMPILER");
    }

    #[test]
    fn source_fetch_failed() {
        let p = match_error("Build failed to execute: pull-lp-source failed for attr in noble: Public key not found, could not verify signature");
        assert!(p.is_some());
        assert_eq!(p.unwrap().key, "SOURCE_FETCH_FAILED");
    }
}
