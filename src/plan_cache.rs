use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Instant;
use tempfile::TempDir;

pub struct PlanEntry {
    pub path: PathBuf,
    pub created_at: Instant,
}

impl PlanEntry {
    pub fn age_str(&self) -> String {
        let secs = self.created_at.elapsed().as_secs();
        if secs < 60 {
            format!("{secs}s ago")
        } else if secs < 3600 {
            format!("{}m ago", secs / 60)
        } else {
            format!("{}h ago", secs / 3600)
        }
    }
}

/// Manages plan files in a secure temporary directory for the process lifetime.
///
/// All plan files are written under a session-specific temp dir and cleaned
/// up automatically on drop, so they never litter module directories.
pub struct PlanCache {
    /// Temp directory owned by this cache instance.
    dir: TempDir,
    entries: HashMap<PathBuf, PlanEntry>,
}

impl PlanCache {
    pub fn new() -> std::io::Result<Self> {
        let dir = tempfile::Builder::new().prefix("rug-").tempdir()?;
        Ok(Self {
            dir,
            entries: HashMap::new(),
        })
    }

    /// Deterministic plan file path for a given module.
    pub fn plan_path_for(&self, module_path: &Path) -> PathBuf {
        let sanitized: String = module_path
            .to_string_lossy()
            .chars()
            .map(|c| {
                if c.is_alphanumeric() || c == '-' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        self.dir.path().join(format!("{sanitized}.tfplan"))
    }

    pub fn register(&mut self, module_path: PathBuf, plan_path: PathBuf) {
        self.entries.insert(
            module_path,
            PlanEntry {
                path: plan_path,
                created_at: Instant::now(),
            },
        );
    }

    pub fn get(&self, module_path: &Path) -> Option<&PlanEntry> {
        self.entries.get(module_path)
    }

    /// Remove a plan entry from the cache and return its file path.
    ///
    /// Does NOT delete the file; callers delete it once any apply using it exits.
    pub fn take(&mut self, module_path: &Path) -> Option<PathBuf> {
        self.entries.remove(module_path).map(|e| e.path)
    }

    pub fn remove_file(path: &Path) {
        std::fs::remove_file(path).ok();
    }
}
