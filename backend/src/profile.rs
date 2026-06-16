//! Build profile loading and validation.
//!
//! A profile is a TOML file that describes the compiler configuration for a
//! batch of builds: which compiler to use, which Ubuntu series to target, and
//! any extra flags to inject via `DEB_*_APPEND` environment variables.
//!
//! Profiles are version-controlled alongside the codebase so that every batch
//! is fully reproducible: the profile name and its full TOML content are
//! snapshotted in the database at build time.

use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::path::Path;

/// Allowed values for `[[flags]].var`.  Using a whitelist so we don't
/// accidentally inject arbitrary environment variables into builds.
const ALLOWED_FLAG_VARS: &[&str] = &[
    "DEB_CFLAGS_APPEND",
    "DEB_CXXFLAGS_APPEND",
    "DEB_CPPFLAGS_APPEND",
    "DEB_LDFLAGS_APPEND",
];

// ---------------------------------------------------------------------------
// Deserialization types
// ---------------------------------------------------------------------------

/// A build profile loaded from a TOML file.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Profile {
    pub compiler: Compiler,
    pub target: Target,
    #[serde(default)]
    pub flags: Vec<Flag>,

    /// The profile name, derived from the filename (not part of the TOML).
    #[serde(skip)]
    pub name: String,

    /// The raw TOML content, for snapshotting into the database.
    #[serde(skip)]
    pub raw_content: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Compiler {
    #[serde(rename = "type")]
    pub compiler_type: CompilerType,
    pub version: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CompilerType {
    Clang,
    Gcc,
}

impl CompilerType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Clang => "clang",
            Self::Gcc => "gcc",
        }
    }
}

impl std::fmt::Display for CompilerType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Target {
    pub series: String,
}

/// A single flag to inject into the build environment.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Flag {
    /// The environment variable to append to (e.g. `DEB_CFLAGS_APPEND`).
    pub var: String,
    /// The flag value (e.g. `-gdwarf-4`).
    pub flag: String,
    /// Human-readable rationale for why this flag is needed.
    pub reason: String,
}

// ---------------------------------------------------------------------------
// Loading and validation
// ---------------------------------------------------------------------------

impl Profile {
    /// Load a profile from a TOML file, validate it, and return it.
    pub fn load(path: &Path) -> Result<Self> {
        let raw_content = std::fs::read_to_string(path)
            .with_context(|| format!("Failed to read profile: {}", path.display()))?;

        let name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .context("Profile path has no filename")?
            .to_string();

        let mut profile: Profile = toml::from_str(&raw_content)
            .with_context(|| format!("Failed to parse profile: {}", path.display()))?;

        profile.name = name;
        profile.raw_content = raw_content;

        profile.validate()?;
        Ok(profile)
    }

    /// Validate profile contents beyond what serde can check.
    fn validate(&self) -> Result<()> {
        if self.compiler.version.is_empty() {
            bail!("Profile {}: compiler.version must not be empty", self.name);
        }

        if self.target.series.is_empty() {
            bail!("Profile {}: target.series must not be empty", self.name);
        }

        // Validate flag variables are in the whitelist.
        for flag in &self.flags {
            if !ALLOWED_FLAG_VARS.contains(&flag.var.as_str()) {
                bail!(
                    "Profile {}: unknown flag variable '{}'. Allowed: {:?}",
                    self.name,
                    flag.var,
                    ALLOWED_FLAG_VARS
                );
            }
            if flag.flag.is_empty() {
                bail!("Profile {}: flag value must not be empty", self.name);
            }
        }

        Ok(())
    }

    /// Check that the target series is available for building.
    ///
    /// For `--chroot-mode=unshare`, sbuild uses debootstrap, so the series
    /// must have a debootstrap script.
    pub fn validate_series_available(&self) -> Result<()> {
        let script_path = format!("/usr/share/debootstrap/scripts/{}", self.target.series);
        if !Path::new(&script_path).exists() {
            bail!(
                "Series '{}' is not available for building: {} does not exist. \
                 Install debootstrap or check the series name.",
                self.target.series,
                script_path
            );
        }
        Ok(())
    }

    /// Collect profile flags grouped by environment variable name.
    ///
    /// Multiple flags targeting the same variable are concatenated with spaces.
    /// Returns a list of `(var_name, combined_value)` pairs.
    pub fn build_env_vars(&self) -> Vec<(String, String)> {
        use std::collections::BTreeMap;
        let mut map: BTreeMap<String, Vec<&str>> = BTreeMap::new();
        for flag in &self.flags {
            map.entry(flag.var.clone()).or_default().push(&flag.flag);
        }
        map.into_iter()
            .map(|(var, flags)| (var, flags.join(" ")))
            .collect()
    }

    /// Generate a batch name from the profile name and current timestamp.
    pub fn batch_name(&self) -> String {
        format!(
            "{}-{}",
            self.name,
            chrono::Utc::now().format("%Y%m%dT%H%M%S")
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_profile(content: &str) -> tempfile::NamedTempFile {
        let mut f = tempfile::Builder::new()
            .suffix(".toml")
            .tempfile()
            .unwrap();
        f.write_all(content.as_bytes()).unwrap();
        f
    }

    #[test]
    fn loads_clang_profile() {
        let f = write_profile(
            r#"
            [compiler]
            type = "clang"
            version = "18"
            [target]
            series = "noble"
            "#,
        );
        let p = Profile::load(f.path()).unwrap();
        assert_eq!(p.compiler.compiler_type, CompilerType::Clang);
        assert_eq!(p.compiler.version, "18");
        assert_eq!(p.target.series, "noble");
        assert!(p.flags.is_empty());
    }

    #[test]
    fn loads_gcc_profile() {
        let f = write_profile(
            r#"
            [compiler]
            type = "gcc"
            version = "13"
            [target]
            series = "noble"
            "#,
        );
        let p = Profile::load(f.path()).unwrap();
        assert_eq!(p.compiler.compiler_type, CompilerType::Gcc);
    }

    #[test]
    fn loads_profile_with_flags() {
        let f = write_profile(
            r#"
            [compiler]
            type = "clang"
            version = "18"
            [target]
            series = "noble"
            [[flags]]
            var = "DEB_CFLAGS_APPEND"
            flag = "-gdwarf-4"
            reason = "dwz compat"
            "#,
        );
        let p = Profile::load(f.path()).unwrap();
        assert_eq!(p.flags.len(), 1);
        assert_eq!(p.flags[0].flag, "-gdwarf-4");
    }

    #[test]
    fn rejects_unknown_flag_var() {
        let f = write_profile(
            r#"
            [compiler]
            type = "clang"
            version = "18"
            [target]
            series = "noble"
            [[flags]]
            var = "LD_PRELOAD"
            flag = "/tmp/evil.so"
            reason = "nope"
            "#,
        );
        assert!(Profile::load(f.path()).is_err());
    }

    #[test]
    fn rejects_unknown_fields() {
        let f = write_profile(
            r#"
            [compiler]
            type = "clang"
            version = "18"
            [target]
            series = "noble"
            [extra]
            foo = "bar"
            "#,
        );
        assert!(Profile::load(f.path()).is_err());
    }

    #[test]
    fn builds_env_vars() {
        let f = write_profile(
            r#"
            [compiler]
            type = "clang"
            version = "18"
            [target]
            series = "noble"
            [[flags]]
            var = "DEB_CFLAGS_APPEND"
            flag = "-gdwarf-4"
            reason = "dwz"
            [[flags]]
            var = "DEB_CXXFLAGS_APPEND"
            flag = "-gdwarf-4"
            reason = "dwz"
            "#,
        );
        let p = Profile::load(f.path()).unwrap();
        let env = p.build_env_vars();
        assert_eq!(env.len(), 2);
        assert_eq!(env[0], ("DEB_CFLAGS_APPEND".to_string(), "-gdwarf-4".to_string()));
    }

    #[test]
    fn empty_flags_no_env_vars() {
        let f = write_profile(
            r#"
            [compiler]
            type = "clang"
            version = "18"
            [target]
            series = "noble"
            "#,
        );
        let p = Profile::load(f.path()).unwrap();
        assert!(p.build_env_vars().is_empty());
    }
}
