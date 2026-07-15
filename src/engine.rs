//! Task execution engine: owns the task list, plan cache, and the
//! per-module run/queue bookkeeping needed to keep at most one task running
//! (and at most one more queued) per module directory.
//!
//! Shared by the TUI (via `App::engine`, polled non-blocking through
//! `drain_events`) and headless mode (polled blocking through `next_update`).

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{mpsc, Semaphore};

use crate::plan_cache::PlanCache;
use crate::runner::spawn_task;
use crate::task::{Task, TaskEvent, TaskEventReceiver, TaskEventSender, TaskStatus};

/// Parameters for a task that is queued behind a running task on the same module.
struct QueuedTask {
    task_id: usize,
    module_name: String,
    command: String,
    args: Vec<String>,
}

/// Everything needed to enqueue a new task.
pub struct TaskSpec {
    pub module_path: PathBuf,
    pub module_name: String,
    pub command: String,
    pub args: Vec<String>,
    /// For plan tasks: the path where the plan file is being written.
    pub plan_output_path: Option<PathBuf>,
    /// The `-target=` addresses this task was scoped to (any command). Empty =
    /// untargeted. For plan tasks it is also registered into the plan cache
    /// alongside `plan_output_path`.
    pub targets: Vec<String>,
    /// For apply tasks created from a cached plan: delete this plan after the
    /// task exits or is cancelled before it starts.
    pub cleanup_plan_path: Option<PathBuf>,
}

/// A processed engine event, returned to the caller so it can react (update
/// UI state, chain a follow-up op, etc.) without reaching into `tasks` itself
/// for the parts already handled here.
#[derive(Debug, Clone, Copy)]
pub enum EngineUpdate {
    // `task_id` isn't read by any caller (both the TUI and headless only
    // react to `Line`/`Finished`, destructuring this variant with `{ .. }`
    // rather than deleting it). Kept for Plan B: API symmetry with the other
    // variants, and a natural hook for a future "started" indicator.
    #[allow(dead_code)]
    Started { task_id: usize },
    Line { task_id: usize },
    Finished { task_id: usize, success: bool },
}

/// Owns task execution: the task list, the plan cache, and per-module
/// run/queue state. At most one task runs per module at a time; at most one
/// more is queued behind it (a new enqueue replaces and cancels any
/// previously-queued task for that module).
pub struct TaskEngine {
    pub tasks: Vec<Task>,
    /// In-session plan file cache; files live in a managed temp dir.
    pub plan_cache: PlanCache,
    next_task_id: usize,
    event_tx: TaskEventSender,
    event_rx: TaskEventReceiver,
    semaphore: Arc<Semaphore>,
    binary: String,
    /// Modules that currently have a running task.
    running_modules: HashSet<PathBuf>,
    /// At most one pending task per module, waiting for the running task to
    /// finish. A new enqueue replaces (and cancels) any existing entry.
    module_queues: HashMap<PathBuf, QueuedTask>,
}

impl TaskEngine {
    pub fn new(binary: String, parallelism: usize) -> std::io::Result<Self> {
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        Ok(Self {
            tasks: Vec::new(),
            plan_cache: PlanCache::new()?,
            next_task_id: 0,
            event_tx,
            event_rx,
            semaphore: Arc::new(Semaphore::new(parallelism)),
            binary,
            running_modules: HashSet::new(),
            module_queues: HashMap::new(),
        })
    }

    /// Push a task onto `self.tasks` and either start it immediately (if the
    /// module is idle) or slot it as the single pending task for the module
    /// (replacing and cancelling any previously-queued task). Returns the
    /// new task's id.
    pub fn push_task(&mut self, spec: TaskSpec) -> usize {
        let task_id = self.next_task_id;
        self.next_task_id += 1;

        self.tasks.push(Task {
            id: task_id,
            module_path: spec.module_path.clone(),
            module_name: spec.module_name.clone(),
            command: spec.command.clone(),
            status: TaskStatus::Pending,
            output_lines: Vec::new(),
            started_at: None,
            finished_at: None,
            plan_output_path: spec.plan_output_path,
            targets: spec.targets,
            cleanup_plan_path: spec.cleanup_plan_path,
            resource_counts: None,
            cancel_handle: None,
        });

        if !self.running_modules.contains(&spec.module_path) {
            // Module is idle: start immediately.
            self.running_modules.insert(spec.module_path.clone());
            let handle = spawn_task(
                task_id,
                spec.module_path,
                spec.module_name,
                self.binary.clone(),
                spec.command,
                spec.args,
                self.event_tx.clone(),
                self.semaphore.clone(),
            );
            if let Some(t) = self.tasks.iter_mut().find(|t| t.id == task_id) {
                t.cancel_handle = Some(handle);
            }
        } else if let Some(old) = self.module_queues.insert(
            spec.module_path,
            QueuedTask {
                task_id,
                module_name: spec.module_name,
                command: spec.command,
                args: spec.args,
            },
        ) {
            // Module is busy: slot as the single pending task, cancelling any
            // previously-queued task that hasn't started yet.
            if let Some(t) = self.tasks.iter_mut().find(|t| t.id == old.task_id) {
                t.status = TaskStatus::Cancelled;
                t.finished_at = Some(std::time::Instant::now());
                if let Some(path) = t.cleanup_plan_path.take() {
                    PlanCache::remove_file(&path);
                }
            }
        }

        task_id
    }

    /// Drain all currently-pending task events (non-blocking). Used by the TUI.
    pub fn drain_events(&mut self) -> Vec<EngineUpdate> {
        let mut updates = Vec::new();
        while let Ok(event) = self.event_rx.try_recv() {
            if let Some(update) = self.apply_event(event) {
                updates.push(update);
            }
        }
        updates
    }

    /// Wait for the next task event (blocking). Used by headless mode.
    /// Returns `None` once the event channel has closed (all senders dropped).
    pub async fn next_update(&mut self) -> Option<EngineUpdate> {
        loop {
            let event = self.event_rx.recv().await?;
            if let Some(update) = self.apply_event(event) {
                return Some(update);
            }
            // Stale-Finished events (see apply_event) produce no update;
            // keep waiting for the next one instead of returning None early.
        }
    }

    /// Apply a single raw task event to `self.tasks` (and plan cache / queue
    /// bookkeeping), returning the corresponding `EngineUpdate` — or `None`
    /// for a stale `Finished` event that should be skipped entirely.
    fn apply_event(&mut self, event: TaskEvent) -> Option<EngineUpdate> {
        match event {
            TaskEvent::Started { task_id } => {
                if let Some(task) = self.tasks.iter_mut().find(|t| t.id == task_id) {
                    task.status = TaskStatus::Running;
                    task.started_at = Some(std::time::Instant::now());
                }
                Some(EngineUpdate::Started { task_id })
            }
            TaskEvent::Line { task_id, line } => {
                if let Some(task) = self.tasks.iter_mut().find(|t| t.id == task_id) {
                    if matches!(task.command.as_str(), "plan" | "apply" | "destroy") {
                        if let Some(counts) = crate::task::parse_counts_from_line(&line) {
                            task.resource_counts = Some(counts);
                        }
                    }
                    task.output_lines.push(line);
                }
                Some(EngineUpdate::Line { task_id })
            }
            TaskEvent::Finished { task_id, success } => {
                // Skip stale Finished events for tasks that were already
                // fully cancelled (e.g. task completed just before SIGINT).
                // Cancelling tasks are NOT skipped — we need the event to
                // transition them to Cancelled and clean up the module queue.
                // This check (and the early return) must happen before ANY
                // bookkeeping below, including module-queue dequeue.
                let already_cancelled = self
                    .tasks
                    .iter()
                    .find(|t| t.id == task_id)
                    .map(|t| t.status == TaskStatus::Cancelled)
                    .unwrap_or(false);
                if already_cancelled {
                    return None;
                }

                let is_cancelling = self
                    .tasks
                    .iter()
                    .find(|t| t.id == task_id)
                    .map(|t| t.status == TaskStatus::Cancelling)
                    .unwrap_or(false);

                // Collect plan info and module path before the mutable borrow below.
                let (plan_info, module_path) = match self.tasks.iter().find(|t| t.id == task_id) {
                    Some(t) => {
                        let plan = if success && !is_cancelling {
                            t.plan_output_path.as_ref().map(|p| {
                                (t.module_path.clone(), p.clone(), t.id, t.targets.clone())
                            })
                        } else {
                            None
                        };
                        (plan, Some(t.module_path.clone()))
                    }
                    None => (None, None),
                };

                let mut cleanup_plan_path = None;
                if let Some(task) = self.tasks.iter_mut().find(|t| t.id == task_id) {
                    cleanup_plan_path = task.cleanup_plan_path.take();
                    task.cancel_handle = None;
                    task.status = if is_cancelling {
                        TaskStatus::Cancelled
                    } else if success {
                        TaskStatus::Success
                    } else {
                        TaskStatus::Failed
                    };
                    task.finished_at = Some(std::time::Instant::now());
                }

                // Register the plan file now that the mutable borrow is released.
                if let Some((mp, plan_path, plan_task_id, plan_targets)) = plan_info {
                    self.plan_cache
                        .register(mp, plan_path, plan_task_id, plan_targets);
                }

                if let Some(path) = cleanup_plan_path {
                    PlanCache::remove_file(&path);
                }

                // Dequeue the next task for this module, or mark it idle.
                if let Some(path) = module_path {
                    if let Some(queued) = self.module_queues.remove(&path) {
                        let handle = spawn_task(
                            queued.task_id,
                            path.clone(),
                            queued.module_name,
                            self.binary.clone(),
                            queued.command,
                            queued.args,
                            self.event_tx.clone(),
                            self.semaphore.clone(),
                        );
                        if let Some(t) = self.tasks.iter_mut().find(|t| t.id == queued.task_id) {
                            t.cancel_handle = Some(handle);
                        }
                    } else {
                        self.running_modules.remove(&path);
                    }
                }

                Some(EngineUpdate::Finished { task_id, success })
            }
        }
    }

    /// Cancel a single task by ID.
    ///
    /// - Queued (not yet spawned): removed from the module queue immediately.
    /// - Pending/Running (spawned, first call): sends SIGINT for graceful
    ///   shutdown; status becomes `Cancelling`. Module cleanup happens when
    ///   the process actually exits (via the `Finished` event in `apply_event`).
    /// - Cancelling (spawned, second call): sends SIGKILL immediately.
    pub fn cancel_task(&mut self, task_id: usize) {
        let (status, module_path) = match self.tasks.iter().find(|t| t.id == task_id) {
            Some(t) => (t.status.clone(), t.module_path.clone()),
            None => return,
        };

        if status.is_terminal() {
            return;
        }

        // Case 1: task is queued (not yet spawned).
        if let Some(queued) = self.module_queues.get(&module_path) {
            if queued.task_id == task_id {
                self.module_queues.remove(&module_path);
                if let Some(t) = self.tasks.iter_mut().find(|t| t.id == task_id) {
                    t.status = TaskStatus::Cancelled;
                    t.finished_at = Some(std::time::Instant::now());
                    if let Some(path) = t.cleanup_plan_path.take() {
                        PlanCache::remove_file(&path);
                    }
                }
                return;
            }
        }

        // Case 2: already cancelling — escalate to SIGKILL.
        if status == TaskStatus::Cancelling {
            if let Some(t) = self.tasks.iter_mut().find(|t| t.id == task_id) {
                if let Some(handle) = t.cancel_handle.as_mut() {
                    handle.force_kill();
                }
            }
            return;
        }

        // Case 3: spawned and running — send SIGINT, enter Cancelling state.
        // Module queue cleanup is deferred until the Finished event arrives.
        if let Some(t) = self.tasks.iter_mut().find(|t| t.id == task_id) {
            if let Some(handle) = t.cancel_handle.as_mut() {
                handle.cancel();
            }
            t.status = TaskStatus::Cancelling;
        }
    }

    pub fn task(&self, id: usize) -> Option<&Task> {
        self.tasks.iter().find(|t| t.id == id)
    }

    /// True if any task is still Pending or Running (i.e. not yet terminal).
    pub fn has_active_tasks(&self) -> bool {
        self.tasks.iter().any(|t| !t.status.is_terminal())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// Same module, three tasks pushed back-to-back: the first runs
    /// immediately, the second is queued, and the third displaces (cancels)
    /// the second before it ever starts. After draining, the first and third
    /// should both complete successfully; the second stays Cancelled.
    #[cfg(unix)]
    #[tokio::test]
    async fn same_module_queue_replaces_displaced_task() {
        let tmp = tempfile::tempdir().unwrap();
        let module_path = tmp.path().to_path_buf();

        let mut engine = TaskEngine::new("echo".to_string(), 1).unwrap();

        let spec = |command: &str| TaskSpec {
            module_path: module_path.clone(),
            module_name: "m".to_string(),
            command: command.to_string(),
            args: Vec::new(),
            plan_output_path: None,
            targets: Vec::new(),
            cleanup_plan_path: None,
        };

        let a = engine.push_task(spec("a"));
        let b = engine.push_task(spec("b"));
        let c = engine.push_task(spec("c"));

        // b was displaced by c before it ever started running.
        assert_eq!(engine.task(b).unwrap().status, TaskStatus::Cancelled);
        assert_eq!(engine.task(c).unwrap().status, TaskStatus::Pending);

        let drain = async {
            while engine.has_active_tasks() {
                engine.next_update().await;
            }
        };
        tokio::time::timeout(Duration::from_secs(10), drain)
            .await
            .expect("engine did not settle within 10s");

        assert_eq!(engine.task(a).unwrap().status, TaskStatus::Success);
        assert_eq!(engine.task(c).unwrap().status, TaskStatus::Success);
        assert_eq!(engine.task(b).unwrap().status, TaskStatus::Cancelled);
    }
}
