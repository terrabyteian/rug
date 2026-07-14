use crate::state::resolve_state_path;
use crate::util::strip_ansi;
use serde::Deserialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Deserialize)]
pub struct LockInfo {
    #[serde(rename = "ID")]
    pub id: String,
    #[serde(rename = "Who")]
    pub who: String,
    #[serde(rename = "Operation")]
    #[allow(dead_code)]
    pub operation: String,
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
    let lock_pos = lines
        .iter()
        .rposition(|l| strip_border(l).contains("Lock Info:"))?;

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

    Some(LockInfo {
        id: id?,
        who,
        operation,
    })
}

/// Strip ANSI escape codes and the box-drawing border (`│`, `╷`, `╵`) that
/// tofu/terraform prints around error messages.
fn strip_border(s: &str) -> String {
    let no_ansi = strip_ansi(s);
    no_ansi
        .trim_start_matches(['\u{2502}', '\u{2577}', '\u{2575}', ' '])
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(raw: &[&str]) -> Vec<String> {
        raw.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn full_block_parses_id_and_who() {
        let out = lines(&[
            "│ Error: Error acquiring the state lock",
            "│",
            "│ Lock Info:",
            "│   ID:        abc-123",
            "│   Path:      terraform.tfstate",
            "│   Operation: OperationTypePlan",
            "│   Who:       user@host",
        ]);
        let info = parse_lock_from_output(&out).unwrap();
        assert_eq!(info.id, "abc-123");
        assert_eq!(info.who, "user@host");
        assert_eq!(info.operation, "OperationTypePlan");
    }

    #[test]
    fn no_lock_block_returns_none() {
        let out = lines(&["Apply complete!", "Resources: 1 added, 0 changed, 0 destroyed."]);
        assert!(parse_lock_from_output(&out).is_none());
    }

    #[test]
    fn missing_id_returns_none() {
        let out = lines(&[
            "│ Lock Info:",
            "│   Who:       user@host",
            "│   Operation: OperationTypePlan",
        ]);
        assert!(parse_lock_from_output(&out).is_none());
    }

    #[test]
    fn last_block_wins() {
        let out = lines(&[
            "│ Lock Info:",
            "│   ID:        first-id",
            "│   Who:       first@host",
            "│   Operation: OperationTypePlan",
            "some unrelated retry output",
            "│ Lock Info:",
            "│   ID:        second-id",
            "│   Who:       second@host",
            "│   Operation: OperationTypeApply",
        ]);
        let info = parse_lock_from_output(&out).unwrap();
        assert_eq!(info.id, "second-id");
        assert_eq!(info.who, "second@host");
    }
}

