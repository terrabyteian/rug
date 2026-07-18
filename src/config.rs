use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

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
    /// Path of the `rug.toml` this config came from; `None` when using defaults.
    #[serde(skip)]
    pub source: Option<PathBuf>,
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
            source: None,
        }
    }
}

impl Config {
    /// Load config from the nearest `rug.toml` at or above `dir`, falling back to
    /// the nearest one at or above the current directory, then to defaults; detect
    /// the binary if the file didn't set one.
    pub fn load(dir: &Path) -> Result<Self> {
        let found = find_config_file(dir).or_else(|| {
            std::env::current_dir()
                .ok()
                .and_then(|cwd| find_config_file(&cwd))
        });

        let mut cfg = match &found {
            Some(path) => Self::from_file(path)?,
            None => Self::default(),
        };
        cfg.source = found;

        if cfg.binary.is_empty() {
            cfg.binary = detect_binary()?;
        }
        Ok(cfg)
    }

    /// Read and parse a `rug.toml` that is known to exist. Unlike discovery, a file
    /// that is present but unreadable or malformed is an error rather than a miss —
    /// otherwise a typo would silently hand the run to an ancestor's config.
    fn from_file(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path).with_context(|| path.display().to_string())?;
        toml::from_str(&content).with_context(|| path.display().to_string())
    }
}

/// Walk up from `start` to the nearest `rug.toml`, using `$HOME` as the ceiling.
fn find_config_file(start: &Path) -> Option<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    find_config_file_in(start, home.as_deref())
}

/// Testable core of config discovery: takes the home directory as a parameter so
/// tests can pin the ceiling inside a tempdir instead of depending on the real
/// `$HOME` (a tempdir lives outside `$HOME` with no repo above it, so an unbounded
/// walk would escape into the real filesystem).
///
/// The walk is bounded so a `rug.toml` in an unrelated ancestor can never silently
/// configure a run: it stops after the directory holding `.git` (the repo root),
/// after `home`, or at the filesystem root, whichever comes first. Each boundary
/// check runs *after* that directory's own candidate check, so the repo root's (or
/// home's) own `rug.toml` still counts.
fn find_config_file_in(start: &Path, home: Option<&Path>) -> Option<PathBuf> {
    let start = canonical(start);
    let home = home.map(canonical);

    for dir in start.ancestors() {
        let candidate = dir.join("rug.toml");
        if candidate.is_file() {
            return Some(candidate);
        }
        // `.git` is a file, not a directory, in worktrees and submodules.
        if dir.join(".git").exists() || home.as_deref() == Some(dir) {
            break;
        }
    }
    None
}

/// `Path::new(".").ancestors()` yields only `"."`, so a relative start would never
/// walk. Callers can't be trusted to have resolved it: main() canonicalizes
/// best-effort and keeps the raw path on failure.
fn canonical(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
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
fn detect_binary_impl(
    env_override: Option<&str>,
    on_path: impl Fn(&str) -> bool,
) -> Result<String> {
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

    /// Every discovery test pins `home` inside the tempdir; see
    /// `find_config_file_in` for why the real `$HOME` won't do.
    fn find(start: &Path, home: &Path) -> Option<PathBuf> {
        find_config_file_in(start, Some(home))
    }

    fn mkdir(base: &Path, rel: &str) -> PathBuf {
        let path = base.join(rel);
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    fn write_config(dir: &Path) -> PathBuf {
        let path = dir.join("rug.toml");
        std::fs::write(&path, "parallelism = 3\n").unwrap();
        canonical(&path)
    }

    #[test]
    fn finds_config_in_start_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = write_config(tmp.path());

        assert_eq!(find(tmp.path(), tmp.path()), Some(cfg));
    }

    #[test]
    fn walks_up_to_nearest_ancestor() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = write_config(tmp.path());
        let start = mkdir(tmp.path(), "a/b/c");

        assert_eq!(find(&start, tmp.path()), Some(cfg));
    }

    #[test]
    fn nearest_ancestor_wins() {
        let tmp = tempfile::tempdir().unwrap();
        write_config(tmp.path());
        let nearer = write_config(&mkdir(tmp.path(), "a"));
        let start = mkdir(tmp.path(), "a/b");

        assert_eq!(find(&start, tmp.path()), Some(nearer));
    }

    #[test]
    fn stops_at_repo_root() {
        let tmp = tempfile::tempdir().unwrap();
        write_config(tmp.path());
        let repo = mkdir(tmp.path(), "repo");
        std::fs::create_dir(repo.join(".git")).unwrap();
        let start = mkdir(tmp.path(), "repo/envs");

        assert_eq!(find(&start, tmp.path()), None);
    }

    #[test]
    fn repo_root_own_config_is_found() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = mkdir(tmp.path(), "repo");
        std::fs::create_dir(repo.join(".git")).unwrap();
        let cfg = write_config(&repo);
        let start = mkdir(tmp.path(), "repo/envs");

        assert_eq!(find(&start, tmp.path()), Some(cfg));
    }

    #[test]
    fn git_file_is_a_boundary() {
        let tmp = tempfile::tempdir().unwrap();
        write_config(tmp.path());
        let repo = mkdir(tmp.path(), "repo");
        // Worktrees and submodules use a `.git` file rather than a directory.
        std::fs::write(repo.join(".git"), "gitdir: /elsewhere\n").unwrap();
        let start = mkdir(tmp.path(), "repo/envs");

        assert_eq!(find(&start, tmp.path()), None);
    }

    #[test]
    fn stops_at_home() {
        let tmp = tempfile::tempdir().unwrap();
        write_config(tmp.path());
        let home = mkdir(tmp.path(), "h");
        let start = mkdir(tmp.path(), "h/x");

        assert_eq!(find(&start, &home), None);
    }

    #[test]
    fn home_own_config_is_found() {
        let tmp = tempfile::tempdir().unwrap();
        let home = mkdir(tmp.path(), "h");
        let cfg = write_config(&home);
        let start = mkdir(tmp.path(), "h/x");

        assert_eq!(find(&start, &home), Some(cfg));
    }

    #[test]
    fn no_config_anywhere() {
        let tmp = tempfile::tempdir().unwrap();
        let start = mkdir(tmp.path(), "a/b");

        assert_eq!(find(&start, tmp.path()), None);
    }

    #[test]
    fn load_reads_config_from_start_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg_path = write_config(tmp.path());
        std::fs::write(&cfg_path, "binary = \"from-dir\"\n").unwrap();

        let cfg = Config::load(tmp.path()).unwrap();
        assert_eq!(cfg.binary, "from-dir");
        assert_eq!(cfg.source, Some(cfg_path));
    }

    #[test]
    fn malformed_config_is_an_error() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("rug.toml"), "parallelism = \"four\"\n").unwrap();

        let err = Config::load(tmp.path()).unwrap_err();
        let msg = format!("{err:#}");
        assert!(
            msg.contains("rug.toml"),
            "error should name the file: {msg}"
        );
    }
}
