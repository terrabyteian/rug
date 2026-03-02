#![allow(dead_code)]
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{mpsc, Semaphore};

use crate::config::Config;
use crate::lock::{read_lock_info, parse_lock_from_output};
use crate::module::Module;
use crate::plan_cache::PlanCache;
use crate::runner::spawn_task;
use crate::state::StateContent;
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
    /// Lock ID (for ForceUnlock confirmations).
    pub lock_id: Option<String>,
    /// Lock holder (for ForceUnlock confirmations).
    pub lock_who: Option<String>,
}

/// Which destructive operation is waiting for confirmation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfirmKind {
    Apply,
    Destroy,
    InitUpgrade,
    ForceUnlock,
}

impl ConfirmKind {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Apply => "APPLY",
            Self::Destroy => "DESTROY",
            Self::InitUpgrade => "INIT -UPGRADE",
            Self::ForceUnlock => "FORCE UNLOCK",
        }
    }
}

/// A destructive command staged for user confirmation.
#[derive(Debug, Clone)]
pub struct PendingConfirm {
    pub kind: ConfirmKind,
    pub targets: Vec<ConfirmTarget>,
}

/// Detail view for a single resource instance — the drill-down from the list.
pub struct ResourceDetail {
    /// Address displayed in the sub-title.
    pub address: String,
    /// Pre-formatted lines of the instance JSON body.
    pub lines: Vec<String>,
    /// Number of lines scrolled from the top.
    pub scroll: usize,
}

/// Which state-explorer operation is being confirmed / run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExplorerOpKind {
    Taint,
    StateRm,
    /// Targeted destroy: single command with all -target flags, not sequential.
    TargetedDestroy,
}

impl ExplorerOpKind {
    /// Terraform sub-command to pass to runner.
    pub fn command(self) -> &'static str {
        match self {
            Self::Taint          => "taint",
            Self::StateRm        => "state",
            Self::TargetedDestroy => "destroy",
        }
    }

    /// Extra args before the resource address (sequential ops only).
    pub fn pre_args(self) -> &'static [&'static str] {
        match self {
            Self::Taint          => &[],
            Self::StateRm        => &["rm"],
            Self::TargetedDestroy => &[],
        }
    }

    pub fn confirm_title(self) -> &'static str {
        match self {
            Self::Taint          => " Confirm Taint ",
            Self::StateRm        => " Confirm Remove from State ",
            Self::TargetedDestroy => " ⚠  Confirm Targeted Destroy ",
        }
    }

    pub fn confirm_verb(self) -> &'static str {
        match self {
            Self::Taint          => "Taint",
            Self::StateRm        => "Remove from state",
            Self::TargetedDestroy => "DESTROY (targeted)",
        }
    }

    pub fn progress_title(self) -> &'static str {
        match self {
            Self::Taint          => " Taint Progress ",
            Self::StateRm        => " State Remove Progress ",
            Self::TargetedDestroy => " Targeted Destroy ",
        }
    }

    pub fn result_title(self, all_ok: bool) -> &'static str {
        match (self, all_ok) {
            (Self::Taint,           true)  => " Taint Complete ",
            (Self::Taint,           false) => " Taint — Some Failed ",
            (Self::StateRm,         true)  => " State Remove Complete ",
            (Self::StateRm,         false) => " State Remove — Some Failed ",
            (Self::TargetedDestroy, true)  => " Targeted Destroy Complete ",
            (Self::TargetedDestroy, false) => " Targeted Destroy Failed ",
        }
    }
}

/// An explorer operation in progress (sequential, one address at a time).
pub struct PendingOp {
    pub kind: ExplorerOpKind,
    /// Remaining addresses (front = next).
    pub queue: Vec<String>,
    /// Currently-running: (task_id, address).
    pub running: Option<(usize, String)>,
    /// Completed entries: (address, success).
    pub done: Vec<(String, bool)>,
}

/// Result of a completed explorer operation, shown until dismissed.
pub struct OpResult {
    pub kind: ExplorerOpKind,
    pub entries: Vec<(String, bool)>,
}

/// State shown in the state-explorer for a single module.
pub struct StateExplorer {
    pub module_idx: usize,
    pub module_name: String,
    pub content: StateContent,
    /// Index into the *filtered* resource list that is currently highlighted.
    pub selected: usize,
    /// Current filter string (case-insensitive substring match against addresses).
    pub filter: String,
    /// Whether the filter input is actively receiving keystrokes.
    pub filter_active: bool,
    /// When `Some`, the view shows resource detail instead of the resource list.
    pub detail_view: Option<ResourceDetail>,
    /// Unfiltered resource indices that are multi-selected.
    pub multi_select: Vec<usize>,
    /// When `Some`, a confirmation dialog is shown for the given operation.
    pub op_confirm: Option<ExplorerOpKind>,
    /// Addresses staged for the pending confirmation.
    pub op_targets: Vec<String>,
    /// Operation currently in progress.
    pub pending_op: Option<PendingOp>,
    /// Result of the last operation (dismissed on any keypress).
    pub op_result: Option<OpResult>,
    /// Set briefly after a targeted plan is queued; dismissed on any keypress.
    pub plan_queued_notice: bool,
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
    /// Tasks currently multi-selected (task IDs).
    pub task_multi_select: Vec<usize>,
    /// Spinner frame counter, incremented each drain_events call (~10 Hz).
    /// Used by the UI to animate the Cancelling status indicator.
    pub spinner_tick: u8,
    /// Filter string for the module list.
    pub filter: String,
    pub filter_active: bool,
    /// Maximum directory depth to show (None = unlimited). Controlled by `[`/`]`.
    pub max_depth: Option<usize>,
    pub show_help: bool,
    /// Staged destructive command awaiting user confirmation.
    pub pending_confirm: Option<PendingConfirm>,
    /// Task IDs staged for cancel confirmation (`C` key). Empty = no dialog.
    pub pending_cancel_task: Vec<usize>,
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
    /// State explorer popup, open when `Some`.
    pub state_explorer: Option<StateExplorer>,
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
            task_multi_select: Vec::new(),
            spinner_tick: 0,
            filter: String::new(),
            filter_active: false,
            max_depth: None,
            show_help: false,
            pending_confirm: None,
            pending_cancel_task: Vec::new(),
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
            state_explorer: None,
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

    /// Toggle the currently highlighted task in/out of `task_multi_select`.
    pub fn toggle_task_select(&mut self) {
        let Some(task_id) = self.selected_task_id else { return };
        if let Some(pos) = self.task_multi_select.iter().position(|&id| id == task_id) {
            self.task_multi_select.remove(pos);
        } else {
            self.task_multi_select.push(task_id);
        }
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
            cancel_handle: None,
        });

        if !self.running_modules.contains(&module_path) {
            // Module is idle: start immediately.
            self.running_modules.insert(module_path.clone());
            let handle = spawn_task(
                task_id,
                module_path,
                module.display_name.clone(),
                self.config.binary.clone(),
                command.to_string(),
                args,
                self.event_tx.clone(),
                self.semaphore.clone(),
            );
            if let Some(t) = self.tasks.iter_mut().find(|t| t.id == task_id) {
                t.cancel_handle = Some(handle);
            }
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
                lock_id: None,
                lock_who: None,
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
                plan_age: None,
                lock_id: None,
                lock_who: None,
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
            match confirm.kind {
                ConfirmKind::Apply => {
                    let indices: Vec<usize> = confirm.targets.iter().map(|t| t.module_idx).collect();
                    self.enqueue_apply_for(&indices);
                }
                ConfirmKind::Destroy => {
                    let indices: Vec<usize> = confirm.targets.iter().map(|t| t.module_idx).collect();
                    self.enqueue_destroy_for(&indices);
                }
                ConfirmKind::InitUpgrade => {
                    let indices: Vec<usize> = confirm.targets.iter().map(|t| t.module_idx).collect();
                    self.enqueue_init_upgrade_for(&indices);
                }
                ConfirmKind::ForceUnlock => {
                    let pairs: Vec<(usize, String)> = confirm.targets
                        .into_iter()
                        .filter_map(|t| t.lock_id.map(|id| (t.module_idx, id)))
                        .collect();
                    self.enqueue_force_unlock_for(&pairs);
                }
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
                lock_id: None,
                lock_who: None,
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

    /// Detect a lock for a module by parsing the output of its most recent
    /// terminal task that contains a "Lock Info:" block.
    ///
    /// Used as a fallback for remote/S3 backends that don't write a local
    /// `.lock.info` file but do print the lock details to stdout/stderr when
    /// lock acquisition fails.
    fn detect_lock_from_tasks(&self, module_idx: usize) -> Option<crate::lock::LockInfo> {
        let module_path = &self.modules[module_idx].path;
        self.tasks.iter()
            .filter(|t| &t.module_path == module_path && t.status.is_terminal())
            .filter_map(|t| {
                let lock = parse_lock_from_output(&t.output_lines)?;
                Some((t.finished_at, lock))
            })
            .max_by_key(|(fa, _)| *fa)
            .map(|(_, lock)| lock)
    }

    /// Stage `force-unlock` for confirmation.
    ///
    /// Detects lock info from a local `.lock.info` file first; if not found,
    /// falls back to parsing the "Lock Info:" block from recent task output
    /// (works for any backend — S3, remote, etc.).
    /// If no lock is detected for any target, returns silently.
    pub fn request_force_unlock_confirm(&mut self) {
        let targets = self.target_indices();
        if targets.is_empty() { return; }

        let confirm_targets: Vec<ConfirmTarget> = targets
            .iter()
            .filter_map(|&i| self.modules.get(i).map(|m| (i, m)))
            .filter_map(|(i, m)| {
                let lock = read_lock_info(&m.path)
                    .or_else(|| self.detect_lock_from_tasks(i))?;
                Some(ConfirmTarget {
                    module_idx: i,
                    module_name: m.display_name.clone(),
                    plan_age: None,
                    lock_id: Some(lock.id),
                    lock_who: Some(lock.who),
                })
            })
            .collect();

        if confirm_targets.is_empty() { return; }

        self.pending_confirm = Some(PendingConfirm {
            kind: ConfirmKind::ForceUnlock,
            targets: confirm_targets,
        });
    }

    /// Enqueue `force-unlock -force <lock_id>` for each (module_idx, lock_id) pair.
    fn enqueue_force_unlock_for(&mut self, targets: &[(usize, String)]) {
        if targets.is_empty() { return; }
        let first_new_idx = self.tasks.len();
        for (idx, lock_id) in targets {
            self.push_task(*idx, "force-unlock", vec!["-force".to_string(), lock_id.clone()], None);
        }
        self.maybe_auto_select_task(first_new_idx);
    }

    pub fn cancel_confirm(&mut self) {
        self.pending_confirm = None;
    }

    /// Stage tasks for cancel confirmation.
    ///
    /// Targets `task_multi_select` if non-empty, otherwise the highlighted task.
    /// Filters out already-terminal tasks. No-op if nothing active remains.
    pub fn request_cancel_task_confirm(&mut self) {
        let candidates: Vec<usize> = if self.task_multi_select.is_empty() {
            self.selected_task_id.into_iter().collect()
        } else {
            self.task_multi_select.clone()
        };

        let active: Vec<usize> = candidates
            .into_iter()
            .filter(|&id| {
                self.tasks.iter()
                    .find(|t| t.id == id)
                    .map(|t| !t.status.is_terminal())
                    .unwrap_or(false)
            })
            .collect();

        if !active.is_empty() {
            self.pending_cancel_task = active;
        }
    }

    /// Execute cancellation for all staged task IDs, then clear staging + multi-select.
    pub fn cancel_staged_tasks(&mut self) {
        let ids = std::mem::take(&mut self.pending_cancel_task);
        for id in ids {
            self.cancel_task(id);
        }
        self.task_multi_select.clear();
    }

    /// Cancel a single task by ID.
    ///
    /// - Queued (not yet spawned): removed from the module queue immediately.
    /// - Pending/Running (spawned, first call): sends SIGINT for graceful
    ///   shutdown; status becomes `Cancelling`. Module cleanup happens when
    ///   the process actually exits (via the `Finished` event in drain_events).
    /// - Cancelling (spawned, second call): sends SIGKILL immediately.
    fn cancel_task(&mut self, task_id: usize) {
        let (status, module_path) = match self.tasks.iter().find(|t| t.id == task_id) {
            Some(t) => (t.status.clone(), t.module_path.clone()),
            None => return,
        };

        if status.is_terminal() { return; }

        // Case 1: task is queued (not yet spawned).
        if let Some(queued) = self.module_queues.get(&module_path) {
            if queued.task_id == task_id {
                self.module_queues.remove(&module_path);
                if let Some(t) = self.tasks.iter_mut().find(|t| t.id == task_id) {
                    t.status = TaskStatus::Cancelled;
                    t.finished_at = Some(std::time::Instant::now());
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

    // ── State explorer ───────────────────────────────────────────────────────

    /// Open the state explorer for the currently selected (or focused) module.
    pub fn open_state_explorer(&mut self) {
        let visible = self.visible_module_indices();
        let Some(&idx) = visible.get(self.selected_module) else { return };
        let module = &self.modules[idx];
        let content = crate::state::read_state(&module.path);
        self.state_explorer = Some(StateExplorer {
            module_idx: idx,
            module_name: module.display_name.clone(),
            content,
            selected: 0,
            filter: String::new(),
            filter_active: false,
            detail_view: None,
            multi_select: Vec::new(),
            op_confirm: None,
            op_targets: Vec::new(),
            pending_op: None,
            op_result: None,
            plan_queued_notice: false,
        });
    }

    pub fn close_state_explorer(&mut self) {
        self.state_explorer = None;
    }

    /// Move the selected resource in the state explorer by `delta` rows,
    /// operating on the currently filtered list.
    pub fn state_explorer_move(&mut self, delta: i32) {
        let Some(explorer) = &mut self.state_explorer else { return };
        if let StateContent::Resources(ref resources) = explorer.content {
            let count = explorer_filtered_count(resources, &explorer.filter);
            if count == 0 { return; }
            explorer.selected = (explorer.selected as i32 + delta)
                .clamp(0, count as i32 - 1) as usize;
        }
    }

    pub fn state_explorer_go_first(&mut self) {
        if let Some(explorer) = &mut self.state_explorer {
            explorer.selected = 0;
        }
    }

    pub fn state_explorer_go_last(&mut self) {
        if let Some(explorer) = &mut self.state_explorer {
            if let StateContent::Resources(ref resources) = explorer.content {
                let count = explorer_filtered_count(resources, &explorer.filter);
                if count > 0 {
                    explorer.selected = count - 1;
                }
            }
        }
    }

    pub fn state_explorer_activate_filter(&mut self) {
        if let Some(explorer) = &mut self.state_explorer {
            explorer.filter.clear();
            explorer.filter_active = true;
            explorer.selected = 0;
        }
    }

    /// Deactivate filter input mode. Keeps the filter string applied.
    pub fn state_explorer_deactivate_filter(&mut self) {
        if let Some(explorer) = &mut self.state_explorer {
            explorer.filter_active = false;
        }
    }

    /// Clear the filter string and deactivate filter mode.
    pub fn state_explorer_clear_filter(&mut self) {
        if let Some(explorer) = &mut self.state_explorer {
            explorer.filter.clear();
            explorer.filter_active = false;
            explorer.selected = 0;
        }
    }

    pub fn state_explorer_filter_push(&mut self, c: char) {
        if let Some(explorer) = &mut self.state_explorer {
            explorer.filter.push(c);
            self.clamp_state_explorer_selection();
        }
    }

    pub fn state_explorer_filter_pop(&mut self) {
        if let Some(explorer) = &mut self.state_explorer {
            explorer.filter.pop();
            self.clamp_state_explorer_selection();
        }
    }

    fn clamp_state_explorer_selection(&mut self) {
        let Some(explorer) = &mut self.state_explorer else { return };
        if let StateContent::Resources(ref resources) = explorer.content {
            let count = explorer_filtered_count(resources, &explorer.filter);
            if count == 0 {
                explorer.selected = 0;
            } else if explorer.selected >= count {
                explorer.selected = count - 1;
            }
        }
    }

    // ── Resource detail view ─────────────────────────────────────────────────

    /// Open the detail view for the currently selected filtered resource.
    pub fn open_resource_detail(&mut self) {
        // Gather data under an immutable borrow first.
        let result: Option<(String, Vec<String>)> = (|| {
            let explorer = self.state_explorer.as_ref()?;
            let StateContent::Resources(ref resources) = explorer.content else { return None };

            let fl = explorer.filter.to_lowercase();
            let filtered: Vec<&crate::state::StateResource> = if explorer.filter.is_empty() {
                resources.iter().collect()
            } else {
                resources.iter().filter(|r| r.address.to_lowercase().contains(&fl)).collect()
            };

            let resource = filtered.get(explorer.selected)?;
            let address = resource.address.clone();
            let json = serde_json::to_string_pretty(&resource.instance)
                .unwrap_or_else(|_| "{}".to_string());
            let lines = json.lines().map(|l| l.to_string()).collect();
            Some((address, lines))
        })();

        if let (Some(explorer), Some((address, lines))) = (&mut self.state_explorer, result) {
            explorer.detail_view = Some(ResourceDetail { address, lines, scroll: 0 });
        }
    }

    /// Close the detail view and return to the resource list.
    pub fn close_resource_detail(&mut self) {
        if let Some(explorer) = &mut self.state_explorer {
            explorer.detail_view = None;
        }
    }

    pub fn resource_detail_scroll(&mut self, delta: i32) {
        let Some(explorer) = &mut self.state_explorer else { return };
        let Some(detail) = &mut explorer.detail_view else { return };
        let max = detail.lines.len().saturating_sub(1);
        if delta < 0 {
            detail.scroll = detail.scroll.saturating_sub((-delta) as usize);
        } else {
            detail.scroll = (detail.scroll + delta as usize).min(max);
        }
    }

    pub fn resource_detail_go_first(&mut self) {
        if let Some(explorer) = &mut self.state_explorer {
            if let Some(detail) = &mut explorer.detail_view {
                detail.scroll = 0;
            }
        }
    }

    pub fn resource_detail_go_last(&mut self) {
        if let Some(explorer) = &mut self.state_explorer {
            if let Some(detail) = &mut explorer.detail_view {
                detail.scroll = detail.lines.len().saturating_sub(1);
            }
        }
    }

    // ── State explorer multi-select & taint ──────────────────────────────────

    /// Toggle the currently highlighted resource in the multi-select set.
    pub fn state_explorer_toggle_select(&mut self) {
        let Some(explorer) = &mut self.state_explorer else { return };
        let StateContent::Resources(ref resources) = explorer.content else { return };

        let fl = explorer.filter.to_lowercase();
        let real_idx = if explorer.filter.is_empty() {
            Some(explorer.selected)
        } else {
            resources
                .iter()
                .enumerate()
                .filter(|(_, r)| r.address.to_lowercase().contains(&fl))
                .nth(explorer.selected)
                .map(|(i, _)| i)
        };
        let Some(real_idx) = real_idx else { return };

        if let Some(pos) = explorer.multi_select.iter().position(|&i| i == real_idx) {
            explorer.multi_select.remove(pos);
        } else {
            explorer.multi_select.push(real_idx);
        }
    }

    pub fn state_explorer_clear_select(&mut self) {
        if let Some(explorer) = &mut self.state_explorer {
            explorer.multi_select.clear();
        }
    }

    /// Enqueue a targeted plan for the selected (or multi-selected) resources.
    /// Runs as a normal task (appears in the task list). No confirmation needed.
    pub fn enqueue_targeted_plan(&mut self) {
        let (module_idx, targets) = {
            let Some(explorer) = self.state_explorer.as_ref() else { return };
            let StateContent::Resources(ref resources) = explorer.content else { return };

            let fl = explorer.filter.to_lowercase();
            let targets: Vec<String> = if explorer.multi_select.is_empty() {
                let filtered: Vec<&crate::state::StateResource> = if explorer.filter.is_empty() {
                    resources.iter().collect()
                } else {
                    resources.iter().filter(|r| r.address.to_lowercase().contains(&fl)).collect()
                };
                match filtered.get(explorer.selected) {
                    Some(r) => vec![r.address.clone()],
                    None => return,
                }
            } else {
                explorer.multi_select.iter()
                    .filter_map(|&i| resources.get(i))
                    .map(|r| r.address.clone())
                    .collect()
            };

            (explorer.module_idx, targets)
        };

        if targets.is_empty() { return; }

        let plan_path = self.plan_cache.plan_path_for(&self.modules[module_idx].path);
        let mut args = vec!["-out".to_string(), plan_path.to_string_lossy().into_owned()];
        for addr in &targets {
            args.push(format!("-target={}", addr));
        }
        let first_new_idx = self.tasks.len();
        self.push_task(module_idx, "plan", args, Some(plan_path));
        self.maybe_auto_select_task(first_new_idx);
        if let Some(explorer) = self.state_explorer.as_mut() {
            explorer.plan_queued_notice = true;
        }
    }

    /// Stage a confirmation for the given operation on the selected (or multi-selected) resources.
    pub fn request_op_confirm(&mut self, kind: ExplorerOpKind) {
        let Some(explorer) = &mut self.state_explorer else { return };
        let StateContent::Resources(ref resources) = explorer.content else { return };
        if resources.is_empty() { return; }

        let targets: Vec<String> = if explorer.multi_select.is_empty() {
            let fl = explorer.filter.to_lowercase();
            let filtered: Vec<&crate::state::StateResource> = if explorer.filter.is_empty() {
                resources.iter().collect()
            } else {
                resources.iter().filter(|r| r.address.to_lowercase().contains(&fl)).collect()
            };
            match filtered.get(explorer.selected) {
                Some(r) => vec![r.address.clone()],
                None => return,
            }
        } else {
            explorer.multi_select.iter()
                .filter_map(|&i| resources.get(i))
                .map(|r| r.address.clone())
                .collect()
        };

        if targets.is_empty() { return; }
        explorer.op_targets = targets;
        explorer.op_confirm = Some(kind);
    }

    pub fn cancel_op_confirm(&mut self) {
        if let Some(explorer) = &mut self.state_explorer {
            explorer.op_confirm = None;
            explorer.op_targets.clear();
        }
    }

    /// Begin executing the confirmed operation: start the first task, set up PendingOp.
    pub fn start_op(&mut self) {
        let (module_idx, kind, mut targets) = {
            let Some(explorer) = self.state_explorer.as_mut() else { return };
            let Some(kind) = explorer.op_confirm.take() else { return };
            let targets = std::mem::take(&mut explorer.op_targets);
            (explorer.module_idx, kind, targets)
        };

        if targets.is_empty() { return; }

        match kind {
            ExplorerOpKind::Taint | ExplorerOpKind::StateRm => {
                // Sequential: run one address at a time, chaining via check_op_completion.
                let first = targets.remove(0);
                let remaining = targets;
                let mut args: Vec<String> = kind.pre_args().iter().map(|s| s.to_string()).collect();
                args.push(first.clone());
                let task_id = self.next_task_id;
                self.push_task(module_idx, kind.command(), args, None);
                if let Some(explorer) = self.state_explorer.as_mut() {
                    explorer.pending_op = Some(PendingOp {
                        kind,
                        queue: remaining,
                        running: Some((task_id, first)),
                        done: Vec::new(),
                    });
                    explorer.multi_select.clear();
                }
            }
            ExplorerOpKind::TargetedDestroy => {
                // Single batch: all -target flags in one command.
                let n = targets.len();
                let mut args = vec!["-auto-approve".to_string()];
                for addr in &targets {
                    args.push(format!("-target={}", addr));
                }
                let label = if n == 1 {
                    targets.remove(0)
                } else {
                    format!("{} targeted resources", n)
                };
                let task_id = self.next_task_id;
                self.push_task(module_idx, "destroy", args, None);
                if let Some(explorer) = self.state_explorer.as_mut() {
                    explorer.pending_op = Some(PendingOp {
                        kind,
                        queue: vec![],
                        running: Some((task_id, label)),
                        done: Vec::new(),
                    });
                    explorer.multi_select.clear();
                }
            }
        }
    }

    /// Called from drain_events when a task finishes: chains the next op or
    /// moves to the result view.
    fn check_op_completion(&mut self, task_id: usize, success: bool) {
        let (module_idx, kind) = {
            let Some(explorer) = self.state_explorer.as_ref() else { return };
            let Some(pt) = explorer.pending_op.as_ref() else { return };
            let Some((rid, _)) = pt.running.as_ref() else { return };
            if *rid != task_id { return; }
            (explorer.module_idx, pt.kind)
        };

        let next_addr = {
            let explorer = self.state_explorer.as_mut().unwrap();
            let pt = explorer.pending_op.as_mut().unwrap();
            let (_, addr) = pt.running.take().unwrap();
            pt.done.push((addr, success));
            if pt.queue.is_empty() { None } else { Some(pt.queue.remove(0)) }
        };

        if let Some(next) = next_addr {
            let mut args: Vec<String> = kind.pre_args().iter().map(|s| s.to_string()).collect();
            args.push(next.clone());
            let new_task_id = self.next_task_id;
            self.push_task(module_idx, kind.command(), args, None);
            let explorer = self.state_explorer.as_mut().unwrap();
            let pt = explorer.pending_op.as_mut().unwrap();
            pt.running = Some((new_task_id, next));
        } else {
            {
                let explorer = self.state_explorer.as_mut().unwrap();
                let done = explorer.pending_op.take().unwrap().done;
                explorer.op_result = Some(OpResult { kind, entries: done });
            }
            // Refresh the resource list so tainted/removed resources reflect
            // their new status when the result popup is dismissed.
            self.refresh_state_explorer();
        }
    }

    pub fn dismiss_op_result(&mut self) {
        if let Some(explorer) = &mut self.state_explorer {
            explorer.op_result = None;
        }
    }

    /// Re-read the state file for the current explorer module and update the
    /// resource list in-place. Clamps selection and clears multi-select.
    pub fn refresh_state_explorer(&mut self) {
        let module_path = {
            let Some(explorer) = self.state_explorer.as_ref() else { return };
            self.modules[explorer.module_idx].path.clone()
        };

        let content = crate::state::read_state(&module_path);

        if let Some(explorer) = self.state_explorer.as_mut() {
            explorer.content = content;
            explorer.multi_select.clear();
            let count = if let StateContent::Resources(ref r) = explorer.content {
                explorer_filtered_count(r, &explorer.filter)
            } else {
                0
            };
            if count == 0 {
                explorer.selected = 0;
            } else if explorer.selected >= count {
                explorer.selected = count - 1;
            }
        }
    }

    // ── Event processing ─────────────────────────────────────────────────────

    /// Drain pending task events (non-blocking).
    pub fn drain_events(&mut self) {
        self.spinner_tick = self.spinner_tick.wrapping_add(1);
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
                    // Skip stale Finished events for tasks that were already
                    // fully cancelled (e.g. task completed just before SIGINT).
                    // Cancelling tasks are NOT skipped — we need the event to
                    // transition them to Cancelled and clean up the module queue.
                    let already_cancelled = self.tasks.iter()
                        .find(|t| t.id == task_id)
                        .map(|t| t.status == TaskStatus::Cancelled)
                        .unwrap_or(false);
                    if already_cancelled { continue; }

                    let is_cancelling = self.tasks.iter()
                        .find(|t| t.id == task_id)
                        .map(|t| t.status == TaskStatus::Cancelling)
                        .unwrap_or(false);

                    // Collect plan info and module path before the mutable borrow below.
                    let (plan_info, module_path) = match self.tasks.iter().find(|t| t.id == task_id) {
                        Some(t) => {
                            let plan = if success && !is_cancelling {
                                t.plan_output_path.as_ref().map(|p| (t.module_path.clone(), p.clone()))
                            } else {
                                None
                            };
                            (plan, Some(t.module_path.clone()))
                        }
                        None => (None, None),
                    };

                    if let Some(task) = self.tasks.iter_mut().find(|t| t.id == task_id) {
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
                    if let Some((mp, plan_path)) = plan_info {
                        self.plan_cache.register(mp, plan_path);
                    }

                    // Dequeue the next task for this module, or mark it idle.
                    if let Some(path) = module_path {
                        if let Some(queued) = self.module_queues.remove(&path) {
                            let module = &self.modules[queued.module_idx];
                            let handle = spawn_task(
                                queued.task_id,
                                path.clone(),
                                module.display_name.clone(),
                                self.config.binary.clone(),
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

                    // Chain the next op task if one is waiting.
                    self.check_op_completion(task_id, success);
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

/// Count resources in `resources` that match `filter` (empty = all).
pub fn explorer_filtered_count(resources: &[crate::state::StateResource], filter: &str) -> usize {
    if filter.is_empty() {
        resources.len()
    } else {
        let fl = filter.to_lowercase();
        resources.iter().filter(|r| r.address.to_lowercase().contains(&fl)).count()
    }
}
