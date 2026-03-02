use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

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

/// Manages plan files in a temporary directory for the lifetime of the process.
///
/// All plan files are written under a session-specific temp dir and cleaned
/// up automatically on drop, so they never litter module directories.
pub struct PlanCache {
    /// Temp directory owned by this cache instance.
    pub dir: PathBuf,
    entries: HashMap<PathBuf, PlanEntry>,
}

impl PlanCache {
    pub fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("rug-{}", std::process::id()));
        std::fs::create_dir_all(&dir).ok();
        Self { dir, entries: HashMap::new() }
    }

    /// Deterministic plan file path for a given module.
    pub fn plan_path_for(&self, module_path: &Path) -> PathBuf {
        let sanitized: String = module_path
            .to_string_lossy()
            .chars()
            .map(|c| if c.is_alphanumeric() || c == '-' { c } else { '_' })
            .collect();
        self.dir.join(format!("{sanitized}.tfplan"))
    }

    pub fn register(&mut self, module_path: PathBuf, plan_path: PathBuf) {
        self.entries.insert(module_path, PlanEntry { path: plan_path, created_at: Instant::now() });
    }

    pub fn get(&self, module_path: &Path) -> Option<&PlanEntry> {
        self.entries.get(module_path)
    }

    /// Remove a plan entry from the cache and return its file path.
    ///
    /// Does NOT delete the file — the caller (or Drop on the whole cache) is
    /// responsible for that. This lets an in-flight apply still read the file
    /// while the cache no longer advertises it as current.
    pub fn take(&mut self, module_path: &Path) -> Option<PathBuf> {
        self.entries.remove(module_path).map(|e| e.path)
    }
}

impl Drop for PlanCache {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.dir).ok();
    }
}
