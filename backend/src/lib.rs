//! Rebuild Experiments Pipeline
//!
//! A tool for building Ubuntu archive packages with different compilers
//! (Clang or GCC) and analyzing the results.  Compiler configuration is
//! managed through versioned TOML profile files.

pub mod analyzer;
pub mod builder;
pub mod db;
pub mod export;
pub mod fetcher;
pub mod models;
pub mod profile;
