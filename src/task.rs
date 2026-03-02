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

/// A single unit of work: run a terraform command in a module directory.
#[derive(Debug)]
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
