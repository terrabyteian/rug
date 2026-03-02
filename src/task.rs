#![allow(dead_code)]
use std::path::PathBuf;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

/// Status of a task.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskStatus {
    Pending,
    Running,
    Success,
    Failed,
    Cancelled,
}

impl TaskStatus {
    pub fn icon(&self) -> &'static str {
        match self {
            Self::Pending => "○",
            Self::Running => "⟳",
            Self::Success => "✓",
            Self::Failed => "✗",
            Self::Cancelled => "⊘",
        }
    }

    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Success | Self::Failed | Self::Cancelled)
    }
}

/// Counts of resource operations extracted from a plan or apply summary line.
///
/// Covers all operation types that appear in the Plan:/Apply complete! summary:
///   add     — resource will be created
///   change  — resource will be updated in-place
///   destroy — resource will be deleted
///   import  — resource will be imported (import blocks, tf 1.5+ / tofu)
///   forget  — resource will be removed from state without destroying (tofu 1.7+)
///
/// Note: `moved` blocks do not appear in the Plan: summary line in current
/// terraform/OpenTofu — they are only shown in the plan body text.
///
/// `has_summary` is set when any terminal Plan/Apply/Destroy summary line was
/// parsed, allowing the UI to distinguish "saw 0 changes" from "no output yet".
#[derive(Debug, Clone, Default)]
pub struct ResourceCounts {
    pub add: u32,
    pub change: u32,
    pub destroy: u32,
    pub import: u32,
    pub forget: u32,
    /// A "No changes." line was seen.
    pub no_changes: bool,
    /// A terminal summary line was parsed (Plan:, Apply complete!, Destroy complete!).
    pub has_summary: bool,
}

impl ResourceCounts {
    pub fn all_zero(&self) -> bool {
        self.add == 0 && self.change == 0 && self.destroy == 0
            && self.import == 0 && self.forget == 0
    }
}

/// Try to extract resource operation counts from a single output line.
///
/// Handles:
///   "No changes. ..."
///   "Plan: N to add, N to change, N to destroy[, N to move][, N to import][, N to forget]."
///   "Apply complete! Resources: N added, N changed, N destroyed[, ...]."
///   "Destroy complete! Resources: N destroyed."
///
/// Returns `None` if the line is not a recognised summary line.
/// Later lines overwrite earlier ones so the last summary in the output wins.
pub fn parse_counts_from_line(line: &str) -> Option<ResourceCounts> {
    let clean = strip_ansi(line).trim().to_string();

    if clean.starts_with("No changes.") {
        return Some(ResourceCounts {
            no_changes: true,
            has_summary: true,
            ..Default::default()
        });
    }

    // "Plan: N to add, N to change, N to destroy[, ...]."
    if let Some(rest) = clean.strip_prefix("Plan: ") {
        let mut counts = parse_segment(rest, "to");
        counts.has_summary = true;
        return Some(counts);
    }

    // "Apply complete! Resources: N added, N changed, N destroyed[, ...]."
    // "Destroy complete! Resources: N destroyed."
    if let Some(pos) = clean.find("Resources: ") {
        let rest = &clean[pos + "Resources: ".len()..];
        let mut counts = parse_segment(rest, "");
        counts.has_summary = true;
        return Some(counts);
    }

    None
}

/// Parse a comma-separated list of count segments.
///
/// `sep` is the word between the number and the verb (e.g. "to" for plan).
/// Pass `""` for apply/destroy format where number immediately precedes the verb.
fn parse_segment(text: &str, sep: &str) -> ResourceCounts {
    let mut counts = ResourceCounts::default();
    for part in text.split(',') {
        let part = part.trim().trim_end_matches('.');
        let tokens: Vec<&str> = part.split_whitespace().collect();
        let (n_str, verb) = if sep.is_empty() {
            // "N verb" — apply/destroy format
            if tokens.len() >= 2 {
                (tokens[0], tokens[1])
            } else {
                continue;
            }
        } else {
            // "N sep verb" — plan format
            if tokens.len() >= 3 && tokens[1] == sep {
                (tokens[0], tokens[2])
            } else {
                continue;
            }
        };
        let Ok(n) = n_str.parse::<u32>() else { continue };
        match verb {
            "add" | "added" => counts.add = n,
            "change" | "changed" => counts.change = n,
            "destroy" | "destroyed" => counts.destroy = n,
            "import" | "imported" => counts.import = n,
            // OpenTofu: remove from state without destroying (tofu 1.7+)
            "forget" | "forgotten" | "remove" | "removed" => counts.forget = n,
            _ => {}
        }
    }
    counts
}

/// Strip ANSI escape sequences from a string.
/// Terraform and tofu emit ANSI colours even when piped on some configurations.
fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\x1b' && chars.peek() == Some(&'[') {
            chars.next(); // consume '['
            // consume until we hit the final byte (an ASCII letter)
            for c in chars.by_ref() {
                if c.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(ch);
        }
    }
    out
}

/// A single unit of work: run a terraform command in a module directory.
pub struct Task {
    pub id: usize,
    pub module_path: PathBuf,
    pub module_name: String,
    pub command: String,
    pub args: Vec<String>,
    pub status: TaskStatus,
    pub output_lines: Vec<String>,
    pub started_at: Option<Instant>,
    pub finished_at: Option<Instant>,
    /// For plan tasks: the path where the plan file is being written.
    /// Registered in PlanCache on successful completion.
    pub plan_output_path: Option<PathBuf>,
    /// Resource operation counts parsed from the plan/apply/destroy summary line.
    /// None until a summary line has been seen in the task output.
    pub resource_counts: Option<ResourceCounts>,
    /// Handle for aborting the spawned tokio task (killing the subprocess).
    /// None for tasks that are still in the module queue (not yet spawned).
    pub abort_handle: Option<tokio::task::AbortHandle>,
}

impl Task {
    pub fn elapsed(&self) -> Option<Duration> {
        match (self.started_at, self.finished_at) {
            (Some(s), Some(f)) => Some(f.duration_since(s)),
            (Some(s), None) => Some(s.elapsed()),
            _ => None,
        }
    }

    pub fn elapsed_str(&self) -> String {
        match self.elapsed() {
            Some(d) => format!("{}s", d.as_secs()),
            None => String::new(),
        }
    }
}

/// Messages streamed from a running task back to the app.
#[derive(Debug)]
pub enum TaskEvent {
    Started { task_id: usize },
    Line { task_id: usize, line: String },
    Finished { task_id: usize, success: bool },
}

pub type TaskEventSender = mpsc::UnboundedSender<TaskEvent>;
pub type TaskEventReceiver = mpsc::UnboundedReceiver<TaskEvent>;
