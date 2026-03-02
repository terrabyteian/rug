use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// Path to the terraform/tofu binary.
    pub binary: String,
    /// Maximum number of concurrent terraform processes.
    pub parallelism: usize,
    /// Directories to ignore during module discovery.
    pub ignore_dirs: Vec<String>,
    /// Show library modules (those without backend/lock signals) in TUI.
    pub show_library_modules: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            binary: String::new(), // populated by detect_binary()
            parallelism: 4,
            ignore_dirs: vec![
                ".terraform".into(),
                ".git".into(),
                "node_modules".into(),
                ".terragrunt-cache".into(),
            ],
            show_library_modules: false,
        }
    }
}

impl Config {
    /// Load config from `rug.toml` in CWD (if present), then detect binary.
    pub fn load() -> Result<Self> {
        let mut cfg = Self::load_file().unwrap_or_default();
        if cfg.binary.is_empty() {
            cfg.binary = detect_binary()?;
        }
        Ok(cfg)
    }

    fn load_file() -> Option<Self> {
        let path = PathBuf::from("rug.toml");
        let content = std::fs::read_to_string(path).ok()?;
        toml::from_str(&content).ok()
    }
}

/// Detect which terraform binary to use.
///
/// Priority:
/// 1. `TF_BINARY` env var
/// 2. `rug.toml` `binary` field (already applied by caller)
/// 3. `tofu` if on PATH
/// 4. `terraform` if on PATH
/// 5. Error
pub fn detect_binary() -> Result<String> {
    if let Ok(bin) = std::env::var("TF_BINARY") {
        if !bin.is_empty() {
            return Ok(bin);
        }
    }

    for candidate in &["tofu", "terraform"] {
        if which(candidate) {
            return Ok(candidate.to_string());
        }
    }

    bail!(
        "Neither 'tofu' nor 'terraform' found on PATH. \
        Install one or set TF_BINARY env var."
    );
}

fn which(bin: &str) -> bool {
    std::process::Command::new("which")
        .arg(bin)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}
