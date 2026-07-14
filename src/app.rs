use std::collections::HashSet;
use std::path::PathBuf;

use crate::config::Config;
use crate::discovery;
use crate::engine::{EngineUpdate, TaskEngine, TaskSpec};
use crate::lock::{parse_lock_from_output, read_lock_info};
use crate::module::Module;
use crate::state::StateContent;
use crate::task::{Task, TaskStatus};

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

/// A single overlay modal. At most one is active at a time.
pub enum Modal {
    Help,
    Confirm(PendingConfirm),
    /// Task IDs staged for cancel confirmation. Always non-empty.
    CancelTasks(Vec<usize>),
    ClearTasks,
    Reset,
    Quit,
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
            Self::Taint => "taint",
            Self::StateRm => "state",
            Self::TargetedDestroy => "destroy",
        }
    }

    /// Extra args before the resource address (sequential ops only).
    pub fn pre_args(self) -> &'static [&'static str] {
        match self {
            Self::Taint => &[],
            Self::StateRm => &["rm"],
            Self::TargetedDestroy => &[],
        }
    }

    pub fn confirm_title(self) -> &'static str {
        match self {
            Self::Taint => " Confirm Taint ",
            Self::StateRm => " Confirm Remove from State ",
            Self::TargetedDestroy => " ⚠  Confirm Targeted Destroy ",
        }
    }

    pub fn confirm_verb(self) -> &'static str {
        match self {
            Self::Taint => "Taint",
            Self::StateRm => "Remove from state",
            Self::TargetedDestroy => "DESTROY (targeted)",
        }
    }

    pub fn progress_title(self) -> &'static str {
        match self {
            Self::Taint => " Taint Progress ",
            Self::StateRm => " State Remove Progress ",
            Self::TargetedDestroy => " Targeted Destroy ",
        }
    }

    pub fn result_title(self, all_ok: bool) -> &'static str {
        match (self, all_ok) {
            (Self::Taint, true) => " Taint Complete ",
            (Self::Taint, false) => " Taint — Some Failed ",
            (Self::StateRm, true) => " State Remove Complete ",
            (Self::StateRm, false) => " State Remove — Some Failed ",
            (Self::TargetedDestroy, true) => " Targeted Destroy Complete ",
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

/// Heights of each scrollable pane, updated every draw pass.
/// Used to compute page-up/page-down scroll amounts.
#[derive(Debug, Default, Clone, Copy)]
pub struct PaneHeights {
    pub modules: u16,
    pub output: u16,
    pub tasks: u16,
    pub explorer: u16,
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
    /// Receiver for an in-flight background state load, if one is running.
    /// `content` is `Loading` for as long as this is `Some`.
    pub load_rx: Option<tokio::sync::oneshot::Receiver<StateContent>>,
}

impl StateExplorer {
    /// Resources in the current content, if any were successfully loaded.
    pub fn resources(&self) -> Option<&[crate::state::StateResource]> {
        match &self.content {
            StateContent::Resources(r) => Some(r),
            _ => None,
        }
    }

    /// Indices into `resources()` that match the current filter (all, if the
    /// filter is empty).
    pub fn filtered_indices(&self) -> Vec<usize> {
        let Some(resources) = self.resources() else {
            return Vec::new();
        };
        if self.filter.is_empty() {
            (0..resources.len()).collect()
        } else {
            let fl = self.filter.to_lowercase();
            resources
                .iter()
                .enumerate()
                .filter(|(_, r)| r.address.to_lowercase().contains(&fl))
                .map(|(i, _)| i)
                .collect()
        }
    }

    /// Number of resources currently visible under the filter.
    pub fn filtered_count(&self) -> usize {
        self.filtered_indices().len()
    }

    /// Real (unfiltered) resource index for the currently highlighted filtered row.
    pub fn selected_real_index(&self) -> Option<usize> {
        self.filtered_indices().get(self.selected).copied()
    }

    /// Addresses an operation should target: the multi-selected set
    /// (unfiltered — this ignores the active filter) if non-empty, otherwise
    /// just the currently highlighted filtered row.
    pub fn target_addresses(&self) -> Vec<String> {
        let Some(resources) = self.resources() else {
            return Vec::new();
        };
        if !self.multi_select.is_empty() {
            self.multi_select
                .iter()
                .filter_map(|&i| resources.get(i))
                .map(|r| r.address.clone())
                .collect()
        } else {
            self.selected_real_index()
                .and_then(|i| resources.get(i))
                .map(|r| r.address.clone())
                .into_iter()
                .collect()
        }
    }
}

/// TUI application state: module selection, UI modes, and a TaskEngine.
pub struct App {
    pub config: Config,
    /// Root directory used for module discovery (re-used on refresh).
    pub root: PathBuf,
    pub modules: Vec<Module>,
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
    /// The single active overlay modal, if any. Mutually exclusive by construction.
    pub modal: Option<Modal>,
    /// When true the output pane fills the whole terminal (mouse capture off).
    pub output_fullscreen: bool,
    /// When true, long lines in the output pane are soft-wrapped (fullscreen only).
    pub output_wrap: bool,
    /// Width of the modules (left) pane in columns. None → default 25%.
    pub h_split_col: Option<u16>,
    /// Height of the output (top-right) pane in rows. None → default 65%.
    pub v_split_row: Option<u16>,
    /// Which resize divider is currently being dragged, if any.
    pub dragging: Option<DragHandle>,
    /// Heights of scrollable panes from the last render pass (for page up/down).
    pub pane_heights: PaneHeights,
    /// Task execution engine: task list, plan cache, run/queue bookkeeping.
    pub engine: TaskEngine,
    /// State explorer popup, open when `Some`.
    pub state_explorer: Option<StateExplorer>,
}

impl App {
    pub fn new(config: Config, root: PathBuf, modules: Vec<Module>) -> std::io::Result<Self> {
        let parallelism = config.parallelism;
        let engine = TaskEngine::new(config.binary.clone(), parallelism)?;
        Ok(Self {
            config,
            root,
            modules,
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
            modal: None,
            output_fullscreen: false,
            output_wrap: false,
            h_split_col: None,
            v_split_row: None,
            dragging: None,
            pane_heights: PaneHeights::default(),
            engine,
            state_explorer: None,
        })
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
        if count == 0 {
            return false;
        }
        let new = (self.selected_module as i32 + delta).clamp(0, count as i32 - 1) as usize;
        let changed = new != self.selected_module;
        self.selected_module = new;
        changed
    }

    /// Returns true if the selection actually moved.
    pub fn move_task_selection(&mut self, delta: i32) -> bool {
        if self.engine.tasks.is_empty() {
            return false;
        }
        let sorted = self.sorted_task_display();
        let current_pos = self
            .selected_task_id
            .and_then(|id| sorted.iter().position(|&vi| self.engine.tasks[vi].id == id))
            .unwrap_or(0) as i32;
        let new_pos = (current_pos + delta).clamp(0, sorted.len() as i32 - 1) as usize;
        if let Some(id) = sorted.get(new_pos).map(|&vi| self.engine.tasks[vi].id) {
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
            self.output_scroll = self
                .output_scroll
                .saturating_add(delta as u16)
                .min(max_scroll);
        }
        self.output_scroll != before
    }

    /// Returns indices into `self.engine.tasks` in display order: most recently active first.
    ///
    /// Sort key per task (descending):
    ///   finished_at > started_at > task.id (newest enqueued first for pending)
    pub fn sorted_task_display(&self) -> Vec<usize> {
        let mut indices: Vec<usize> = (0..self.engine.tasks.len()).collect();
        indices.sort_by(|&a, &b| {
            let ta = &self.engine.tasks[a];
            let tb = &self.engine.tasks[b];
            let time_a = ta.finished_at.or(ta.started_at);
            let time_b = tb.finished_at.or(tb.started_at);
            match (time_b, time_a) {
                (Some(t2), Some(t1)) => t2.cmp(&t1), // newer finished/started first
                (Some(_), None) => std::cmp::Ordering::Less, // timed before untimed
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => tb.id.cmp(&ta.id), // newer pending first
            }
        });
        indices
    }

    pub fn toggle_multi_select(&mut self) {
        let visible = self.visible_module_indices();
        let Some(&real_idx) = visible.get(self.selected_module) else {
            return;
        };
        if let Some(pos) = self.multi_select.iter().position(|&i| i == real_idx) {
            self.multi_select.remove(pos);
        } else {
            self.multi_select.push(real_idx);
        }
        self.multi_select_anchor = Some(self.selected_module);
    }

    /// Toggle the currently highlighted task in/out of `task_multi_select`.
    pub fn toggle_task_select(&mut self) {
        let Some(task_id) = self.selected_task_id else {
            return;
        };
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
                    let id = self.engine.tasks[vi].id;
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
                    let id = self.engine.tasks[vi].id;
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
        let Ok(modules) = discovery::discover(&self.root, &self.config) else {
            return;
        };
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
            visible
                .get(self.selected_module)
                .copied()
                .into_iter()
                .collect()
        } else {
            self.multi_select.clone()
        }
    }

    // ── Command enqueueing ───────────────────────────────────────────────────

    /// Enqueue `plan` for the current target modules, writing plan files into
    /// the managed temp dir so they can be reused by a subsequent apply.
    pub fn enqueue_plan(&mut self) {
        let targets = self.target_indices();
        if targets.is_empty() {
            return;
        }

        for idx in targets {
            let plan_path = self.engine.plan_cache.plan_path_for(&self.modules[idx].path);
            let args = vec!["-out".to_string(), plan_path.to_string_lossy().into_owned()];
            self.push_task_for(idx, "plan", args, Some(plan_path), None);
        }

        self.maybe_auto_select_task();
    }

    /// Enqueue a generic command (init, exec, etc.) for the current targets.
    pub fn enqueue_command(&mut self, command: &str, extra_args: Vec<String>) {
        let targets = self.target_indices();
        if targets.is_empty() {
            return;
        }

        for idx in targets {
            self.push_task_for(idx, command, extra_args.clone(), None, None);
        }

        self.maybe_auto_select_task();
    }

    /// Enqueue apply for explicitly captured target indices (from a PendingConfirm).
    ///
    /// Per module: if a plan file exists in the cache, apply from that file;
    /// otherwise fall back to `-auto-approve`. The cache entry is removed so
    /// the UI stops advertising a stale plan, and the file is deleted after the
    /// apply process exits.
    fn enqueue_apply_for(&mut self, targets: &[usize]) {
        if targets.is_empty() {
            return;
        }

        for &idx in targets {
            let module_path = self.modules[idx].path.clone();
            // `take` removes the cache entry but does NOT delete the file.
            let (args, cleanup_plan_path) =
                if let Some(plan_path) = self.engine.plan_cache.take(&module_path) {
                    (
                        vec![plan_path.to_string_lossy().into_owned()],
                        Some(plan_path),
                    )
                } else {
                    (vec!["-auto-approve".to_string()], None)
                };
            self.push_task_for(idx, "apply", args, None, cleanup_plan_path);
        }

        self.maybe_auto_select_task();
    }

    /// Enqueue destroy for explicitly captured target indices.
    fn enqueue_destroy_for(&mut self, targets: &[usize]) {
        if targets.is_empty() {
            return;
        }
        for &idx in targets {
            self.push_task_for(
                idx,
                "destroy",
                vec!["-auto-approve".to_string()],
                None,
                None,
            );
        }
        self.maybe_auto_select_task();
    }

    /// Bridge from module-index-based call sites to the `TaskEngine`, which
    /// only knows about module path/name (not the `App`-level module list).
    fn push_task_for(
        &mut self,
        module_idx: usize,
        command: &str,
        args: Vec<String>,
        plan_output_path: Option<PathBuf>,
        cleanup_plan_path: Option<PathBuf>,
    ) -> usize {
        let module = &self.modules[module_idx];
        self.engine.push_task(TaskSpec {
            module_path: module.path.clone(),
            module_name: module.display_name.clone(),
            command: command.to_string(),
            args,
            plan_output_path,
            cleanup_plan_path,
        })
    }

    fn maybe_auto_select_task(&mut self) {
        // Always jump to the top of the sorted task list so the user sees the
        // most recently submitted tasks immediately after enqueueing.
        let sorted = self.sorted_task_display();
        if let Some(&vi) = sorted.first() {
            let id = self.engine.tasks[vi].id;
            self.set_selected_task(id);
        }
    }

    // ── Confirmation flow ────────────────────────────────────────────────────

    /// Stage `apply` for confirmation, annotating each target with plan info.
    ///
    /// When the Tasks pane is focused and all selected tasks are plan commands,
    /// the apply targets are derived from current ready plan tasks rather than
    /// the module selection. Stale plan-task selections stage nothing.
    /// Otherwise falls back to the normal module selection.
    pub fn request_apply_confirm(&mut self) {
        if self.focus == Focus::Tasks {
            if let Some(confirm_targets) = self.apply_targets_from_plan_tasks() {
                if confirm_targets.is_empty() {
                    return;
                }
                self.modal = Some(Modal::Confirm(PendingConfirm {
                    kind: ConfirmKind::Apply,
                    targets: confirm_targets,
                }));
                return;
            }
        }
        self.stage_module_confirm(ConfirmKind::Apply);
    }

    /// Derive apply `ConfirmTarget`s from the currently selected plan task(s)
    /// in the Tasks pane. Returns `None` if the selection contains non-plan tasks.
    /// Stale plan tasks are ignored; if none are current, returns an empty vec
    /// so callers do not fall back to the module selection.
    fn apply_targets_from_plan_tasks(&self) -> Option<Vec<ConfirmTarget>> {
        let task_ids: Vec<usize> = if !self.task_multi_select.is_empty() {
            self.task_multi_select.clone()
        } else {
            self.selected_task_id.into_iter().collect()
        };

        if task_ids.is_empty() {
            return None;
        }

        let tasks: Vec<&Task> = task_ids
            .iter()
            .filter_map(|&id| self.engine.tasks.iter().find(|t| t.id == id))
            .collect();

        if tasks.is_empty() {
            return None;
        }

        // All selected tasks must be plan commands; otherwise fall back.
        if tasks.iter().any(|t| t.command != "plan") {
            return None;
        }

        let mut seen = HashSet::new();
        let mut targets = Vec::new();
        for task in tasks {
            let Some(plan) = self.ready_plan_for_task(task) else {
                continue;
            };
            if let Some((idx, module)) = self
                .modules
                .iter()
                .enumerate()
                .find(|(_, m)| m.path == task.module_path)
            {
                if seen.insert(idx) {
                    targets.push(ConfirmTarget {
                        module_idx: idx,
                        module_name: module.display_name.clone(),
                        plan_age: Some(plan.age_str()),
                        lock_id: None,
                        lock_who: None,
                    });
                }
            }
        }

        Some(targets)
    }

    pub fn ready_plan_for_task(&self, task: &Task) -> Option<&crate::plan_cache::PlanEntry> {
        let plan = self.engine.plan_cache.get(&task.module_path)?;
        if task.command == "plan"
            && task.status == TaskStatus::Success
            && task.plan_output_path.as_ref() == Some(&plan.path)
            && plan.task_id == task.id
        {
            Some(plan)
        } else {
            None
        }
    }

    /// Stage `destroy` for confirmation.
    pub fn request_destroy_confirm(&mut self) {
        self.stage_module_confirm(ConfirmKind::Destroy);
    }

    /// Build a `ConfirmTarget` for `module_idx` under `kind`, or `None` to skip
    /// the module (only possible for `ForceUnlock`, when no lock is detected).
    fn annotate_confirm_target(
        &self,
        kind: &ConfirmKind,
        idx: usize,
        m: &Module,
    ) -> Option<ConfirmTarget> {
        match kind {
            ConfirmKind::Apply => Some(ConfirmTarget {
                module_idx: idx,
                module_name: m.display_name.clone(),
                plan_age: self.engine.plan_cache.get(&m.path).map(|e| e.age_str()),
                lock_id: None,
                lock_who: None,
            }),
            ConfirmKind::Destroy | ConfirmKind::InitUpgrade => Some(ConfirmTarget {
                module_idx: idx,
                module_name: m.display_name.clone(),
                plan_age: None,
                lock_id: None,
                lock_who: None,
            }),
            ConfirmKind::ForceUnlock => {
                let lock = read_lock_info(&m.path).or_else(|| self.detect_lock_from_tasks(idx))?;
                Some(ConfirmTarget {
                    module_idx: idx,
                    module_name: m.display_name.clone(),
                    plan_age: None,
                    lock_id: Some(lock.id),
                    lock_who: Some(lock.who),
                })
            }
        }
    }

    /// Stage `kind` for confirmation against the current module selection
    /// (`target_indices()`), annotating each target via `annotate_confirm_target`.
    /// Stages nothing if the selection or the annotated target list is empty.
    fn stage_module_confirm(&mut self, kind: ConfirmKind) {
        let targets = self.target_indices();
        if targets.is_empty() {
            return;
        }

        let confirm_targets: Vec<ConfirmTarget> = targets
            .iter()
            .filter_map(|&i| self.modules.get(i).map(|m| (i, m)))
            .filter_map(|(i, m)| self.annotate_confirm_target(&kind, i, m))
            .collect();

        if confirm_targets.is_empty() {
            return;
        }

        self.modal = Some(Modal::Confirm(PendingConfirm {
            kind,
            targets: confirm_targets,
        }));
    }

    /// Execute the confirmed command. Call after user presses `y`.
    pub fn confirm_execute(&mut self) {
        match self.modal.take() {
            Some(Modal::Confirm(confirm)) => match confirm.kind {
                ConfirmKind::Apply => {
                    let indices: Vec<usize> =
                        confirm.targets.iter().map(|t| t.module_idx).collect();
                    self.enqueue_apply_for(&indices);
                }
                ConfirmKind::Destroy => {
                    let indices: Vec<usize> =
                        confirm.targets.iter().map(|t| t.module_idx).collect();
                    self.enqueue_destroy_for(&indices);
                }
                ConfirmKind::InitUpgrade => {
                    let indices: Vec<usize> =
                        confirm.targets.iter().map(|t| t.module_idx).collect();
                    self.enqueue_init_upgrade_for(&indices);
                }
                ConfirmKind::ForceUnlock => {
                    let pairs: Vec<(usize, String)> = confirm
                        .targets
                        .into_iter()
                        .filter_map(|t| t.lock_id.map(|id| (t.module_idx, id)))
                        .collect();
                    self.enqueue_force_unlock_for(&pairs);
                }
            },
            other => self.modal = other,
        }
    }

    /// Stage `init -upgrade` for confirmation.
    pub fn request_init_upgrade_confirm(&mut self) {
        self.stage_module_confirm(ConfirmKind::InitUpgrade);
    }

    fn enqueue_init_upgrade_for(&mut self, targets: &[usize]) {
        if targets.is_empty() {
            return;
        }
        for &idx in targets {
            self.push_task_for(idx, "init", vec!["-upgrade".to_string()], None, None);
        }
        self.maybe_auto_select_task();
    }

    /// Detect a lock for a module by parsing the output of its most recent
    /// terminal task that contains a "Lock Info:" block.
    ///
    /// Used as a fallback for remote/S3 backends that don't write a local
    /// `.lock.info` file but do print the lock details to stdout/stderr when
    /// lock acquisition fails.
    fn detect_lock_from_tasks(&self, module_idx: usize) -> Option<crate::lock::LockInfo> {
        let module_path = &self.modules[module_idx].path;
        self.engine
            .tasks
            .iter()
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
        self.stage_module_confirm(ConfirmKind::ForceUnlock);
    }

    /// Enqueue `force-unlock -force <lock_id>` for each (module_idx, lock_id) pair.
    fn enqueue_force_unlock_for(&mut self, targets: &[(usize, String)]) {
        if targets.is_empty() {
            return;
        }
        for (idx, lock_id) in targets {
            self.push_task_for(
                *idx,
                "force-unlock",
                vec!["-force".to_string(), lock_id.clone()],
                None,
                None,
            );
        }
        self.maybe_auto_select_task();
    }

    pub fn cancel_confirm(&mut self) {
        self.modal = None;
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
                self.engine
                    .tasks
                    .iter()
                    .find(|t| t.id == id)
                    .map(|t| !t.status.is_terminal())
                    .unwrap_or(false)
            })
            .collect();

        if !active.is_empty() {
            self.modal = Some(Modal::CancelTasks(active));
        }
    }

    /// Execute cancellation for all staged task IDs, then clear staging + multi-select.
    pub fn cancel_staged_tasks(&mut self) {
        let Some(Modal::CancelTasks(ids)) = self.modal.take() else {
            return;
        };
        for id in ids {
            self.engine.cancel_task(id);
        }
        self.task_multi_select.clear();
    }

    pub fn completed_task_count(&self) -> usize {
        self.engine
            .tasks
            .iter()
            .filter(|t| t.status.is_terminal())
            .count()
    }

    /// Stage clearing completed task history for confirmation.
    ///
    /// Active tasks are never cleared because their completion events still need
    /// to be tracked by the app.
    pub fn request_clear_tasks_confirm(&mut self) {
        if self.completed_task_count() > 0 {
            self.modal = Some(Modal::ClearTasks);
        }
    }

    /// Clear terminal tasks from the task pane, preserving active tasks.
    pub fn clear_completed_tasks(&mut self) {
        self.modal = None;
        if self.engine.tasks.is_empty() {
            return;
        }

        let previous_selection = self.selected_task_id;
        self.engine.tasks.retain(|t| !t.status.is_terminal());

        let remaining_ids: HashSet<usize> = self.engine.tasks.iter().map(|t| t.id).collect();
        self.task_multi_select
            .retain(|id| remaining_ids.contains(id));

        let selection_still_exists = previous_selection
            .map(|id| remaining_ids.contains(&id))
            .unwrap_or(false);

        if !selection_still_exists {
            self.selected_task_id = self
                .sorted_task_display()
                .first()
                .map(|&vi| self.engine.tasks[vi].id);
            self.output_scroll = 0;
        }
    }

    /// Stage a session reset for confirmation.
    pub fn request_reset_confirm(&mut self) {
        self.modal = Some(Modal::Reset);
    }

    /// Counts of items the next `reset_session` call will clear.
    /// Returns (cached plans, queued tasks, finished tasks).
    pub fn reset_summary(&self) -> (usize, usize, usize) {
        let queued = self
            .engine
            .tasks
            .iter()
            .filter(|t| t.status == TaskStatus::Pending)
            .count();
        let finished = self
            .engine
            .tasks
            .iter()
            .filter(|t| t.status.is_terminal())
            .count();
        (self.engine.plan_cache.entry_count(), queued, finished)
    }

    /// Reset session state to a fresh-launch feel, preserving only tasks that
    /// are currently executing. Cancels queued tasks, drops terminal-task
    /// history, clears the plan cache, resets navigation/selection, and
    /// dismisses any open overlays.
    pub fn reset_session(&mut self) {
        let queued_ids: Vec<usize> = self
            .engine
            .tasks
            .iter()
            .filter(|t| t.status == TaskStatus::Pending)
            .map(|t| t.id)
            .collect();
        for id in queued_ids {
            self.engine.cancel_task(id);
        }

        self.engine.tasks.retain(|t| !t.status.is_terminal());

        self.engine.plan_cache.clear();

        self.multi_select.clear();
        self.multi_select_anchor = None;
        self.filter.clear();
        self.filter_active = false;
        self.max_depth = None;
        self.selected_module = 0;

        self.task_multi_select.clear();
        self.selected_task_id = None;
        self.output_scroll = 0;

        self.modal = None;

        self.focus = Focus::Modules;
    }

    // ── State explorer ───────────────────────────────────────────────────────

    /// Open the state explorer for the currently selected (or focused) module.
    /// State is loaded in the background (see `spawn_state_load`); the
    /// explorer opens immediately showing `StateContent::Loading`.
    pub fn open_state_explorer(&mut self) {
        let visible = self.visible_module_indices();
        let Some(&idx) = visible.get(self.selected_module) else {
            return;
        };
        let module = &self.modules[idx];
        self.state_explorer = Some(StateExplorer {
            module_idx: idx,
            module_name: module.display_name.clone(),
            content: StateContent::Loading,
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
            load_rx: None,
        });
        self.spawn_state_load();
    }

    pub fn close_state_explorer(&mut self) {
        self.state_explorer = None;
    }

    /// Move the selected resource in the state explorer by `delta` rows,
    /// operating on the currently filtered list.
    pub fn state_explorer_move(&mut self, delta: i32) {
        let Some(explorer) = &mut self.state_explorer else {
            return;
        };
        let count = explorer.filtered_count();
        if count == 0 {
            return;
        }
        explorer.selected = (explorer.selected as i32 + delta).clamp(0, count as i32 - 1) as usize;
    }

    pub fn state_explorer_go_first(&mut self) {
        if let Some(explorer) = &mut self.state_explorer {
            explorer.selected = 0;
        }
    }

    pub fn state_explorer_go_last(&mut self) {
        if let Some(explorer) = &mut self.state_explorer {
            let count = explorer.filtered_count();
            if count > 0 {
                explorer.selected = count - 1;
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
        let Some(explorer) = &mut self.state_explorer else {
            return;
        };
        let count = explorer.filtered_count();
        if count == 0 {
            explorer.selected = 0;
        } else if explorer.selected >= count {
            explorer.selected = count - 1;
        }
    }

    // ── Resource detail view ─────────────────────────────────────────────────

    /// Open the detail view for the currently selected filtered resource.
    pub fn open_resource_detail(&mut self) {
        // Gather data under an immutable borrow first.
        let result: Option<(String, Vec<String>)> = (|| {
            let explorer = self.state_explorer.as_ref()?;
            let resources = explorer.resources()?;
            let real_idx = explorer.selected_real_index()?;
            let resource = resources.get(real_idx)?;
            let address = resource.address.clone();
            let json = serde_json::to_string_pretty(&resource.instance)
                .unwrap_or_else(|_| "{}".to_string());
            let lines = json.lines().map(|l| l.to_string()).collect();
            Some((address, lines))
        })();

        if let (Some(explorer), Some((address, lines))) = (&mut self.state_explorer, result) {
            explorer.detail_view = Some(ResourceDetail {
                address,
                lines,
                scroll: 0,
            });
        }
    }

    /// Close the detail view and return to the resource list.
    pub fn close_resource_detail(&mut self) {
        if let Some(explorer) = &mut self.state_explorer {
            explorer.detail_view = None;
        }
    }

    pub fn resource_detail_scroll(&mut self, delta: i32) {
        let Some(explorer) = &mut self.state_explorer else {
            return;
        };
        let Some(detail) = &mut explorer.detail_view else {
            return;
        };
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
        let Some(explorer) = &mut self.state_explorer else {
            return;
        };
        let Some(real_idx) = explorer.selected_real_index() else {
            return;
        };

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
        let Some(explorer) = self.state_explorer.as_ref() else {
            return;
        };
        let module_idx = explorer.module_idx;
        let targets = explorer.target_addresses();

        if targets.is_empty() {
            return;
        }

        let plan_path = self
            .engine
            .plan_cache
            .plan_path_for(&self.modules[module_idx].path);
        let mut args = vec!["-out".to_string(), plan_path.to_string_lossy().into_owned()];
        for addr in &targets {
            args.push(format!("-target={}", addr));
        }
        self.push_task_for(module_idx, "plan", args, Some(plan_path), None);
        self.maybe_auto_select_task();
        if let Some(explorer) = self.state_explorer.as_mut() {
            explorer.plan_queued_notice = true;
        }
    }

    /// Stage a confirmation for the given operation on the selected (or multi-selected) resources.
    pub fn request_op_confirm(&mut self, kind: ExplorerOpKind) {
        let Some(explorer) = &mut self.state_explorer else {
            return;
        };
        let targets = explorer.target_addresses();
        if targets.is_empty() {
            return;
        }
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
            let Some(explorer) = self.state_explorer.as_mut() else {
                return;
            };
            let Some(kind) = explorer.op_confirm.take() else {
                return;
            };
            let targets = std::mem::take(&mut explorer.op_targets);
            (explorer.module_idx, kind, targets)
        };

        if targets.is_empty() {
            return;
        }

        match kind {
            ExplorerOpKind::Taint | ExplorerOpKind::StateRm => {
                // Sequential: run one address at a time, chaining via check_op_completion.
                let first = targets.remove(0);
                let remaining = targets;
                let mut args: Vec<String> = kind.pre_args().iter().map(|s| s.to_string()).collect();
                args.push(first.clone());
                let task_id = self.push_task_for(module_idx, kind.command(), args, None, None);
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
                let task_id = self.push_task_for(module_idx, "destroy", args, None, None);
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
    /// moves to the result view. No-op if there's no pending op, or the
    /// finished task isn't the one it's currently waiting on.
    fn check_op_completion(&mut self, task_id: usize, success: bool) {
        // Phase 1: mutate the PendingOp (record the completion, dequeue the
        // next address if any) under one scoped borrow, then decide what to
        // do next without holding any borrow of `self`.
        enum NextAction {
            RunNext {
                module_idx: usize,
                kind: ExplorerOpKind,
                addr: String,
            },
            Finished {
                kind: ExplorerOpKind,
            },
        }

        let next_action: Option<NextAction> = (|| {
            let explorer = self.state_explorer.as_mut()?;
            let module_idx = explorer.module_idx;
            let pt = explorer.pending_op.as_mut()?;
            let (rid, addr) = pt.running.take()?;
            if rid != task_id {
                // Not the task we're waiting on — put it back untouched.
                pt.running = Some((rid, addr));
                return None;
            }
            pt.done.push((addr, success));
            let kind = pt.kind;
            if pt.queue.is_empty() {
                Some(NextAction::Finished { kind })
            } else {
                let next_addr = pt.queue.remove(0);
                Some(NextAction::RunNext {
                    module_idx,
                    kind,
                    addr: next_addr,
                })
            }
        })();

        // Phase 2: act on the decision. `push_task_for` needs `&mut self`,
        // so this happens after the scoped borrow above has ended.
        match next_action {
            Some(NextAction::RunNext {
                module_idx,
                kind,
                addr,
            }) => {
                let mut args: Vec<String> = kind.pre_args().iter().map(|s| s.to_string()).collect();
                args.push(addr.clone());
                let new_task_id = self.push_task_for(module_idx, kind.command(), args, None, None);
                if let Some(pt) = self
                    .state_explorer
                    .as_mut()
                    .and_then(|e| e.pending_op.as_mut())
                {
                    pt.running = Some((new_task_id, addr));
                }
            }
            Some(NextAction::Finished { kind }) => {
                if let Some(explorer) = self.state_explorer.as_mut() {
                    if let Some(pending) = explorer.pending_op.take() {
                        explorer.op_result = Some(OpResult {
                            kind,
                            entries: pending.done,
                        });
                    }
                }
                // Refresh the resource list so tainted/removed resources reflect
                // their new status when the result popup is dismissed.
                self.refresh_state_explorer();
            }
            None => {}
        }
    }

    pub fn dismiss_op_result(&mut self) {
        if let Some(explorer) = &mut self.state_explorer {
            explorer.op_result = None;
        }
    }

    /// Re-read the state file for the current explorer module in the
    /// background and update the resource list once it lands. The view shows
    /// a loading spinner while the read is in flight (unlike the old
    /// synchronous refresh, which froze the UI thread for remote backends).
    pub fn refresh_state_explorer(&mut self) {
        self.spawn_state_load();
    }

    /// Kick off a background state read for the current explorer module, if
    /// one isn't already in flight. Sets `content = Loading` immediately;
    /// the result is installed by `poll_state_load` once it arrives.
    fn spawn_state_load(&mut self) {
        let Some(explorer) = self.state_explorer.as_mut() else {
            return;
        };
        if explorer.load_rx.is_some() {
            // A load is already in flight — don't start a second one.
            return;
        }
        let Some(module) = self.modules.get(explorer.module_idx) else {
            return;
        };
        let path = module.path.clone();
        let binary = self.config.binary.clone();

        let (tx, rx) = tokio::sync::oneshot::channel();
        tokio::task::spawn_blocking(move || {
            let _ = tx.send(crate::state::read_state(&path, &binary));
        });

        explorer.content = StateContent::Loading;
        explorer.load_rx = Some(rx);
    }

    /// Poll for a completed background state load, installing the result and
    /// clamping selection if one has landed. Called at the end of `drain_events`.
    fn poll_state_load(&mut self) {
        let Some(explorer) = self.state_explorer.as_mut() else {
            return;
        };
        let Some(rx) = explorer.load_rx.as_mut() else {
            return;
        };

        let content = match rx.try_recv() {
            Ok(content) => content,
            Err(tokio::sync::oneshot::error::TryRecvError::Empty) => return,
            Err(tokio::sync::oneshot::error::TryRecvError::Closed) => {
                StateContent::Error("state load task failed".to_string())
            }
        };

        explorer.load_rx = None;
        explorer.content = content;
        explorer.multi_select.clear();
        let count = explorer.filtered_count();
        if count == 0 {
            explorer.selected = 0;
        } else if explorer.selected >= count {
            explorer.selected = count - 1;
        }
    }

    // ── Event processing ─────────────────────────────────────────────────────

    /// Drain pending task events (non-blocking).
    ///
    /// Bumps the spinner tick, applies every engine update to `self.engine.tasks`
    /// (already done inside the engine), and reacts to `Finished` updates by
    /// chaining any pending state-explorer operation. Also polls for a
    /// completed background state load (see `spawn_state_load`).
    pub fn drain_events(&mut self) {
        self.spinner_tick = self.spinner_tick.wrapping_add(1);
        for update in self.engine.drain_events() {
            if let EngineUpdate::Finished { task_id, success } = update {
                self.check_op_completion(task_id, success);
            }
        }
        self.poll_state_load();
    }

    // ── Output pane ──────────────────────────────────────────────────────────

    fn output_task(&self) -> Option<&Task> {
        if let Some(id) = self.selected_task_id {
            return self.engine.tasks.iter().find(|t| t.id == id);
        }
        // Fallback: most recently active task with output.
        self.sorted_task_display()
            .into_iter()
            .map(|vi| &self.engine.tasks[vi])
            .find(|t| !t.output_lines.is_empty() || t.status == TaskStatus::Running)
    }

    pub fn current_output(&self) -> &[String] {
        self.output_task()
            .map(|t| t.output_lines.as_slice())
            .unwrap_or_default()
    }

    pub fn output_title(&self) -> String {
        if let Some(task) = self.output_task() {
            return format!("Output [{}: {}]", task.module_name, task.command);
        }
        "Output".to_string()
    }

    /// Tasks that are still Pending or Running.
    pub fn active_tasks(&self) -> Vec<&Task> {
        self.engine
            .tasks
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::module::{Module, ModuleKind};
    use std::path::PathBuf;
    use std::time::Instant;

    fn test_app() -> App {
        let root = PathBuf::from("/tmp/rug-test");
        let modules = vec![Module {
            path: root.join("app"),
            display_name: "app".to_string(),
            kind: ModuleKind::Root,
        }];
        let config = Config {
            binary: "terraform".to_string(),
            parallelism: 1,
            ignore_dirs: Vec::new(),
            show_library_modules: false,
        };

        App::new(config, root, modules).unwrap()
    }

    fn push_plan_task(app: &mut App, id: usize, plan_path: PathBuf) {
        let module = &app.modules[0];
        app.engine.tasks.push(Task {
            id,
            module_path: module.path.clone(),
            module_name: module.display_name.clone(),
            command: "plan".to_string(),
            status: TaskStatus::Success,
            output_lines: Vec::new(),
            started_at: Some(Instant::now()),
            finished_at: Some(Instant::now()),
            plan_output_path: Some(plan_path),
            cleanup_plan_path: None,
            resource_counts: None,
            cancel_handle: None,
        });
    }

    #[test]
    fn ready_plan_for_task_requires_current_cache_owner() {
        let mut app = test_app();
        let module_path = app.modules[0].path.clone();
        let plan_path = app.engine.plan_cache.plan_path_for(&module_path);

        push_plan_task(&mut app, 1, plan_path.clone());
        push_plan_task(&mut app, 2, plan_path.clone());
        app.engine
            .plan_cache
            .register(module_path, plan_path, app.engine.tasks[1].id);

        assert!(app.ready_plan_for_task(&app.engine.tasks[0]).is_none());
        assert!(app.ready_plan_for_task(&app.engine.tasks[1]).is_some());
    }

    #[test]
    fn stale_plan_task_apply_does_not_fallback_to_module_selection() {
        let mut app = test_app();
        let module_path = app.modules[0].path.clone();
        let plan_path = app.engine.plan_cache.plan_path_for(&module_path);

        push_plan_task(&mut app, 1, plan_path.clone());
        push_plan_task(&mut app, 2, plan_path.clone());
        app.engine
            .plan_cache
            .register(module_path, plan_path, app.engine.tasks[1].id);

        app.focus = Focus::Tasks;
        app.selected_module = 0;
        app.selected_task_id = Some(app.engine.tasks[0].id);
        app.request_apply_confirm();

        assert!(app.modal.is_none());
    }
}
