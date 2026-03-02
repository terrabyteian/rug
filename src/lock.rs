use std::path::{Path, PathBuf};
use serde::Deserialize;
use crate::state::resolve_state_path;

#[derive(Debug, Clone, Deserialize)]
pub struct LockInfo {
    #[serde(rename = "ID")]        pub id: String,
    #[serde(rename = "Who")]       pub who: String,
    #[serde(rename = "Operation")] #[allow(dead_code)] pub operation: String,
}

/// Read the lock file for a module using a local-backend `.lock.info` file.
///
/// Looks for `<statefile>.lock.info` next to the module's state file.
/// Returns `None` if no state file is found or no lock file exists.
pub fn read_lock_info(module_path: &Path) -> Option<LockInfo> {
    let state_path = resolve_state_path(module_path)?;
    let mut p = state_path.into_os_string();
    p.push(".lock.info");
    let content = std::fs::read_to_string(PathBuf::from(p)).ok()?;
    serde_json::from_str(&content).ok()
}

/// Parse lock information from tofu/terraform task output lines.
///
/// Works for any backend: whenever locking fails, tofu prints a "Lock Info:"
/// block to stdout/stderr containing the ID, Who, and Operation fields.
/// Scans from the end so it picks up the most recent lock error if there are
/// multiple in the same output buffer.
pub fn parse_lock_from_output(lines: &[String]) -> Option<LockInfo> {
    // Scan from the end to find the last "Lock Info:" marker.
    let lock_pos = lines.iter().rposition(|l| strip_border(l).contains("Lock Info:"))?;

    let mut id = None;
    let mut who = String::new();
    let mut operation = String::new();

    for line in &lines[lock_pos + 1..] {
        let clean = strip_border(line);
        let trimmed = clean.trim();
        if let Some(v) = trimmed.strip_prefix("ID:") {
            id = Some(v.trim().to_string());
        } else if let Some(v) = trimmed.strip_prefix("Who:") {
            who = v.trim().to_string();
        } else if let Some(v) = trimmed.strip_prefix("Operation:") {
            operation = v.trim().to_string();
        }
        if id.is_some() && !who.is_empty() && !operation.is_empty() {
            break;
        }
    }

    Some(LockInfo { id: id?, who, operation })
}

/// Strip ANSI escape codes and the box-drawing border (`│`, `╷`, `╵`) that
/// tofu/terraform prints around error messages.
fn strip_border(s: &str) -> String {
    let no_ansi = strip_ansi(s);
    no_ansi
        .trim_start_matches(|c: char| matches!(c, '\u{2502}' | '\u{2577}' | '\u{2575}' | ' '))
        .to_string()
}

fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\x1b' && chars.peek() == Some(&'[') {
            chars.next();
            for c in chars.by_ref() {
                if c.is_ascii_alphabetic() { break; }
            }
        } else {
            out.push(ch);
        }
    }
    out
}
