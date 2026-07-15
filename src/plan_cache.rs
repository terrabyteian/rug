use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Instant;
use tempfile::TempDir;

pub struct PlanEntry {
    pub path: PathBuf,
    pub task_id: usize,
    pub created_at: Instant,
    /// Resource addresses this plan was scoped to via `-target=`. Empty for a
    /// full (non-targeted) plan.
    pub targets: Vec<String>,
}

impl PlanEntry {
    /// True if this plan was produced with `-target=` flags (partial plan).
    pub fn is_targeted(&self) -> bool {
        !self.targets.is_empty()
    }

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

    pub fn register(
        &mut self,
        module_path: PathBuf,
        plan_path: PathBuf,
        task_id: usize,
        targets: Vec<String>,
    ) {
        self.entries.insert(
            module_path,
            PlanEntry {
                path: plan_path,
                task_id,
                created_at: Instant::now(),
                targets,
            },
        );
    }

    pub fn get(&self, module_path: &Path) -> Option<&PlanEntry> {
        self.entries.get(module_path)
    }

    pub fn entry_count(&self) -> usize {
        self.entries.len()
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

    /// Drop every cached plan and delete its file. Safe to call while plans
    /// are being applied: the apply path takes ownership via `take()` before
    /// spawning, so in-flight applies hold their own file handle.
    pub fn clear(&mut self) {
        for (_, entry) in self.entries.drain() {
            Self::remove_file(&entry.path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_stores_targets_and_reports_targeted() {
        let mut cache = PlanCache::new().unwrap();
        let module = PathBuf::from("/tmp/mod");
        let plan = cache.plan_path_for(&module);

        // Full plan: empty targets → not targeted.
        cache.register(module.clone(), plan.clone(), 1, Vec::new());
        let entry = cache.get(&module).unwrap();
        assert!(entry.targets.is_empty());
        assert!(!entry.is_targeted());

        // Targeted plan: non-empty targets → targeted, stored verbatim.
        cache.register(
            module.clone(),
            plan,
            2,
            vec!["module.net".to_string(), "null_resource.a".to_string()],
        );
        let entry = cache.get(&module).unwrap();
        assert!(entry.is_targeted());
        assert_eq!(entry.targets.len(), 2);
        assert_eq!(entry.targets[0], "module.net");
    }
}
