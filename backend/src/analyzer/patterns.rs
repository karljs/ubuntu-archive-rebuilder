//! Log-line patterns: ERROR_PATTERNS (failed builds), OBSERVATION_PATTERNS
//! (succeeded builds).

pub struct ErrorPattern {
    pub key: &'static str,
    pub description: &'static str,
    pub patterns: &'static [&'static str],
    pub require_prefix: Option<&'static str>,
    pub exclude_if_contains: &'static [&'static str],
    pub dedup_by_extracted_key: bool,
    pub suppressed_by: &'static [&'static str],
    pub class: crate::models::FindingClass,
}

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

const IMPLICIT_FUNCTION_DECLARATION: ErrorPattern = ErrorPattern {
    key: "IMPLICIT_FUNCTION_DECLARATION",
    description: "Implicit function declaration (not permitted in C99; Clang rejects)",
    patterns: &[
        "implicit declaration of function",
        "Wimplicit-function-declaration",
    ],
    require_prefix: None,
    exclude_if_contains: &[
        // Configure probes, not failures.
        "checking whether",
        "supports compile flag",
        "compiler handles",
    ],
    dedup_by_extracted_key: true,
    suppressed_by: &[],
    class: crate::models::FindingClass::Toolchain,
};

const UNDECLARED_IDENTIFIER: ErrorPattern = ErrorPattern {
    key: "UNDECLARED_IDENTIFIER",
    description: "Use of undeclared identifier",
    patterns: &["use of undeclared identifier"],
    require_prefix: None,
    exclude_if_contains: &["use of undeclared identifier '__builtin_"],
    dedup_by_extracted_key: true,
    suppressed_by: &[],
    class: crate::models::FindingClass::Toolchain,
};

const MISSING_BUILTIN: ErrorPattern = ErrorPattern {
    key: "MISSING_BUILTIN",
    description: "GCC built-in function not available in Clang",
    patterns: &["use of undeclared identifier '__builtin_"],
    require_prefix: None,
    exclude_if_contains: &[],
    dedup_by_extracted_key: true,
    suppressed_by: &[],
    class: crate::models::FindingClass::Toolchain,
};

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

// Clang 15+ promotes -Wint-conversion to an error by default.
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

const CXX17_REGISTER_REMOVED: ErrorPattern = ErrorPattern {
    key: "CXX17_REGISTER_REMOVED",
    description:
        "Use of 'register' storage class specifier, removed in C++17 (Clang errors; GCC warns)",
    patterns: &["'register' storage class specifier", "-Wregister"],
    require_prefix: None,
    exclude_if_contains: &[],
    dedup_by_extracted_key: false,
    suppressed_by: &[],
    class: crate::models::FindingClass::Toolchain,
};

const CXX_CHECKED_INT_TYPE: ErrorPattern = ErrorPattern {
    key: "CXX_CHECKED_INT_TYPE",
    description:
        "Checked integer builtin requires a proper integer type (Clang is stricter than GCC)",
    patterns: &["checked integer operation must be an integer type"],
    require_prefix: None,
    exclude_if_contains: &[],
    dedup_by_extracted_key: false,
    suppressed_by: &[],
    class: crate::models::FindingClass::Toolchain,
};

const LINK_MISSING_SYMBOL: ErrorPattern = ErrorPattern {
    key: "LINK_MISSING_SYMBOL",
    description: "Undefined symbol at link time",
    patterns: &["undefined reference to"],
    require_prefix: None,
    exclude_if_contains: &["libtool:", "gcc -", "g++ -", "clang-", "clang "],
    dedup_by_extracted_key: true,
    suppressed_by: &[],
    class: crate::models::FindingClass::Toolchain,
};

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

const LINK_MISSING_LIBRARY: ErrorPattern = ErrorPattern {
    key: "LINK_MISSING_LIBRARY",
    description: "Required library not found during linking",
    patterns: &["cannot find -l"],
    require_prefix: None,
    exclude_if_contains: &[],
    dedup_by_extracted_key: true,
    suppressed_by: &[],
    class: crate::models::FindingClass::Toolchain,
};

// Catch-all: suppressed_by keeps it from double-reporting when a specific
// diagnostic matched another line.
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

// Clang emits DWARF5 by default; dwz 0.15/0.16 can't process it. The
// -gdwarf-4 profile flag exists for this.

const LTO_DWARF_MISMATCH: ErrorPattern = ErrorPattern {
    key: "LTO_DWARF_MISMATCH",
    description:
        "DWARF5 format incompatibility; dwz cannot process Clang output (use -gdwarf-4 profile)",
    patterns: &[
        "DWARF error: invalid or unhandled FORM value",
        "DWARF error: can't find",
        "DWARF error: offset",
        "Unknown debugging section .debug_addr",
        "dh_dwz: error: dwz",
    ],
    require_prefix: None,
    exclude_if_contains: &[],
    dedup_by_extracted_key: false,
    suppressed_by: &[],
    class: crate::models::FindingClass::Toolchain,
};

// Ubuntu injects -flto=auto globally; Clang 11 rejects the value. Not an
// unknown flag, a bad argument to a valid one.
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

const UNSUPPORTED_COMPILER_FLAG: ErrorPattern = ErrorPattern {
    key: "UNSUPPORTED_COMPILER_FLAG",
    description: "Compiler flag not supported by Clang",
    patterns: &[
        "unsupported option",
        "unknown argument:",
        "unknown argument '",
        "error: unsupported argument",
        "the clang compiler does not support",
    ],
    require_prefix: None,
    // Configure probes and compiler-identification probes (-qversion, -V);
    // the errors are expected, never real failures.
    exclude_if_contains: &[
        "conftest",
        "ac_ext",
        "checking for",
        "checking whether",
        "'-qversion'",
        "'-version'",
        "'-V'",
        "'--version'",
        "'-qversion;",
        "'--ec++'",
        "'--c++'",
    ],
    dedup_by_extracted_key: true,
    suppressed_by: &[],
    class: crate::models::FindingClass::Toolchain,
};

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

const WERROR_UNUSED: ErrorPattern = ErrorPattern {
    key: "WERROR_UNUSED",
    description: "Unused variable/parameter/function warning promoted to error via -Werror",
    patterns: &["-Werror,-Wunused", "error: unused"],
    require_prefix: None,
    exclude_if_contains: &[
        "checking whether",
        "supports compile flag",
        "compiler handles",
    ],
    dedup_by_extracted_key: false,
    suppressed_by: &[],
    class: crate::models::FindingClass::Toolchain,
};

const WERROR_OTHER: ErrorPattern = ErrorPattern {
    key: "WERROR_OTHER",
    description: "Warning promoted to error via -Werror",
    patterns: &["-Werror,-W", "error: -Werror"],
    require_prefix: None,
    exclude_if_contains: &["-Werror,-Wformat", "-Werror,-Wunused"],
    dedup_by_extracted_key: false,
    suppressed_by: &[],
    class: crate::models::FindingClass::Toolchain,
};

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

// require_prefix avoids incidental "No such file" lines from cleanup; the
// excludes drop autoconf probe headers.
const MISSING_HEADER: ErrorPattern = ErrorPattern {
    key: "MISSING_HEADER",
    description: "Required header file not found",
    patterns: &["file not found", "No such file or directory"],
    require_prefix: Some("fatal error:"),
    exclude_if_contains: &["ac_nonexistent.h", "conftest"],
    dedup_by_extracted_key: true,
    suppressed_by: &[],
    class: crate::models::FindingClass::Toolchain,
};

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

// Infra flakiness, not a toolchain issue: concurrent `install -d` under -j
// races. The needle omits "install: " because interleaved stderr mangles it.
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

const SEGFAULT_IN_COMPILER: ErrorPattern = ErrorPattern {
    key: "SEGFAULT_IN_COMPILER",
    description:
        "Compiler process crashed (segmentation fault or frontend crash; likely a Clang bug)",
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

const OUT_OF_MEMORY: ErrorPattern = ErrorPattern {
    key: "OUT_OF_MEMORY",
    description: "Build process ran out of memory",
    patterns: &[
        "Cannot allocate memory",
        "out of memory",
        "memory exhausted",
    ],
    require_prefix: None,
    exclude_if_contains: &[],
    dedup_by_extracted_key: false,
    suppressed_by: &[],
    class: crate::models::FindingClass::Toolchain,
};

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

// First match wins: specific patterns must precede generic ones.
pub static ERROR_PATTERNS: &[&ErrorPattern] = &[
    &LTO_DWARF_MISMATCH,
    &BLOCKS_RUNTIME_MISSING,
    &GNU_NESTED_FUNCTIONS,
    &GNU_VLA_IN_STRUCT,
    &GNU_GLOBAL_REGISTER_VAR,
    &GNU_ASM_GOTO,
    &GNU_ASM_SYNTAX,
    &IMPLICIT_FUNCTION_DECLARATION,
    &MISSING_BUILTIN,
    &UNDECLARED_IDENTIFIER,
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
    &LINK_MISSING_SYMBOL,
    &LINK_MULTIPLE_DEFINITION,
    &LINK_MISSING_LIBRARY,
    &LINK_FAILURE,
    &UNSUPPORTED_LTO_AUTO,
    &UNSUPPORTED_COMPILER_FLAG,
    &WERROR_FORMAT_STRING,
    &WERROR_UNUSED,
    &WERROR_OTHER,
    &CONFIGURE_COMPILER_TEST_FAILED,
    &CMAKE_FEATURE_PROBE_FAILED,
    &BUILD_SYSTEM_MISDETECTS_COMPILER,
    &MISSING_OPENMP,
    &MISSING_HEADER,
    &SOURCE_FETCH_FAILED,
    &PARALLEL_INSTALL_RACE,
    &BUILD_TIMEOUT,
    &SEGFAULT_IN_COMPILER,
    &OUT_OF_MEMORY,
    &SYMBOL_ABI_CHANGE,
];

// Observations: succeeded builds only.

// Match the -Wignored-optimization-argument diagnostic, never the bare
// -ffat-lto-objects string: that flag is on every Ubuntu compile command
// line, so matching it fires on every build, including GCC's.
const LTO_FAT_OBJECTS_IGNORED: ErrorPattern = ErrorPattern {
    key: "LTO_FAT_OBJECTS_IGNORED",
    description:
        "Ubuntu's -ffat-lto-objects flag is silently ignored by Clang (different LTO model)",
    patterns: &["ignored-optimization-argument"],
    require_prefix: None,
    exclude_if_contains: &[],
    dedup_by_extracted_key: false,
    suppressed_by: &[],
    class: crate::models::FindingClass::Toolchain,
};

const UNKNOWN_WARNING_FLAG: ErrorPattern = ErrorPattern {
    key: "UNKNOWN_WARNING_FLAG",
    description: "GCC-specific warning flag not recognised by Clang (ignored, not a failure)",
    patterns: &["unknown warning option"],
    require_prefix: None,
    exclude_if_contains: &[
        "checking whether",
        "supports compile flag",
        "compiler handles",
    ],
    dedup_by_extracted_key: true,
    suppressed_by: &[],
    class: crate::models::FindingClass::Toolchain,
};

pub static OBSERVATION_PATTERNS: &[&ErrorPattern] =
    &[&LTO_FAT_OBJECTS_IGNORED, &UNKNOWN_WARNING_FLAG];

pub fn match_pattern<'a>(line: &str, patterns: &'a [&'a ErrorPattern]) -> Option<&'a ErrorPattern> {
    for pattern in patterns {
        if let Some(prefix) = pattern.require_prefix {
            if !line.contains(prefix) {
                continue;
            }
        }
        if pattern
            .exclude_if_contains
            .iter()
            .any(|exc| line.contains(exc))
        {
            continue;
        }
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
        let p = match_error("fatal error: 'omp.h' file not found");
        assert!(p.is_some());
        assert_eq!(p.unwrap().key, "MISSING_OPENMP");

        let p2 = match_error("fatal error: 'foo/bar.h' file not found");
        assert!(p2.is_some());
        assert_eq!(p2.unwrap().key, "MISSING_HEADER");

        let p3 = match_error("rm: cannot remove 'libtoolT': No such file or directory");
        assert!(p3.is_none() || p3.unwrap().key != "MISSING_HEADER");

        let p4 = match_error("Warning: /etc/ssl/certs/NetLock.pem (No such file or directory)");
        assert!(p4.is_none() || p4.unwrap().key != "MISSING_HEADER");
    }

    #[test]
    fn link_missing_symbol_not_matching_command_line() {
        let p =
            match_error("/usr/bin/ld: .libs/libbarcode.so: undefined reference to `rpl_calloc'");
        assert!(p.is_some());
        assert_eq!(p.unwrap().key, "LINK_MISSING_SYMBOL");
    }

    #[test]
    fn configure_probe_not_a_flag_error() {
        let p = match_error("checking whether C compiler handles -Wunused-parameter... yes");
        assert!(p.is_none() || p.unwrap().key != "UNSUPPORTED_COMPILER_FLAG");
    }

    #[test]
    fn implicit_function_configure_probe_excluded() {
        let p =
            match_error("checking whether compiler handles -Wimplicit-function-declaration... yes");
        assert!(p.is_none() || p.unwrap().key != "IMPLICIT_FUNCTION_DECLARATION");
    }

    #[test]
    fn lto_fat_objects_is_observation_not_error() {
        let line = "clang: warning: optimization flag '-ffat-lto-objects' is not supported [-Wignored-optimization-argument]";
        assert!(match_error(line).is_none());
        let p = match_obs(line);
        assert!(p.is_some());
        assert_eq!(p.unwrap().key, "LTO_FAT_OBJECTS_IGNORED");
    }

    #[test]
    fn lto_fat_objects_does_not_match_plain_command_line() {
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
        let p = match_error("clang-18: error: unknown argument '--debug-prefix-map=/build/x=.'; did you mean '-fdebug-prefix-map=/build/x=.'?");
        assert!(p.is_some());
        assert_eq!(p.unwrap().key, "UNSUPPORTED_COMPILER_FLAG");
    }

    #[test]
    fn lto_auto_before_unsupported_flag() {
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
        let p = match_error(
            "configure: error: could not find a working compiler, see config.log for details",
        );
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
