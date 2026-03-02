#![allow(dead_code)]
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{mpsc, Semaphore};

use crate::config::Config;
use crate::module::Module;
use crate::plan_cache::PlanCache;
use crate::runner::spawn_task;
use crate::task::{Task, TaskEvent, TaskEventReceiver, TaskEventSender, TaskStatus};
use crate::discovery;

/// Parameters for a task that is queued behind a running task on the same module.
struct QueuedTask {
    task_id: usize,
    module_idx: usize,
    command: String,
    args: Vec<String>,
    plan_output_path: Option<PathBuf>,
}

/// Which pane currently has keyboard focus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Modules,
    Tasks,
    Output,
}

/// Which resize divider is being dragged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DragHandle {
    /// Vertical border between the modules pane and the right panel.
    Vertical,
    /// Horizontal border between the output pane and the tasks pane.
    Horizontal,
}

/// Per-module info shown in a confirmation overlay.
#[derive(Debug, Clone)]
pub struct ConfirmTarget {
    pub module_idx: usize,
    pub module_name: String,
    /// Human-readable plan age ("2m ago") or None if no prior plan.
    pub plan_age: Option<String>,
}

/// Which destructive operation is waiting for confirmation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfirmKind {
    Apply,
    Destroy,
    InitUpgrade,
}

impl ConfirmKind {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Apply => "APPLY",
            Self::Destroy => "DESTROY",
            Self::InitUpgrade => "INIT -UPGRADE",
        }
    }
}

/// A destructive command staged for user confirmation.
#[derive(Debug, Clone)]
pub struct PendingConfirm {
    pub kind: ConfirmKind,
    pub targets: Vec<ConfirmTarget>,
}

/// Shared application state, used by both TUI and headless modes.
pub struct App {
    pub config: Config,
    /// Root directory used for module discovery (re-used on refresh).
    pub root: PathBuf,
    pub modules: Vec<Module>,
    pub tasks: Vec<Task>,
    pub selected_module: usize,
    /// ID of the currently highlighted task (`task.id`), stable across re-sorts.
    pub selected_task_id: Option<usize>,
    /// How many lines the output pane is scrolled up from the bottom.
    /// 0 = auto-follow the tail. Resets when switching tasks.
    pub output_scroll: u16,
    pub focus: Focus,
    /// Modules currently multi-selected (indices into `modules`).
    pub multi_select: Vec<usize>,
    /// Visible-list position of the last Space press; used as the anchor for
    /// Ctrl+Space range selection.
    pub multi_select_anchor: Option<usize>,
    /// Filter string for the module list.
    pub filter: String,
    pub filter_active: bool,
    /// Maximum directory depth to show (None = unlimited). Controlled by `[`/`]`.
    pub max_depth: Option<usize>,
    pub show_help: bool,
    /// Staged destructive command awaiting user confirmation.
    pub pending_confirm: Option<PendingConfirm>,
    /// Set when the user pressed `q` while tasks were still running.
    /// The TUI stays alive until all tasks finish (or user force-quits).
    pub pending_quit: bool,
    /// When true the output pane fills the whole terminal (mouse capture off).
    pub output_fullscreen: bool,
    /// Width of the modules (left) pane in columns. None → default 25%.
    pub h_split_col: Option<u16>,
    /// Height of the output (top-right) pane in rows. None → default 65%.
    pub v_split_row: Option<u16>,
    /// Which resize divider is currently being dragged, if any.
    pub dragging: Option<DragHandle>,
    pub next_task_id: usize,
    pub event_tx: TaskEventSender,
    pub event_rx: TaskEventReceiver,
    pub semaphore: Arc<Semaphore>,
    /// In-session plan file cache; files live in a managed temp dir.
    pub plan_cache: PlanCache,
    /// Modules that currently have a running task.
    running_modules: HashSet<PathBuf>,
    /// At most one pending task per module, waiting for the running task to
    /// finish. A new enqueue replaces (and cancels) any existing entry.
    module_queues: HashMap<PathBuf, QueuedTask>,
}

impl App {
    pub fn new(config: Config, root: PathBuf, modules: Vec<Module>) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        let parallelism = config.parallelism;
        Self {
            config,
            root,
            modules,
            tasks: Vec::new(),
            selected_module: 0,
            selected_task_id: None,
            output_scroll: 0,
            focus: Focus::Modules,
            multi_select: Vec::new(),
            multi_select_anchor: None,
            filter: String::new(),
            filter_active: false,
            max_depth: None,
            show_help: false,
            pending_confirm: None,
            pending_quit: false,
            output_fullscreen: false,
            h_split_col: None,
            v_split_row: None,
            dragging: None,
            next_task_id: 0,
            event_tx: tx,
            event_rx: rx,
            semaphore: Arc::new(Semaphore::new(parallelism)),
            plan_cache: PlanCache::new(),
            running_modules: HashSet::new(),
            module_queues: HashMap::new(),
        }
    }

    // ── Layout splits ────────────────────────────────────────────────────────

    /// Width of the left (modules) pane, clamped to stay usable.
    pub fn effective_h_split(&self, total_width: u16) -> u16 {
        self.h_split_col
            .unwrap_or(total_width / 4)
            .clamp(5, total_width.saturating_sub(10))
    }

    /// Height of the top-right (output) pane, clamped to stay usable.
    pub fn effective_v_split(&self, total_height: u16) -> u16 {
        self.v_split_row
            .unwrap_or(total_height * 65 / 100)
            .clamp(4, total_height.saturating_sub(4))
    }

    // ── Module navigation ────────────────────────────────────────────────────

    /// Indices of modules visible after applying the current filter and depth limit.
    pub fn visible_module_indices(&self) -> Vec<usize> {
        let filter = self.filter.to_lowercase();
        self.modules
            .iter()
            .enumerate()
            .filter(|(_, m)| {
                (filter.is_empty() || m.display_name.to_lowercase().contains(&filter))
                    && self.max_depth.map_or(true, |d| module_depth(m) <= d)
            })
            .map(|(i, _)| i)
            .collect()
    }

    /// Deepest depth level present across all modules.
    fn max_module_depth(&self) -> usize {
        self.modules.iter().map(module_depth).max().unwrap_or(1)
    }

    pub fn decrease_depth(&mut self) {
        let current = self.max_depth.unwrap_or_else(|| self.max_module_depth());
        self.max_depth = Some(current.saturating_sub(1).max(1));
    }

    pub fn increase_depth(&mut self) {
        if let Some(n) = self.max_depth {
            if n + 1 >= self.max_module_depth() {
                self.max_depth = None; // back to unlimited
            } else {
                self.max_depth = Some(n + 1);
            }
        }
        // already unlimited — nothing to do
    }

    /// Returns true if the selection actually moved.
    pub fn move_module_selection(&mut self, delta: i32) -> bool {
        let count = self.visible_module_indices().len();
        if count == 0 { return false; }
        let new = (self.selected_module as i32 + delta)
            .clamp(0, count as i32 - 1) as usize;
        let changed = new != self.selected_module;
        self.selected_module = new;
        changed
    }

    /// Returns true if the selection actually moved.
    pub fn move_task_selection(&mut self, delta: i32) -> bool {
        if self.tasks.is_empty() { return false; }
        let sorted = self.sorted_task_display();
        let current_pos = self.selected_task_id
            .and_then(|id| sorted.iter().position(|&vi| self.tasks[vi].id == id))
            .unwrap_or(0) as i32;
        let new_pos = (current_pos + delta)
            .clamp(0, sorted.len() as i32 - 1) as usize;
        if let Some(id) = sorted.get(new_pos).map(|&vi| self.tasks[vi].id) {
            if Some(id) != self.selected_task_id {
                self.set_selected_task(id);
                return true;
            }
        }
        false
    }

    fn set_selected_task(&mut self, id: usize) {
        self.selected_task_id = Some(id);
        self.output_scroll = 0;
    }

    /// Scroll the output pane. Positive = scroll up (see earlier lines).
    /// Clamped to 0 at the bottom and to the total line count at the top.
    /// Returns true if the scroll position actually changed.
    pub fn scroll_output(&mut self, delta: i32) -> bool {
        let max_scroll = self.current_output().len() as u16;
        let before = self.output_scroll;
        if delta < 0 {
            self.output_scroll = self.output_scroll.saturating_sub((-delta) as u16);
        } else {
            self.output_scroll = self.output_scroll
                .saturating_add(delta as u16)
                .min(max_scroll);
        }
        self.output_scroll != before
    }

    /// Returns indices into `self.tasks` in display order: most recently active first.
    ///
    /// Sort key per task (descending):
    ///   finished_at > started_at > task.id (newest enqueued first for pending)
    pub fn sorted_task_display(&self) -> Vec<usize> {
        let mut indices: Vec<usize> = (0..self.tasks.len()).collect();
        indices.sort_by(|&a, &b| {
            let ta = &self.tasks[a];
            let tb = &self.tasks[b];
            let time_a = ta.finished_at.or(ta.started_at);
            let time_b = tb.finished_at.or(tb.started_at);
            match (time_b, time_a) {
                (Some(t2), Some(t1)) => t2.cmp(&t1),       // newer finished/started first
                (Some(_), None)      => std::cmp::Ordering::Less,   // timed before untimed
                (None, Some(_))      => std::cmp::Ordering::Greater,
                (None, None)         => tb.id.cmp(&ta.id), // newer pending first
            }
        });
        indices
    }

    pub fn toggle_multi_select(&mut self) {
        let visible = self.visible_module_indices();
        let Some(&real_idx) = visible.get(self.selected_module) else { return };
        if let Some(pos) = self.multi_select.iter().position(|&i| i == real_idx) {
            self.multi_select.remove(pos);
        } else {
            self.multi_select.push(real_idx);
        }
        self.multi_select_anchor = Some(self.selected_module);
    }

    /// Add all visible modules between the last Space anchor and the cursor to
    /// the selection (Ctrl+Space range-select). Falls back to a plain toggle if
    /// there is no anchor yet.
    pub fn range_select(&mut self) {
        let Some(anchor) = self.multi_select_anchor else {
            self.toggle_multi_select();
            return;
        };
        let visible = self.visible_module_indices();
        let current = self.selected_module;
        let lo = anchor.min(current);
        let hi = (anchor.max(current)).min(visible.len().saturating_sub(1));
        for &real_idx in &visible[lo..=hi] {
            if !self.multi_select.contains(&real_idx) {
                self.multi_select.push(real_idx);
            }
        }
        self.multi_select_anchor = Some(current);
    }

    pub fn go_to_first(&mut self) {
        match self.focus {
            Focus::Modules => {
                self.selected_module = 0;
            }
            Focus::Tasks => {
                let sorted = self.sorted_task_display();
                if let Some(&vi) = sorted.first() {
                    let id = self.tasks[vi].id;
                    self.set_selected_task(id);
                }
            }
            Focus::Output => {
                self.output_scroll = self.current_output().len() as u16;
            }
        }
    }

    pub fn go_to_last(&mut self) {
        match self.focus {
            Focus::Modules => {
                let count = self.visible_module_indices().len();
                if count > 0 {
                    self.selected_module = count - 1;
                }
            }
            Focus::Tasks => {
                let sorted = self.sorted_task_display();
                if let Some(&vi) = sorted.last() {
                    let id = self.tasks[vi].id;
                    self.set_selected_task(id);
                }
            }
            Focus::Output => {
                self.output_scroll = 0; // tail-follow
            }
        }
    }

    pub fn cycle_focus(&mut self) {
        self.focus = match self.focus {
            Focus::Modules => Focus::Tasks,
            Focus::Tasks => Focus::Output,
            Focus::Output => Focus::Modules,
        };
    }

    /// Re-run module discovery against the original root directory and update
    /// the module list. Selection is clamped; multi-select is cleared since
    /// indices may have shifted.
    pub fn refresh_modules(&mut self) {
        let Ok(modules) = discovery::discover(&self.root, &self.config) else { return };
        self.modules = modules.into_iter().filter(|m| m.is_root()).collect();
        self.multi_select.clear();
        let visible_count = self.visible_module_indices().len();
        if visible_count == 0 {
            self.selected_module = 0;
        } else if self.selected_module >= visible_count {
            self.selected_module = visible_count - 1;
        }
    }

    // ── Target resolution ────────────────────────────────────────────────────

    /// Module indices that the next action should target.
    fn target_indices(&self) -> Vec<usize> {
        if self.multi_select.is_empty() {
            let visible = self.visible_module_indices();
            visible.get(self.selected_module).copied().into_iter().collect()
        } else {
            self.multi_select.clone()
        }
    }

    // ── Command enqueueing ───────────────────────────────────────────────────

    /// Enqueue `plan` for the current target modules, writing plan files into
    /// the managed temp dir so they can be reused by a subsequent apply.
    pub fn enqueue_plan(&mut self) {
        let targets = self.target_indices();
        if targets.is_empty() { return; }
        let first_new_idx = self.tasks.len();

        for idx in targets {
            let plan_path = self.plan_cache.plan_path_for(&self.modules[idx].path);
            let args = vec!["-out".to_string(), plan_path.to_string_lossy().into_owned()];
            self.push_task(idx, "plan", args, Some(plan_path));
        }

        self.maybe_auto_select_task(first_new_idx);
    }

    /// Enqueue a generic command (init, exec, etc.) for the current targets.
    pub fn enqueue_command(&mut self, command: &str, extra_args: Vec<String>) {
        let targets = self.target_indices();
        if targets.is_empty() { return; }
        let first_new_idx = self.tasks.len();

        for idx in targets {
            self.push_task(idx, command, extra_args.clone(), None);
        }

        self.maybe_auto_select_task(first_new_idx);
    }

    /// Enqueue apply for explicitly captured target indices (from a PendingConfirm).
    ///
    /// Per module: if a plan file exists in the cache, apply from that file;
    /// otherwise fall back to `-auto-approve`. The cache entry is removed so
    /// the UI stops advertising a stale plan, but the file itself stays on
    /// disk for terraform to read — the temp dir Drop handles final cleanup.
    fn enqueue_apply_for(&mut self, targets: &[usize]) {
        if targets.is_empty() { return; }
        let first_new_idx = self.tasks.len();

        for &idx in targets {
            let module_path = self.modules[idx].path.clone();
            // `take` removes the cache entry but does NOT delete the file.
            let args = if let Some(plan_path) = self.plan_cache.take(&module_path) {
                vec![plan_path.to_string_lossy().into_owned()]
            } else {
                vec!["-auto-approve".to_string()]
            };
            self.push_task(idx, "apply", args, None);
        }

        self.maybe_auto_select_task(first_new_idx);
    }

    /// Enqueue destroy for explicitly captured target indices.
    fn enqueue_destroy_for(&mut self, targets: &[usize]) {
        if targets.is_empty() { return; }
        let first_new_idx = self.tasks.len();
        for &idx in targets {
            self.push_task(idx, "destroy", vec!["-auto-approve".to_string()], None);
        }
        self.maybe_auto_select_task(first_new_idx);
    }

    /// Low-level: push a task onto `self.tasks` and either start it immediately
    /// (if the module is idle) or slot it as the single pending task for the
    /// module (replacing and cancelling any previously-queued task).
    fn push_task(
        &mut self,
        module_idx: usize,
        command: &str,
        args: Vec<String>,
        plan_output_path: Option<PathBuf>,
    ) {
        let module = &self.modules[module_idx];
        let task_id = self.next_task_id;
        self.next_task_id += 1;
        let module_path = module.path.clone();

        self.tasks.push(Task {
            id: task_id,
            module_path: module_path.clone(),
            module_name: module.display_name.clone(),
            command: command.to_string(),
            args: args.clone(),
            status: TaskStatus::Pending,
            output_lines: Vec::new(),
            started_at: None,
            finished_at: None,
            plan_output_path: plan_output_path.clone(),
            resource_counts: None,
        });

        if !self.running_modules.contains(&module_path) {
            // Module is idle: start immediately.
            self.running_modules.insert(module_path.clone());
            spawn_task(
                task_id,
                module_path,
                module.display_name.clone(),
                self.config.binary.clone(),
                command.to_string(),
                args,
                self.event_tx.clone(),
                self.semaphore.clone(),
            );
        } else {
            // Module is busy: slot as the single pending task, cancelling any
            // previously-queued task that hasn't started yet.
            if let Some(old) = self.module_queues.insert(module_path, QueuedTask {
                task_id,
                module_idx,
                command: command.to_string(),
                args,
                plan_output_path,
            }) {
                if let Some(t) = self.tasks.iter_mut().find(|t| t.id == old.task_id) {
                    t.status = TaskStatus::Cancelled;
                    t.finished_at = Some(std::time::Instant::now());
                }
            }
        }
    }

    fn maybe_auto_select_task(&mut self, first_new_idx: usize) {
        if self.selected_task_id.is_none() {
            if let Some(task) = self.tasks.get(first_new_idx) {
                self.set_selected_task(task.id);
            }
        }
    }

    // ── Confirmation flow ────────────────────────────────────────────────────

    /// Stage `apply` for confirmation, annotating each target with plan info.
    pub fn request_apply_confirm(&mut self) {
        let targets = self.target_indices();
        if targets.is_empty() { return; }

        let confirm_targets: Vec<ConfirmTarget> = targets
            .iter()
            .filter_map(|&i| self.modules.get(i).map(|m| (i, m)))
            .map(|(i, m)| ConfirmTarget {
                module_idx: i,
                module_name: m.display_name.clone(),
                plan_age: self.plan_cache.get(&m.path).map(|e| e.age_str()),
            })
            .collect();

        self.pending_confirm = Some(PendingConfirm {
            kind: ConfirmKind::Apply,
            targets: confirm_targets,
        });
    }

    /// Stage `destroy` for confirmation.
    pub fn request_destroy_confirm(&mut self) {
        let targets = self.target_indices();
        if targets.is_empty() { return; }

        let confirm_targets: Vec<ConfirmTarget> = targets
            .iter()
            .filter_map(|&i| self.modules.get(i).map(|m| (i, m)))
            .map(|(i, m)| ConfirmTarget {
                module_idx: i,
                module_name: m.display_name.clone(),
                plan_age: None, // destroy doesn't use plan files
            })
            .collect();

        self.pending_confirm = Some(PendingConfirm {
            kind: ConfirmKind::Destroy,
            targets: confirm_targets,
        });
    }

    /// Execute the confirmed command. Call after user presses `y`.
    pub fn confirm_execute(&mut self) {
        if let Some(confirm) = self.pending_confirm.take() {
            let indices: Vec<usize> = confirm.targets.iter().map(|t| t.module_idx).collect();
            match confirm.kind {
                ConfirmKind::Apply => self.enqueue_apply_for(&indices),
                ConfirmKind::Destroy => self.enqueue_destroy_for(&indices),
                ConfirmKind::InitUpgrade => self.enqueue_init_upgrade_for(&indices),
            }
        }
    }

    /// Stage `init -upgrade` for confirmation.
    pub fn request_init_upgrade_confirm(&mut self) {
        let targets = self.target_indices();
        if targets.is_empty() { return; }

        let confirm_targets: Vec<ConfirmTarget> = targets
            .iter()
            .filter_map(|&i| self.modules.get(i).map(|m| (i, m)))
            .map(|(i, m)| ConfirmTarget {
                module_idx: i,
                module_name: m.display_name.clone(),
                plan_age: None,
            })
            .collect();

        self.pending_confirm = Some(PendingConfirm {
            kind: ConfirmKind::InitUpgrade,
            targets: confirm_targets,
        });
    }

    fn enqueue_init_upgrade_for(&mut self, targets: &[usize]) {
        if targets.is_empty() { return; }
        let first_new_idx = self.tasks.len();
        for &idx in targets {
            self.push_task(idx, "init", vec!["-upgrade".to_string()], None);
        }
        self.maybe_auto_select_task(first_new_idx);
    }

    pub fn cancel_confirm(&mut self) {
        self.pending_confirm = None;
    }

    // ── Event processing ─────────────────────────────────────────────────────

    /// Drain pending task events (non-blocking).
    pub fn drain_events(&mut self) {
        while let Ok(event) = self.event_rx.try_recv() {
            match event {
                TaskEvent::Started { task_id } => {
                    if let Some(task) = self.tasks.iter_mut().find(|t| t.id == task_id) {
                        task.status = TaskStatus::Running;
                        task.started_at = Some(std::time::Instant::now());
                    }
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
                }
                TaskEvent::Finished { task_id, success } => {
                    // Collect plan info and module path before the mutable borrow below.
                    let (plan_info, module_path) = match self.tasks.iter().find(|t| t.id == task_id) {
                        Some(t) => {
                            let plan = if success {
                                t.plan_output_path.as_ref().map(|p| (t.module_path.clone(), p.clone()))
                            } else {
                                None
                            };
                            (plan, Some(t.module_path.clone()))
                        }
                        None => (None, None),
                    };

                    if let Some(task) = self.tasks.iter_mut().find(|t| t.id == task_id) {
                        task.status = if success { TaskStatus::Success } else { TaskStatus::Failed };
                        task.finished_at = Some(std::time::Instant::now());
                    }

                    // Register the plan file now that the mutable borrow is released.
                    if let Some((mp, plan_path)) = plan_info {
                        self.plan_cache.register(mp, plan_path);
                    }

                    // Dequeue the next task for this module, or mark it idle.
                    if let Some(path) = module_path {
                        if let Some(queued) = self.module_queues.remove(&path) {
                            let module = &self.modules[queued.module_idx];
                            spawn_task(
                                queued.task_id,
                                path,
                                module.display_name.clone(),
                                self.config.binary.clone(),
                                queued.command,
                                queued.args,
                                self.event_tx.clone(),
                                self.semaphore.clone(),
                            );
                        } else {
                            self.running_modules.remove(&path);
                        }
                    }
                }
            }
        }
    }

    // ── Output pane ──────────────────────────────────────────────────────────

    fn output_task(&self) -> Option<&Task> {
        if let Some(id) = self.selected_task_id {
            return self.tasks.iter().find(|t| t.id == id);
        }
        // Fallback: most recently active task with output.
        self.sorted_task_display()
            .into_iter()
            .map(|vi| &self.tasks[vi])
            .find(|t| !t.output_lines.is_empty() || t.status == TaskStatus::Running)
    }

    pub fn current_output(&self) -> &[String] {
        self.output_task().map(|t| t.output_lines.as_slice()).unwrap_or_default()
    }

    pub fn output_title(&self) -> String {
        if let Some(task) = self.output_task() {
            return format!("Output [{}: {}]", task.module_name, task.command);
        }
        "Output".to_string()
    }

    pub fn all_tasks_done(&self) -> bool {
        self.tasks.iter().all(|t| t.status.is_terminal())
    }

    pub fn any_task_failed(&self) -> bool {
        self.tasks.iter().any(|t| t.status == TaskStatus::Failed)
    }

    /// Tasks that are still Pending or Running.
    pub fn active_tasks(&self) -> Vec<&Task> {
        self.tasks
            .iter()
            .filter(|t| !t.status.is_terminal())
            .collect()
    }
}

fn module_depth(module: &crate::module::Module) -> usize {
    std::path::Path::new(&module.display_name)
        .components()
        .count()
}
