use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Instant;

use crate::config::Config;
use crate::discovery;
use crate::engine::{EngineUpdate, TaskEngine, TaskSpec};
use crate::lock::{parse_lock_from_output, read_lock_info};
use crate::module::Module;
use crate::state::StateContent;
use crate::task::{ResourceCounts, Task, TaskStatus};

/// The two top-level screens: the module picker and the run board.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Select,
    Run,
}

/// One module brought into a Run session. `path` is the source of truth;
/// `module_idx` is a cache into `app.modules`, re-resolved after a refresh.
#[derive(Debug, Clone)]
pub struct SessionModule {
    pub module_idx: usize,
    pub path: PathBuf,
    pub name: String,
}

/// The set of modules the user is running actions against on the Run screen,
/// plus that screen's board/output view state.
pub struct RunSession {
    /// Display order = selection order at creation.
    pub modules: Vec<SessionModule>,
    /// Cursor index into `modules` (never into `app.modules`).
    pub cursor: usize,
    /// Board multi-select: indices into `modules`.
    pub selected: Vec<usize>,
    /// Anchor position for range-select on the board.
    pub select_anchor: Option<usize>,
    /// Latest task id started by this session, per module path.
    pub latest_task: HashMap<PathBuf, usize>,
    /// Output pane fills the whole window (mouse capture off).
    pub fullscreen: bool,
    /// Lines scrolled up from the tail; 0 = tail-follow.
    pub output_scroll: u16,
    /// Soft-wrap long output lines.
    pub output_wrap: bool,
    pub created_at: Instant,
}

impl RunSession {
    pub fn new(modules: Vec<SessionModule>) -> Self {
        Self {
            modules,
            cursor: 0,
            selected: Vec::new(),
            select_anchor: None,
            latest_task: HashMap::new(),
            fullscreen: false,
            output_scroll: 0,
            output_wrap: false,
            created_at: Instant::now(),
        }
    }
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

/// Heights of each scrollable region, updated every draw pass.
/// Used to compute page-up/page-down scroll amounts and mouse hit-testing.
#[derive(Debug, Default, Clone, Copy)]
pub struct ViewportHeights {
    /// Select list height (for page navigation).
    pub list: u16,
    /// Run output pane height (for page navigation).
    pub output: u16,
    /// State explorer viewport height (for page navigation).
    pub explorer: u16,
    /// Run board list height (for page navigation).
    pub board: u16,
    /// Select list area top row (mouse hit-testing).
    pub list_top: u16,
    /// Select list scroll offset from the last render (mouse hit-testing).
    pub list_offset: u16,
    /// Run board list area top row (mouse hit-testing).
    pub board_top: u16,
    /// Run board list scroll offset from the last render (mouse hit-testing).
    pub board_offset: u16,
    /// Run output area top row; `u16::MAX` when no output pane is shown (H4).
    pub output_top: u16,
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
    /// Which top-level screen is showing.
    pub screen: Screen,
    pub modules: Vec<Module>,
    pub selected_module: usize,
    /// Modules currently multi-selected (indices into `modules`).
    pub multi_select: Vec<usize>,
    /// Visible-list position of the last Space press; used as the anchor for
    /// Ctrl+Space range selection.
    pub multi_select_anchor: Option<usize>,
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
    /// Heights/positions of scrollable regions from the last render pass
    /// (page up/down amounts and mouse hit-testing).
    pub viewport: ViewportHeights,
    /// Task execution engine: task list, plan cache, run/queue bookkeeping.
    pub engine: TaskEngine,
    /// State explorer popup, open when `Some`.
    pub state_explorer: Option<StateExplorer>,
    /// Active Run session. Invariant: `screen == Run` ⇒ `session.is_some()`.
    pub session: Option<RunSession>,
}

impl App {
    pub fn new(config: Config, root: PathBuf, modules: Vec<Module>) -> std::io::Result<Self> {
        let parallelism = config.parallelism;
        let engine = TaskEngine::new(config.binary.clone(), parallelism)?;
        Ok(Self {
            config,
            root,
            screen: Screen::Select,
            modules,
            selected_module: 0,
            multi_select: Vec::new(),
            multi_select_anchor: None,
            spinner_tick: 0,
            filter: String::new(),
            filter_active: false,
            max_depth: None,
            modal: None,
            viewport: ViewportHeights::default(),
            engine,
            state_explorer: None,
            session: None,
        })
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
                    && self.max_depth.is_none_or(|d| module_depth(m) <= d)
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

    /// Set the module cursor to an absolute position in the visible list
    /// (mouse click). No-op if out of range.
    pub fn set_module_cursor(&mut self, pos: usize) {
        if pos < self.visible_module_indices().len() {
            self.selected_module = pos;
        }
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

        self.remap_session();
    }

    /// `*` key: select every visible module, or clear them all if they are
    /// already all selected. Only touches visible modules; hidden selections
    /// (under a filter) are preserved when selecting all.
    pub fn toggle_select_all_visible(&mut self) {
        let visible = self.visible_module_indices();
        if visible.is_empty() {
            return;
        }
        let all_selected = visible.iter().all(|i| self.multi_select.contains(i));
        if all_selected {
            self.multi_select.retain(|i| !visible.contains(i));
        } else {
            for i in visible {
                if !self.multi_select.contains(&i) {
                    self.multi_select.push(i);
                }
            }
        }
    }

    /// Spinner frame index + command for the newest non-terminal task on
    /// `path`, if any. Used to draw a running indicator next to a module.
    pub fn module_activity(&self, path: &Path) -> Option<(usize, String)> {
        let task = self
            .engine
            .tasks
            .iter()
            .filter(|t| t.module_path == *path && !t.status.is_terminal())
            .max_by_key(|t| t.id)?;
        let frame =
            (self.spinner_tick as usize / 2) % crate::ui::theme::SPINNER_FRAMES.len();
        Some((frame, task.command.clone()))
    }

    /// Resource counts from the plan currently cached for `path` (looked up via
    /// the cache entry's owning task), if that task recorded a summary.
    pub fn ready_plan_counts(&self, path: &Path) -> Option<&ResourceCounts> {
        let entry = self.engine.plan_cache.get(path)?;
        let task = self.engine.task(entry.task_id)?;
        task.resource_counts.as_ref()
    }

    // ── Run session ──────────────────────────────────────────────────────────

    /// Enter the Run screen for the current Select targets. Resumes an existing
    /// session if the target set matches it exactly; otherwise starts a fresh
    /// session (old tasks keep running in the engine). No-op if no targets.
    pub fn enter_run(&mut self) {
        let mut targets = self.target_indices();
        if targets.is_empty() {
            return;
        }
        targets.sort_unstable();
        targets.dedup();

        let target_paths: HashSet<PathBuf> = targets
            .iter()
            .map(|&i| self.modules[i].path.clone())
            .collect();

        let same = self
            .session
            .as_ref()
            .map(|s| {
                let session_paths: HashSet<PathBuf> =
                    s.modules.iter().map(|m| m.path.clone()).collect();
                session_paths == target_paths
            })
            .unwrap_or(false);

        if same {
            self.screen = Screen::Run;
            return;
        }

        let modules: Vec<SessionModule> = targets
            .iter()
            .map(|&i| SessionModule {
                module_idx: i,
                path: self.modules[i].path.clone(),
                name: self.modules[i].display_name.clone(),
            })
            .collect();

        self.session = Some(RunSession::new(modules));
        self.screen = Screen::Run;
    }

    /// Raw `app.modules` indices the next Run action should target: the board
    /// multi-selected subset if non-empty, else ALL session modules.
    pub fn run_scope_indices(&self) -> Vec<usize> {
        let Some(s) = &self.session else {
            return Vec::new();
        };
        if s.selected.is_empty() {
            s.modules.iter().map(|m| m.module_idx).collect()
        } else {
            s.selected
                .iter()
                .filter_map(|&i| s.modules.get(i))
                .map(|m| m.module_idx)
                .collect()
        }
    }

    /// Raw `app.modules` index of the highlighted board row.
    pub fn run_highlight_index(&self) -> Option<usize> {
        let s = self.session.as_ref()?;
        s.modules.get(s.cursor).map(|m| m.module_idx)
    }

    /// Path of the highlighted board module, if any.
    pub fn run_cursor_path(&self) -> Option<PathBuf> {
        let s = self.session.as_ref()?;
        s.modules.get(s.cursor).map(|m| m.path.clone())
    }

    /// The task shown for `path` on the Run board, plus whether it is a
    /// background task from a previous session (the `·prev` tag). Resolution:
    /// this session's latest task for the path, else the newest non-terminal
    /// task for it in the engine, else none (idle).
    pub fn display_task_for(&self, path: &Path) -> Option<(&Task, bool)> {
        if let Some(s) = &self.session {
            if let Some(&id) = s.latest_task.get(path) {
                if let Some(t) = self.engine.task(id) {
                    return Some((t, false));
                }
            }
        }
        self.engine
            .tasks
            .iter()
            .filter(|t| t.module_path == *path && !t.status.is_terminal())
            .max_by_key(|t| t.id)
            .map(|t| (t, true))
    }

    /// Output lines of the highlighted board module's display task.
    pub fn run_output_lines(&self) -> &[String] {
        self.session
            .as_ref()
            .and_then(|s| s.modules.get(s.cursor))
            .and_then(|m| self.display_task_for(&m.path))
            .map(|(t, _)| t.output_lines.as_slice())
            .unwrap_or_default()
    }

    /// Move the board cursor by `delta`, resetting output scroll on a change.
    /// Returns true if the cursor moved.
    pub fn run_move_cursor(&mut self, delta: i32) -> bool {
        let Some(s) = self.session.as_mut() else {
            return false;
        };
        let count = s.modules.len();
        if count == 0 {
            return false;
        }
        let new = (s.cursor as i32 + delta).clamp(0, count as i32 - 1) as usize;
        let changed = new != s.cursor;
        s.cursor = new;
        if changed {
            s.output_scroll = 0;
        }
        changed
    }

    /// Set the board cursor to an absolute index (mouse click), resetting the
    /// output scroll on a change. Returns true if the cursor moved.
    pub fn run_set_cursor(&mut self, pos: usize) -> bool {
        let Some(s) = self.session.as_mut() else {
            return false;
        };
        if pos >= s.modules.len() {
            return false;
        }
        let changed = pos != s.cursor;
        s.cursor = pos;
        if changed {
            s.output_scroll = 0;
        }
        changed
    }

    /// Toggle the cursor row in/out of the board multi-select; set the anchor.
    pub fn run_board_toggle(&mut self) {
        if let Some(s) = self.session.as_mut() {
            let cur = s.cursor;
            if let Some(pos) = s.selected.iter().position(|&i| i == cur) {
                s.selected.remove(pos);
            } else {
                s.selected.push(cur);
            }
            s.select_anchor = Some(cur);
        }
    }

    /// Range-select board rows from the anchor to the cursor (inclusive).
    pub fn run_board_range(&mut self) {
        if let Some(s) = self.session.as_mut() {
            let Some(anchor) = s.select_anchor else {
                let cur = s.cursor;
                if !s.selected.contains(&cur) {
                    s.selected.push(cur);
                }
                s.select_anchor = Some(cur);
                return;
            };
            let lo = anchor.min(s.cursor);
            let hi = anchor.max(s.cursor).min(s.modules.len().saturating_sub(1));
            for i in lo..=hi {
                if !s.selected.contains(&i) {
                    s.selected.push(i);
                }
            }
            s.select_anchor = Some(s.cursor);
        }
    }

    /// Select every board row, or clear if all are already selected.
    pub fn run_board_toggle_all(&mut self) {
        if let Some(s) = self.session.as_mut() {
            if !s.modules.is_empty() && s.selected.len() == s.modules.len() {
                s.selected.clear();
            } else {
                s.selected = (0..s.modules.len()).collect();
            }
        }
    }

    /// Clear the board multi-select.
    pub fn run_board_clear(&mut self) {
        if let Some(s) = self.session.as_mut() {
            s.selected.clear();
        }
    }

    /// Stage a cancel confirmation for the non-terminal display tasks of the
    /// modules in `targets` (raw indices). No-op if none are active.
    pub fn request_cancel_run_scope(&mut self, targets: &[usize]) {
        let paths: Vec<PathBuf> = targets
            .iter()
            .filter_map(|&i| self.modules.get(i).map(|m| m.path.clone()))
            .collect();
        let mut ids = Vec::new();
        for p in paths {
            if let Some((t, _)) = self.display_task_for(&p) {
                if !t.status.is_terminal() {
                    ids.push(t.id);
                }
            }
        }
        if !ids.is_empty() {
            self.modal = Some(Modal::CancelTasks(ids));
        }
    }

    /// Scroll the Run output pane (positive = scroll up toward older lines).
    /// Clamped to the display task's line count. Returns true if it moved.
    pub fn run_scroll_output(&mut self, delta: i32) -> bool {
        let max = self.run_output_lines().len() as u16;
        let Some(s) = self.session.as_mut() else {
            return false;
        };
        let before = s.output_scroll;
        if delta < 0 {
            s.output_scroll = s.output_scroll.saturating_sub((-delta) as u16);
        } else {
            s.output_scroll = s.output_scroll.saturating_add(delta as u16).min(max);
        }
        s.output_scroll != before
    }

    /// `(module_count, running_count)` for the current session, if any.
    pub fn session_indicator(&self) -> Option<(usize, usize)> {
        let s = self.session.as_ref()?;
        let n = s.modules.len();
        let running = s
            .modules
            .iter()
            .filter(|m| {
                self.engine
                    .tasks
                    .iter()
                    .any(|t| t.module_path == m.path && !t.status.is_terminal())
            })
            .count();
        Some((n, running))
    }

    /// Record the newest task id per module path into the session's
    /// `latest_task` map (each module's display task on the Run board).
    pub fn record_session_tasks(&mut self, ids: &[usize]) {
        if self.session.is_none() {
            return;
        }
        let pairs: Vec<(PathBuf, usize)> = ids
            .iter()
            .filter_map(|&id| {
                self.engine
                    .tasks
                    .iter()
                    .find(|t| t.id == id)
                    .map(|t| (t.module_path.clone(), id))
            })
            .collect();
        if let Some(session) = self.session.as_mut() {
            for (path, id) in pairs {
                session.latest_task.insert(path, id);
            }
        }
    }

    /// Re-resolve the session against the current module list after a refresh:
    /// remap `module_idx` by path, drop missing modules, clamp cursor and
    /// selection. Drops the session entirely (and leaves the Run screen) if no
    /// module survives.
    fn remap_session(&mut self) {
        let Some(mut session) = self.session.take() else {
            return;
        };

        let index: HashMap<PathBuf, usize> = self
            .modules
            .iter()
            .enumerate()
            .map(|(i, m)| (m.path.clone(), i))
            .collect();

        let mut kept: Vec<SessionModule> = Vec::new();
        let mut old_to_new: HashMap<usize, usize> = HashMap::new();
        for (old_pos, sm) in session.modules.iter().enumerate() {
            if let Some(&new_idx) = index.get(&sm.path) {
                old_to_new.insert(old_pos, kept.len());
                kept.push(SessionModule {
                    module_idx: new_idx,
                    path: sm.path.clone(),
                    name: sm.name.clone(),
                });
            }
        }

        if kept.is_empty() {
            if self.screen == Screen::Run {
                self.screen = Screen::Select;
            }
            return; // session already taken; stays None
        }

        let new_selected: Vec<usize> = session
            .selected
            .iter()
            .filter_map(|old| old_to_new.get(old).copied())
            .collect();
        let cursor = old_to_new
            .get(&session.cursor)
            .copied()
            .unwrap_or(0)
            .min(kept.len() - 1);

        session.modules = kept;
        session.selected = new_selected;
        session.cursor = cursor;
        session.select_anchor = None;
        session.latest_task.retain(|p, _| index.contains_key(p));

        self.session = Some(session);
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

    /// Enqueue `plan` for `targets`, writing plan files into the managed temp
    /// dir so they can be reused by a subsequent apply. Returns created ids.
    pub fn enqueue_plan(&mut self, targets: &[usize]) -> Vec<usize> {
        let mut ids = Vec::new();
        for &idx in targets {
            let plan_path = self.engine.plan_cache.plan_path_for(&self.modules[idx].path);
            let args = vec!["-out".to_string(), plan_path.to_string_lossy().into_owned()];
            ids.push(self.push_task_for(idx, "plan", args, Some(plan_path), None));
        }
        ids
    }

    /// Enqueue a generic command (init, exec, etc.) for `targets`. Returns ids.
    pub fn enqueue_command(
        &mut self,
        command: &str,
        extra_args: Vec<String>,
        targets: &[usize],
    ) -> Vec<usize> {
        let mut ids = Vec::new();
        for &idx in targets {
            ids.push(self.push_task_for(idx, command, extra_args.clone(), None, None));
        }
        ids
    }

    /// Enqueue apply for explicitly captured target indices (from a PendingConfirm).
    ///
    /// Per module: if a plan file exists in the cache, apply from that file;
    /// otherwise fall back to `-auto-approve`. The cache entry is removed so
    /// the UI stops advertising a stale plan, and the file is deleted after the
    /// apply process exits.
    fn enqueue_apply_for(&mut self, targets: &[usize]) -> Vec<usize> {
        let mut ids = Vec::new();
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
            ids.push(self.push_task_for(idx, "apply", args, None, cleanup_plan_path));
        }

        ids
    }

    /// Enqueue destroy for explicitly captured target indices.
    fn enqueue_destroy_for(&mut self, targets: &[usize]) -> Vec<usize> {
        let mut ids = Vec::new();
        for &idx in targets {
            ids.push(self.push_task_for(
                idx,
                "destroy",
                vec!["-auto-approve".to_string()],
                None,
                None,
            ));
        }
        ids
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

    // ── Confirmation flow ────────────────────────────────────────────────────

    /// Stage `apply` for confirmation against `targets`, annotating each with
    /// plan info. Apply consumes any cached plan file per module.
    pub fn request_apply_confirm(&mut self, targets: &[usize]) {
        self.stage_module_confirm(ConfirmKind::Apply, targets);
    }

    /// Whether `task` is the plan whose output file the plan cache currently
    /// owns for its module (i.e. a subsequent apply would consume it). Retained
    /// for the cache-owner invariant it encodes and its regression test; the
    /// live apply path re-derives this via `plan_cache.take` in `enqueue_apply_for`.
    #[allow(dead_code)]
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

    /// Stage `destroy` for confirmation against `targets`.
    pub fn request_destroy_confirm(&mut self, targets: &[usize]) {
        self.stage_module_confirm(ConfirmKind::Destroy, targets);
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

    /// Stage `kind` for confirmation against `targets`, annotating each target
    /// via `annotate_confirm_target`. Stages nothing if the target list or the
    /// annotated target list is empty.
    fn stage_module_confirm(&mut self, kind: ConfirmKind, targets: &[usize]) {
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
            Some(Modal::Confirm(confirm)) => {
                let ids = match confirm.kind {
                    ConfirmKind::Apply => {
                        let indices: Vec<usize> =
                            confirm.targets.iter().map(|t| t.module_idx).collect();
                        self.enqueue_apply_for(&indices)
                    }
                    ConfirmKind::Destroy => {
                        let indices: Vec<usize> =
                            confirm.targets.iter().map(|t| t.module_idx).collect();
                        self.enqueue_destroy_for(&indices)
                    }
                    ConfirmKind::InitUpgrade => {
                        let indices: Vec<usize> =
                            confirm.targets.iter().map(|t| t.module_idx).collect();
                        self.enqueue_init_upgrade_for(&indices)
                    }
                    ConfirmKind::ForceUnlock => {
                        let pairs: Vec<(usize, String)> = confirm
                            .targets
                            .into_iter()
                            .filter_map(|t| t.lock_id.map(|id| (t.module_idx, id)))
                            .collect();
                        self.enqueue_force_unlock_for(&pairs)
                    }
                };
                self.record_session_tasks(&ids);
            }
            other => self.modal = other,
        }
    }

    /// Stage `init -upgrade` for confirmation against `targets`.
    pub fn request_init_upgrade_confirm(&mut self, targets: &[usize]) {
        self.stage_module_confirm(ConfirmKind::InitUpgrade, targets);
    }

    fn enqueue_init_upgrade_for(&mut self, targets: &[usize]) -> Vec<usize> {
        let mut ids = Vec::new();
        for &idx in targets {
            ids.push(self.push_task_for(idx, "init", vec!["-upgrade".to_string()], None, None));
        }
        ids
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
    pub fn request_force_unlock_confirm(&mut self, targets: &[usize]) {
        self.stage_module_confirm(ConfirmKind::ForceUnlock, targets);
    }

    /// Enqueue `force-unlock -force <lock_id>` for each (module_idx, lock_id) pair.
    fn enqueue_force_unlock_for(&mut self, targets: &[(usize, String)]) -> Vec<usize> {
        let mut ids = Vec::new();
        for (idx, lock_id) in targets {
            ids.push(self.push_task_for(
                *idx,
                "force-unlock",
                vec!["-force".to_string(), lock_id.clone()],
                None,
                None,
            ));
        }
        ids
    }

    pub fn cancel_confirm(&mut self) {
        self.modal = None;
    }

    /// Execute cancellation for all staged task IDs.
    pub fn cancel_staged_tasks(&mut self) {
        let Some(Modal::CancelTasks(ids)) = self.modal.take() else {
            return;
        };
        for id in ids {
            self.engine.cancel_task(id);
        }
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

    /// Clear terminal tasks from the engine history, preserving active tasks.
    /// The Run board falls back to live task resolution for any module whose
    /// recorded display task was cleared (see `display_task_for`).
    pub fn clear_completed_tasks(&mut self) {
        self.modal = None;
        self.engine.tasks.retain(|t| !t.status.is_terminal());
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

        self.modal = None;

        self.session = None;
        self.screen = Screen::Select;
    }

    // ── State explorer ───────────────────────────────────────────────────────

    /// Open the state explorer for the currently selected (or focused) module.
    /// State is loaded in the background (see `spawn_state_load`); the
    /// explorer opens immediately showing `StateContent::Loading`.
    pub fn open_state_explorer(&mut self, module_idx: usize) {
        let Some(module) = self.modules.get(module_idx) else {
            return;
        };
        self.state_explorer = Some(StateExplorer {
            module_idx,
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

    fn multi_module_app(n: usize) -> App {
        let root = PathBuf::from("/tmp/rug-test-multi");
        let modules: Vec<Module> = (0..n)
            .map(|i| Module {
                path: root.join(format!("m{i}")),
                display_name: format!("m{i}"),
                kind: ModuleKind::Root,
            })
            .collect();
        let config = Config {
            binary: "terraform".to_string(),
            parallelism: 1,
            ignore_dirs: Vec::new(),
            show_library_modules: false,
        };
        App::new(config, root, modules).unwrap()
    }

    #[test]
    fn enter_run_resumes_same_set_and_forks_on_change() {
        let mut app = multi_module_app(3);

        // Enter Run for modules {0,1}.
        app.multi_select = vec![0, 1];
        app.enter_run();
        assert_eq!(app.screen, Screen::Run);
        assert_eq!(app.session.as_ref().unwrap().modules.len(), 2);

        // Dirty the board state, go back, re-enter with the same set → RESUME
        // (board state preserved, no fresh session).
        app.session.as_mut().unwrap().cursor = 1;
        app.session.as_mut().unwrap().selected = vec![0];
        app.screen = Screen::Select;
        app.enter_run();
        assert_eq!(app.screen, Screen::Run);
        assert_eq!(app.session.as_ref().unwrap().cursor, 1);
        assert_eq!(app.session.as_ref().unwrap().selected, vec![0]);
        assert_eq!(app.session.as_ref().unwrap().modules.len(), 2);

        // Change the target set → FRESH session (board state reset).
        app.screen = Screen::Select;
        app.multi_select = vec![0, 1, 2];
        app.enter_run();
        assert_eq!(app.session.as_ref().unwrap().cursor, 0);
        assert!(app.session.as_ref().unwrap().selected.is_empty());
        assert_eq!(app.session.as_ref().unwrap().modules.len(), 3);
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
}
