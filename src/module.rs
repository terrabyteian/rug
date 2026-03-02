use std::path::{Path, PathBuf};

/// Classification of a terraform module.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModuleKind {
    /// Has backend/state signals — deployable. Shown by default.
    Root,
    /// Only has `.tf` files but no backend/state signals — reusable library.
    Library,
}

/// A discovered terraform module directory.
#[derive(Debug, Clone)]
pub struct Module {
    /// Absolute path to the module directory.
    pub path: PathBuf,
    /// Human-friendly name (relative path from the discovery root).
    pub display_name: String,
    pub kind: ModuleKind,
}

impl Module {
    pub fn new(path: PathBuf, root: &Path, kind: ModuleKind) -> Self {
        let display_name = path
            .strip_prefix(root)
            .unwrap_or(&path)
            .to_string_lossy()
            .into_owned();
        Self { path, display_name, kind }
    }

    pub fn is_root(&self) -> bool {
        self.kind == ModuleKind::Root
    }
}
