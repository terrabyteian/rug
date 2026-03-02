use anyhow::Result;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

use crate::config::Config;
use crate::module::{Module, ModuleKind};

/// Walk `root` and return discovered terraform modules.
///
/// Root modules: directories with `.tf` files AND backend/lock signals.
/// Library modules: directories with `.tf` files but no signals (excluded unless
/// `config.show_library_modules` is true).
pub fn discover(root: &Path, config: &Config) -> Result<Vec<Module>> {
    let ignore: HashSet<&str> = config.ignore_dirs.iter().map(|s| s.as_str()).collect();
    let mut modules = Vec::new();
    let mut visited: HashSet<PathBuf> = HashSet::new();

    let walker = WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| {
            // Skip ignored directories.
            if e.file_type().is_dir() {
                let name = e.file_name().to_string_lossy();
                return !ignore.contains(name.as_ref());
            }
            true
        });

    for entry in walker.flatten() {
        if !entry.file_type().is_dir() {
            continue;
        }
        let dir = entry.path().to_path_buf();
        if !visited.insert(dir.clone()) {
            continue;
        }

        let Some(kind) = classify_dir(&dir) else {
            continue;
        };

        if kind == ModuleKind::Library && !config.show_library_modules {
            continue;
        }

        modules.push(Module::new(dir, root, kind));
    }

    modules.sort_by(|a, b| a.display_name.cmp(&b.display_name));
    Ok(modules)
}

/// Classify a directory as Root, Library, or None (not a tf module).
fn classify_dir(dir: &Path) -> Option<ModuleKind> {
    let tf_files = collect_tf_files(dir);
    if tf_files.is_empty() {
        return None;
    }

    // Check for lock file or state file (strong signals).
    if dir.join(".terraform.lock.hcl").exists()
        || dir.join("terraform.tfstate").exists()
        || dir.join("terraform.tfstate.d").exists()
    {
        return Some(ModuleKind::Root);
    }

    // Check for backend block inside any .tf file.
    for tf in &tf_files {
        if file_has_backend(tf) {
            return Some(ModuleKind::Root);
        }
    }

    Some(ModuleKind::Library)
}

fn collect_tf_files(dir: &Path) -> Vec<PathBuf> {
    match std::fs::read_dir(dir) {
        Ok(entries) => entries
            .flatten()
            .filter(|e| {
                e.file_type().map(|t| t.is_file()).unwrap_or(false)
                    && e.path().extension().and_then(|x| x.to_str()) == Some("tf")
            })
            .map(|e| e.path())
            .collect(),
        Err(_) => vec![],
    }
}

/// Returns true if the file contains a `backend { ... }` block declaration.
fn file_has_backend(path: &Path) -> bool {
    let Ok(content) = std::fs::read_to_string(path) else {
        return false;
    };
    content.lines().any(|line| {
        let trimmed = line.trim();
        trimmed.starts_with("backend ") || trimmed == "backend{"
    })
}
