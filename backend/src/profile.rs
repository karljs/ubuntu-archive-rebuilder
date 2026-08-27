//! Profile TOML loading and validation.

use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::path::Path;

// Whitelist: profiles must not inject arbitrary env vars into builds.
const ALLOWED_FLAG_VARS: &[&str] = &[
    "DEB_CFLAGS_APPEND",
    "DEB_CXXFLAGS_APPEND",
    "DEB_CPPFLAGS_APPEND",
    "DEB_LDFLAGS_APPEND",
];

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Profile {
    pub compiler: Compiler,
    pub target: Target,
    #[serde(default)]
    pub flags: Vec<Flag>,

    /// From the filename, not the TOML.
    #[serde(skip)]
    pub name: String,

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

impl std::str::FromStr for CompilerType {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s {
            "clang" => Ok(Self::Clang),
            "gcc" => Ok(Self::Gcc),
            other => Err(format!(
                "unknown compiler type '{other}' (expected: clang, gcc)"
            )),
        }
    }
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

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Flag {
    pub var: String,
    pub flag: String,
    pub reason: String,
}

impl Profile {
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

    fn validate(&self) -> Result<()> {
        if self.compiler.version.is_empty() {
            bail!("Profile {}: compiler.version must not be empty", self.name);
        }

        if self.target.series.is_empty() {
            bail!("Profile {}: target.series must not be empty", self.name);
        }

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

    /// unshare mode debootstraps the series.
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
        let mut f = tempfile::Builder::new().suffix(".toml").tempfile().unwrap();
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
        assert_eq!(
            env[0],
            ("DEB_CFLAGS_APPEND".to_string(), "-gdwarf-4".to_string())
        );
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
