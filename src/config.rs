use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
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
    /// Load config, preferring `<dir>/rug.toml`, falling back to `./rug.toml`,
    /// then defaults; detect the binary if it wasn't set by either file.
    pub fn load(dir: &Path) -> Result<Self> {
        let mut cfg = Self::load_file(&dir.join("rug.toml"))
            .or_else(|| Self::load_file(Path::new("rug.toml")))
            .unwrap_or_default();
        if cfg.binary.is_empty() {
            cfg.binary = detect_binary()?;
        }
        Ok(cfg)
    }

    fn load_file(path: &Path) -> Option<Self> {
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
    let env_override = std::env::var("TF_BINARY").ok();
    detect_binary_impl(env_override.as_deref(), binary_on_path)
}

/// Testable core of binary detection: takes the env override and a PATH
/// predicate as parameters so tests never need to mutate real env vars or
/// touch the real PATH.
fn detect_binary_impl(env_override: Option<&str>, on_path: impl Fn(&str) -> bool) -> Result<String> {
    if let Some(bin) = env_override {
        if !bin.is_empty() {
            return Ok(bin.to_string());
        }
    }

    for candidate in &["tofu", "terraform"] {
        if on_path(candidate) {
            return Ok(candidate.to_string());
        }
    }

    bail!(
        "Neither 'tofu' nor 'terraform' found on PATH. \
        Install one or set TF_BINARY env var."
    );
}

/// Pure PATH scan for an executable named `bin` — replaces shelling out to
/// `which`. On unix, requires a regular file with any exec bit set; on other
/// platforms, just checks the file exists (no exec-bit concept via std).
fn binary_on_path(bin: &str) -> bool {
    let Some(path_var) = std::env::var_os("PATH") else {
        return false;
    };

    std::env::split_paths(&path_var).any(|dir| is_executable(&dir.join(bin)))
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .map(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    path.is_file()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_override_wins() {
        let result = detect_binary_impl(Some("custom-tf"), |_| true);
        assert_eq!(result.unwrap(), "custom-tf");
    }

    #[test]
    fn empty_env_override_ignored() {
        let result = detect_binary_impl(Some(""), |bin| bin == "terraform");
        assert_eq!(result.unwrap(), "terraform");
    }

    #[test]
    fn tofu_preferred_over_terraform() {
        let result = detect_binary_impl(None, |_| true);
        assert_eq!(result.unwrap(), "tofu");
    }

    #[test]
    fn nothing_found_errors() {
        let result = detect_binary_impl(None, |_| false);
        assert!(result.is_err());
    }

    #[test]
    fn load_prefers_dir_rug_toml() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("rug.toml"), "binary = \"from-dir\"\n").unwrap();

        let cfg = Config::load(tmp.path()).unwrap();
        assert_eq!(cfg.binary, "from-dir");
    }
}
