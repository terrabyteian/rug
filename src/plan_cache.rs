use std::collections::HashMap;
#[cfg(target_os = "linux")]
use std::os::fd::FromRawFd;
use std::os::fd::{AsRawFd, OwnedFd, RawFd};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

/// A nameless file holding terraform plan bytes.
///
/// Linux: a `memfd` — RAM-backed, never on any filesystem. Other unix: an
/// unlinked temp file — no directory entry, so the plan is unreachable by
/// path and the kernel reclaims it on last close, even if rug is SIGKILLed.
///
/// Clones share one underlying descriptor (refcounted); the fd closes when
/// the last clone drops. The fd stays CLOEXEC in this process so spawned
/// children never inherit it — except the one terraform child the plan is
/// for, which clears the flag post-fork (see `runner::spawn_task`).
#[derive(Clone)]
pub struct PlanHandle {
    fd: Arc<OwnedFd>,
}

impl PlanHandle {
    pub fn anonymous() -> std::io::Result<Self> {
        Ok(Self {
            fd: Arc::new(anonymous_fd()?),
        })
    }

    pub fn raw_fd(&self) -> RawFd {
        self.fd.as_raw_fd()
    }

    /// Path the terraform child uses to reach this fd. Valid in the child
    /// because fds are inherited at the same numbers across fork, and the
    /// number is pinned for as long as any clone is alive.
    pub fn dev_fd_path(&self) -> String {
        format!("/dev/fd/{}", self.raw_fd())
    }

    /// Rewind to offset 0. On macOS, opening /dev/fd/N duplicates the
    /// descriptor and SHARES its offset, so an apply must start from 0
    /// after the plan wrote up to EOF.
    pub fn rewind(&self) {
        unsafe {
            libc::lseek(self.raw_fd(), 0, libc::SEEK_SET);
        }
    }

    #[cfg(test)]
    pub fn ref_count(&self) -> usize {
        Arc::strong_count(&self.fd)
    }
}

#[cfg(target_os = "linux")]
fn anonymous_fd() -> std::io::Result<OwnedFd> {
    let fd = unsafe { libc::memfd_create(c"rug-plan".as_ptr(), libc::MFD_CLOEXEC) };
    if fd >= 0 {
        return Ok(unsafe { OwnedFd::from_raw_fd(fd) });
    }
    // memfd_create can be blocked (e.g. seccomp); an unlinked file still
    // beats a named one.
    unlinked_tempfile()
}

#[cfg(not(target_os = "linux"))]
fn anonymous_fd() -> std::io::Result<OwnedFd> {
    unlinked_tempfile()
}

fn unlinked_tempfile() -> std::io::Result<OwnedFd> {
    Ok(OwnedFd::from(tempfile::tempfile()?))
}

pub struct PlanEntry {
    pub handle: PlanHandle,
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

/// In-memory cache of plan handles for the process lifetime.
///
/// Plans live in anonymous fds (see `PlanHandle`), never as named files, so
/// there is nothing on disk to clean up: dropping an entry closes the fd.
#[derive(Default)]
pub struct PlanCache {
    entries: HashMap<PathBuf, PlanEntry>,
}

impl PlanCache {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(
        &mut self,
        module_path: PathBuf,
        handle: PlanHandle,
        task_id: usize,
        targets: Vec<String>,
    ) {
        self.entries.insert(
            module_path,
            PlanEntry {
                handle,
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

    /// Remove a plan entry from the cache and return its handle, transferring
    /// ownership to the apply that will consume it.
    pub fn take(&mut self, module_path: &Path) -> Option<PlanHandle> {
        self.entries.remove(module_path).map(|e| e.handle)
    }

    /// Drop every cached plan. Safe to call while plans are being applied:
    /// the apply path takes its own handle clone via `take()` before
    /// spawning, so the fd stays open until the last clone drops.
    pub fn clear(&mut self) {
        self.entries.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_stores_targets_and_reports_targeted() {
        let mut cache = PlanCache::new();
        let module = PathBuf::from("/tmp/mod");

        // Full plan: empty targets → not targeted.
        let handle = PlanHandle::anonymous().unwrap();
        cache.register(module.clone(), handle, 1, Vec::new());
        let entry = cache.get(&module).unwrap();
        assert!(entry.targets.is_empty());
        assert!(!entry.is_targeted());

        // Targeted plan: non-empty targets → targeted, stored verbatim.
        let handle = PlanHandle::anonymous().unwrap();
        cache.register(
            module.clone(),
            handle,
            2,
            vec!["module.net".to_string(), "null_resource.a".to_string()],
        );
        let entry = cache.get(&module).unwrap();
        assert!(entry.is_targeted());
        assert_eq!(entry.targets.len(), 2);
        assert_eq!(entry.targets[0], "module.net");
    }

    /// The anonymous fd is a real writable, seekable file: write through the
    /// raw fd, rewind, and read the bytes back.
    #[test]
    fn anonymous_handle_roundtrip() {
        let handle = PlanHandle::anonymous().unwrap();
        let payload = b"plan-bytes";
        let written = unsafe {
            libc::write(
                handle.raw_fd(),
                payload.as_ptr() as *const libc::c_void,
                payload.len(),
            )
        };
        assert_eq!(written, payload.len() as isize);

        handle.rewind();
        let mut buf = [0u8; 16];
        let read = unsafe {
            libc::read(
                handle.raw_fd(),
                buf.as_mut_ptr() as *mut libc::c_void,
                buf.len(),
            )
        };
        assert_eq!(read, payload.len() as isize);
        assert_eq!(&buf[..payload.len()], payload);
    }

    /// Encodes the in-flight-apply invariant: a handle taken from the cache
    /// keeps working after `clear()` — the fd only closes when the last
    /// clone drops.
    #[test]
    fn taken_handle_survives_clear() {
        let mut cache = PlanCache::new();
        let module = PathBuf::from("/tmp/mod");
        cache.register(
            module.clone(),
            PlanHandle::anonymous().unwrap(),
            1,
            Vec::new(),
        );

        let taken = cache.take(&module).unwrap();
        cache.clear();
        assert_eq!(cache.entry_count(), 0);

        let payload = b"still-alive";
        let written = unsafe {
            libc::write(
                taken.raw_fd(),
                payload.as_ptr() as *const libc::c_void,
                payload.len(),
            )
        };
        assert_eq!(written, payload.len() as isize);
    }
}
