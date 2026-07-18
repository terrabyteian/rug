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
use crate::ui::output_layout::OutputLayout;

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
    /// Output pane fills the whole window.
    pub fullscreen: bool,
    /// Display rows scrolled up from the tail; 0 = tail-follow.
    pub output_scroll: usize,
    /// Soft-wrap long output lines.
    pub output_wrap: bool,
    /// In-progress or completed drag selection over the output pane.
    pub selection: Option<OutputSelection>,
    /// A press anchor waiting to be promoted into a selection by a drag.
    pub pending_sel: Option<PendingSel>,
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
            selection: None,
            pending_sel: None,
            created_at: Instant::now(),
        }
    }
}

/// A position in output content: source line index + char index into the
/// ANSI-stripped line text. Content-anchored so selections survive resize,
/// wrap toggles, and streaming appends.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct SelPos {
    pub line: usize,
    pub ch: usize,
}

/// An in-progress or completed drag selection over the output pane.
#[derive(Debug, Clone, Copy)]
pub struct OutputSelection {
    /// Display task the coordinates refer to; the selection is dropped when
    /// the cursor module's display task changes.
    pub task: Option<usize>,
    pub anchor: SelPos,
    pub head: SelPos,
    pub dragging: bool,
}

impl OutputSelection {
    /// `(min, max)` of `anchor`/`head` in document order.
    pub fn ordered(&self) -> (SelPos, SelPos) {
        if self.anchor <= self.head {
            (self.anchor, self.head)
        } else {
            (self.head, self.anchor)
        }
    }

    /// True when `anchor == head` (a click with no drag).
    pub fn is_empty(&self) -> bool {
        self.anchor == self.head
    }
}

/// A left-press on output content that has not yet become a selection.
/// Promoted to an `OutputSelection` by the first drag; discarded on release.
#[derive(Debug, Clone, Copy)]
pub struct PendingSel {
    /// Display task at press time; stale pendings are discarded.
    pub task: Option<usize>,
    pub pos: SelPos,
}

/// Per-module info shown in a confirmation overlay.
#[derive(Debug, Clone)]
pub struct ConfirmTarget {
    pub module_idx: usize,
    pub module_name: String,
    /// Human-readable plan age ("2m ago") or None if no prior plan.
    pub plan_age: Option<String>,
    /// For Apply: `-target=` addresses of the cached plan (empty = full plan).
    pub plan_targets: Vec<String>,
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
    /// Targeted apply: single `apply -auto-approve` with all -target flags.
    TargetedApply,
}

impl ExplorerOpKind {
    /// Terraform sub-command to pass to runner.
    pub fn command(self) -> &'static str {
        match self {
            Self::Taint => "taint",
            Self::StateRm => "state",
            Self::TargetedDestroy => "destroy",
            Self::TargetedApply => "apply",
        }
    }

    /// Extra args before the resource address (sequential ops only).
    pub fn pre_args(self) -> &'static [&'static str] {
        match self {
            Self::Taint => &[],
            Self::StateRm => &["rm"],
            Self::TargetedDestroy => &[],
            Self::TargetedApply => &[],
        }
    }

    pub fn confirm_title(self) -> &'static str {
        match self {
            Self::Taint => " Confirm Taint ",
            Self::StateRm => " Confirm Remove from State ",
            Self::TargetedDestroy => " ⚠  Confirm Targeted Destroy ",
            Self::TargetedApply => " ⚠  Confirm Targeted Apply ",
        }
    }

    pub fn confirm_verb(self) -> &'static str {
        match self {
            Self::Taint => "Taint",
            Self::StateRm => "Remove from state",
            Self::TargetedDestroy => "DESTROY (targeted)",
            Self::TargetedApply => "APPLY (targeted)",
        }
    }

    pub fn progress_title(self) -> &'static str {
        match self {
            Self::Taint => " Taint Progress ",
            Self::StateRm => " State Remove Progress ",
            Self::TargetedDestroy => " Targeted Destroy ",
            Self::TargetedApply => " Targeted Apply ",
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
            (Self::TargetedApply, true) => " Targeted Apply Complete ",
            (Self::TargetedApply, false) => " Targeted Apply Failed ",
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
    /// Run output area left column (mouse hit-testing).
    pub output_left: u16,
    /// Run output area width in columns (wrap math, mouse hit-testing).
    pub output_width: u16,
}

/// One rendered row of the state explorer's grouped list: either a selectable
/// module header, or a resource line (indented when it belongs to a group).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExplorerRow {
    ModuleHeader {
        prefix: String,
        count: usize,
    },
    /// `res_idx` is the UNFILTERED index into `StateContent::Resources`.
    Resource {
        res_idx: usize,
        indent: bool,
    },
}

/// Build the grouped explorer row list from `resources` under `filter`.
///
/// Root resources (no module prefix) come first in original order, then one
/// group per distinct module prefix (sorted lexicographically) with its
/// members in original order. A group header is emitted only when at least one
/// member survives the (case-insensitive substring) filter; `count` is the
/// number of surviving members.
pub fn build_explorer_rows(
    resources: &[crate::state::StateResource],
    filter: &str,
) -> Vec<ExplorerRow> {
    let filter_lower = filter.to_lowercase();
    let matches = |addr: &str| filter.is_empty() || addr.to_lowercase().contains(&filter_lower);

    let mut rows = Vec::new();

    // Root resources first, in original order.
    for (idx, r) in resources.iter().enumerate() {
        if crate::state::module_prefix(&r.address).is_none() && matches(&r.address) {
            rows.push(ExplorerRow::Resource {
                res_idx: idx,
                indent: false,
            });
        }
    }

    // Distinct module prefixes, sorted lexicographically.
    let mut prefixes: Vec<String> = resources
        .iter()
        .filter_map(|r| crate::state::module_prefix(&r.address))
        .collect();
    prefixes.sort();
    prefixes.dedup();

    for prefix in prefixes {
        let members: Vec<usize> = resources
            .iter()
            .enumerate()
            .filter(|(_, r)| {
                crate::state::module_prefix(&r.address).as_deref() == Some(prefix.as_str())
                    && matches(&r.address)
            })
            .map(|(i, _)| i)
            .collect();
        if members.is_empty() {
            continue;
        }
        rows.push(ExplorerRow::ModuleHeader {
            prefix,
            count: members.len(),
        });
        for idx in members {
            rows.push(ExplorerRow::Resource {
                res_idx: idx,
                indent: true,
            });
        }
    }

    rows
}

/// State shown in the state-explorer for a single module.
pub struct StateExplorer {
    pub module_idx: usize,
    pub module_name: String,
    pub content: StateContent,
    /// Index into the grouped row list (`rows()`) that is currently highlighted.
    pub selected: usize,
    /// Current filter string (case-insensitive substring match against addresses).
    pub filter: String,
    /// Whether the filter input is actively receiving keystrokes.
    pub filter_active: bool,
    /// When `Some`, the view shows resource detail instead of the resource list.
    pub detail_view: Option<ResourceDetail>,
    /// Unfiltered resource indices that are multi-selected.
    pub multi_select: Vec<usize>,
    /// Whole module prefixes that are selected (targets the module as a unit).
    pub module_select: Vec<String>,
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

    /// The grouped row list under the current filter (empty unless resources
    /// are loaded).
    pub fn rows(&self) -> Vec<ExplorerRow> {
        match self.resources() {
            Some(resources) => build_explorer_rows(resources, &self.filter),
            None => Vec::new(),
        }
    }

    /// Number of rows currently visible (headers included).
    pub fn row_count(&self) -> usize {
        self.rows().len()
    }

    /// The currently highlighted row, if any.
    pub fn selected_row(&self) -> Option<ExplorerRow> {
        self.rows().into_iter().nth(self.selected)
    }

    /// Addresses an operation should target (`-target=` / `state rm` accept
    /// module addresses too). If anything is selected: the deduped selected
    /// module prefixes (a prefix covered by another selected prefix is dropped),
    /// sorted, followed by every multi-selected resource address NOT already
    /// covered by a selected prefix. Otherwise: the highlighted row — a header
    /// yields its prefix, a resource its address.
    pub fn target_addresses(&self) -> Vec<String> {
        let Some(resources) = self.resources() else {
            return Vec::new();
        };
        if !self.module_select.is_empty() || !self.multi_select.is_empty() {
            let mut prefixes: Vec<String> = self
                .module_select
                .iter()
                .filter(|p| {
                    !self.module_select.iter().any(|other| {
                        other.as_str() != p.as_str() && crate::state::is_covered_by(p, other)
                    })
                })
                .cloned()
                .collect();
            prefixes.sort();
            prefixes.dedup();

            let mut out = prefixes.clone();
            for &i in &self.multi_select {
                if let Some(r) = resources.get(i) {
                    if !prefixes
                        .iter()
                        .any(|p| crate::state::is_covered_by(&r.address, p))
                    {
                        out.push(r.address.clone());
                    }
                }
            }
            out
        } else {
            match self.selected_row() {
                Some(ExplorerRow::ModuleHeader { prefix, .. }) => vec![prefix],
                Some(ExplorerRow::Resource { res_idx, .. }) => resources
                    .get(res_idx)
                    .map(|r| r.address.clone())
                    .into_iter()
                    .collect(),
                None => Vec::new(),
            }
        }
    }

    /// Addresses to pass to an operation of `kind`. For `Taint` (which accepts
    /// only single resource addresses) each selected module prefix is expanded
    /// into its managed member addresses, skipping data sources. For every
    /// other kind, module addresses are valid so `target_addresses()` is used
    /// verbatim.
    pub fn op_targets_for(&self, kind: ExplorerOpKind) -> Vec<String> {
        match kind {
            ExplorerOpKind::Taint => {
                let Some(resources) = self.resources() else {
                    return Vec::new();
                };
                let mut out = Vec::new();
                for addr in self.target_addresses() {
                    // A module prefix is never itself a full resource address.
                    if resources.iter().any(|r| r.address == addr) {
                        out.push(addr);
                    } else {
                        for r in resources {
                            if crate::state::is_covered_by(&r.address, &addr)
                                && !crate::state::is_data_address(&r.address)
                            {
                                out.push(r.address.clone());
                            }
                        }
                    }
                }
                out
            }
            _ => self.target_addresses(),
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
    /// Display-row layout cache for the Run output pane, kept in sync with
    /// the display task/geometry every render pass (`sync_output_layout`).
    pub output_layout: OutputLayout,
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
            output_layout: OutputLayout::default(),
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
        let frame = (self.spinner_tick as usize / 2) % crate::ui::theme::SPINNER_FRAMES.len();
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
        display_task(&self.engine, self.session.as_ref(), path)
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

    /// Id of the task whose output `run_output_lines` returns, if any.
    pub fn run_display_task_id(&self) -> Option<usize> {
        self.session
            .as_ref()
            .and_then(|s| s.modules.get(s.cursor))
            .and_then(|m| self.display_task_for(&m.path))
            .map(|(t, _)| t.id)
    }

    /// Bring `self.output_layout` up to date with the highlighted board
    /// module's display-task output at the given pane geometry.
    ///
    /// Routed through the free `display_task` helper (rather than
    /// `self.display_task_for`) because `lines` borrows `self.engine` while
    /// `self.output_layout` needs a simultaneous `&mut` borrow; destructuring
    /// `self` here lets the borrow checker see the two fields as disjoint.
    pub fn sync_output_layout(&mut self, width: u16, wrap: bool) {
        let App {
            engine,
            session,
            output_layout,
            ..
        } = self;
        let session_ref = session.as_ref();
        let display = session_ref
            .and_then(|s| s.modules.get(s.cursor))
            .and_then(|m| display_task(engine, session_ref, &m.path));
        let task_id = display.as_ref().map(|(t, _)| t.id);
        let lines: &[String] = display
            .map(|(t, _)| t.output_lines.as_slice())
            .unwrap_or(&[]);
        output_layout.sync(task_id, lines, width, wrap);

        // A selection anchored to a display task that's no longer showing
        // (cursor moved to a different module, or the module's display task
        // changed) is no longer meaningful — drop it.
        if let Some(s) = session.as_mut() {
            if s.selection.is_some_and(|sel| sel.task != task_id) {
                s.selection = None;
            }
            if s.pending_sel.is_some_and(|p| p.task != task_id) {
                s.pending_sel = None;
            }
        }
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
            s.selection = None;
            s.pending_sel = None;
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
            s.selection = None;
            s.pending_sel = None;
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

    /// Scroll the Run output pane by `delta` DISPLAY rows (positive = scroll
    /// up toward older lines). Clamped to `output_layout`'s total display-row
    /// count minus the pane height recorded at the last draw
    /// (`viewport.output`). Returns true if it moved.
    pub fn run_scroll_output(&mut self, delta: i32) -> bool {
        let max = self
            .output_layout
            .total_rows()
            .saturating_sub(self.viewport.output as usize);
        let Some(s) = self.session.as_mut() else {
            return false;
        };
        let before = s.output_scroll;
        if delta < 0 {
            s.output_scroll = s.output_scroll.saturating_sub((-delta) as usize);
        } else {
            s.output_scroll = s.output_scroll.saturating_add(delta as usize).min(max);
        }
        s.output_scroll != before
    }

    /// Scroll the Run output pane all the way up (oldest content), at the
    /// pane geometry recorded at the last draw.
    pub fn run_scroll_to_top(&mut self) {
        let max = self
            .output_layout
            .total_rows()
            .saturating_sub(self.viewport.output as usize);
        if let Some(s) = self.session.as_mut() {
            s.output_scroll = max;
        }
    }

    /// Scroll the Run output pane back to the tail (0 = follow tail).
    pub fn run_scroll_to_bottom(&mut self) {
        if let Some(s) = self.session.as_mut() {
            s.output_scroll = 0;
        }
    }

    /// Map a mouse (column,row) to a content position in the output pane, using
    /// the geometry recorded by the last draw. `clamp`: coordinates outside the
    /// pane clamp to the nearest edge (used for drags); when false, misses
    /// return None (used for the initial press).
    pub fn output_hit_test(&self, col: u16, row: u16, clamp: bool) -> Option<SelPos> {
        let vp = self.viewport;
        if vp.output_top == u16::MAX || vp.output_width == 0 {
            return None;
        }
        let total = self.output_layout.total_rows();
        if total == 0 {
            return None;
        }
        let session = self.session.as_ref()?;

        // Same first_row math as the renderers.
        let max_scroll = total.saturating_sub(vp.output as usize);
        let scroll = session.output_scroll.min(max_scroll);
        let first_row = max_scroll - scroll;

        let bottom = vp.output_top.saturating_add(vp.output);
        let right = vp.output_left.saturating_add(vp.output_width);

        let (row, col) = if clamp {
            (
                row.clamp(vp.output_top, bottom.saturating_sub(1)),
                col.clamp(vp.output_left, right.saturating_sub(1)),
            )
        } else {
            if row < vp.output_top || row >= bottom || col < vp.output_left || col >= right {
                return None;
            }
            (row, col)
        };

        let display_row = first_row + (row - vp.output_top) as usize;
        let (line, row_in_line, cell) = if display_row >= total {
            if !clamp {
                return None;
            }
            // Past the last display row: clamp to the end of the last line
            // (char_at_cell below clamps an overflowing cell to range.end).
            let (line, row_in_line) = self.output_layout.locate(total - 1)?;
            (line, row_in_line, usize::MAX)
        } else {
            let (line, row_in_line) = self.output_layout.locate(display_row)?;
            (line, row_in_line, (col - vp.output_left) as usize)
        };

        let lines = self.run_output_lines();
        let raw = lines.get(line)?;
        let stripped = crate::util::strip_ansi(raw);
        let ranges =
            crate::ui::output_layout::wrap_ranges(&stripped, vp.output_width, session.output_wrap);
        let row_range = ranges.get(row_in_line)?.clone();
        let ch = crate::ui::output_layout::char_at_cell(&stripped, row_range, cell);

        Some(SelPos { line, ch })
    }

    /// Record a press anchor on output content, anchored to the current
    /// display task (so it's discarded if the display task changes
    /// underneath it). Clears any existing selection (click-to-deselect) but
    /// creates none of its own — see `run_selection_begin_from_pending`.
    pub fn run_selection_arm(&mut self, pos: SelPos) {
        let task = self.run_display_task_id();
        if let Some(s) = self.session.as_mut() {
            s.selection = None;
            s.pending_sel = Some(PendingSel { task, pos });
        }
    }

    /// Promote the pending press anchor into a real dragging selection.
    /// Consumes the pending anchor; returns false (and creates nothing) if
    /// none exists or its display task no longer matches the current one.
    pub fn run_selection_begin_from_pending(&mut self) -> bool {
        let task = self.run_display_task_id();
        let Some(s) = self.session.as_mut() else {
            return false;
        };
        let Some(p) = s.pending_sel.take() else {
            return false;
        };
        if p.task != task {
            return false;
        }
        s.selection = Some(OutputSelection {
            task,
            anchor: p.pos,
            head: p.pos,
            dragging: true,
        });
        true
    }

    /// Discard the pending press anchor, if any. Returns whether one existed.
    pub fn run_discard_pending_sel(&mut self) -> bool {
        let Some(s) = self.session.as_mut() else {
            return false;
        };
        s.pending_sel.take().is_some()
    }

    /// Extend the in-progress drag selection to `pos`. No-op unless a
    /// selection exists and is currently dragging.
    pub fn run_selection_drag(&mut self, pos: SelPos) {
        if let Some(sel) = self.session.as_mut().and_then(|s| s.selection.as_mut()) {
            if sel.dragging {
                sel.head = pos;
            }
        }
    }

    /// End the in-progress drag, leaving the selected range in place — unless
    /// the drag never moved, in which case the zero-length selection is
    /// dropped rather than left lingering to eat the next Esc.
    pub fn run_selection_end(&mut self) {
        if let Some(s) = self.session.as_mut() {
            if let Some(sel) = s.selection.as_mut() {
                sel.dragging = false;
                if sel.is_empty() {
                    s.selection = None;
                }
            }
        }
    }

    /// Clear any selection and any not-yet-promoted press anchor.
    pub fn run_clear_selection(&mut self) {
        if let Some(s) = self.session.as_mut() {
            s.selection = None;
            s.pending_sel = None;
        }
    }

    /// ANSI-stripped selected text, lines joined with '\n'. None when there is
    /// no selection, it is empty, or its task no longer matches.
    pub fn run_selected_text(&self) -> Option<String> {
        let session = self.session.as_ref()?;
        let sel = session.selection?;
        if sel.is_empty() || sel.task != self.run_display_task_id() {
            return None;
        }
        let (a, b) = sel.ordered();
        let lines = self.run_output_lines();

        if a.line == b.line {
            let stripped = crate::util::strip_ansi(lines.get(a.line)?);
            let chars: Vec<char> = stripped.chars().collect();
            let lo = a.ch.min(chars.len());
            let hi = b.ch.min(chars.len());
            return Some(chars[lo..hi].iter().collect());
        }

        let mut out = Vec::new();
        if let Some(raw) = lines.get(a.line) {
            let stripped = crate::util::strip_ansi(raw);
            let chars: Vec<char> = stripped.chars().collect();
            let lo = a.ch.min(chars.len());
            out.push(chars[lo..].iter().collect::<String>());
        }
        for line in lines.iter().take(b.line).skip(a.line + 1) {
            out.push(crate::util::strip_ansi(line));
        }
        if let Some(raw) = lines.get(b.line) {
            let stripped = crate::util::strip_ansi(raw);
            let chars: Vec<char> = stripped.chars().collect();
            let hi = b.ch.min(chars.len());
            out.push(chars[..hi].iter().collect::<String>());
        }

        Some(out.join("\n"))
    }

    /// ANSI-stripped full output of the highlighted task, lines joined with '\n'.
    /// None when there are no output lines.
    pub fn run_all_output_text(&self) -> Option<String> {
        let lines = self.run_output_lines();
        if lines.is_empty() {
            return None;
        }
        let stripped: Vec<String> = lines
            .iter()
            .map(|line| crate::util::strip_ansi(line))
            .collect();
        Some(stripped.join("\n"))
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

    /// Make an explorer-launched task first-class on the Run board: ensure the
    /// module is part of a session and record the task as its latest.
    ///
    /// If no session exists one is created containing just this module WITHOUT
    /// switching screens (the user stays where they are; Tab/Enter reach it as
    /// usual). If a session exists but doesn't include the module, the module is
    /// appended (a push keeps existing cursor/selected indices — which point at
    /// earlier positions — valid). Either way `latest_task` is updated so the
    /// board row shows the task.
    fn record_explorer_task(&mut self, module_idx: usize, task_id: usize) {
        let Some(module) = self.modules.get(module_idx) else {
            return;
        };
        let path = module.path.clone();
        let name = module.display_name.clone();

        match self.session.as_mut() {
            Some(session) => {
                if !session.modules.iter().any(|m| m.path == path) {
                    session.modules.push(SessionModule {
                        module_idx,
                        path: path.clone(),
                        name,
                    });
                }
                session.latest_task.insert(path, task_id);
            }
            None => {
                let mut session = RunSession::new(vec![SessionModule {
                    module_idx,
                    path: path.clone(),
                    name,
                }]);
                session.latest_task.insert(path, task_id);
                self.session = Some(session);
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
            let plan_path = self
                .engine
                .plan_cache
                .plan_path_for(&self.modules[idx].path);
            let args = vec!["-out".to_string(), plan_path.to_string_lossy().into_owned()];
            ids.push(self.push_task_for(idx, "plan", args, Some(plan_path), Vec::new(), None));
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
            ids.push(self.push_task_for(idx, command, extra_args.clone(), None, Vec::new(), None));
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
            ids.push(self.push_task_for(idx, "apply", args, None, Vec::new(), cleanup_plan_path));
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
                Vec::new(),
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
        targets: Vec<String>,
        cleanup_plan_path: Option<PathBuf>,
    ) -> usize {
        let module = &self.modules[module_idx];
        self.engine.push_task(TaskSpec {
            module_path: module.path.clone(),
            module_name: module.display_name.clone(),
            command: command.to_string(),
            args,
            plan_output_path,
            targets,
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
            ConfirmKind::Apply => {
                let entry = self.engine.plan_cache.get(&m.path);
                Some(ConfirmTarget {
                    module_idx: idx,
                    module_name: m.display_name.clone(),
                    plan_age: entry.map(|e| e.age_str()),
                    plan_targets: entry.map(|e| e.targets.clone()).unwrap_or_default(),
                    lock_id: None,
                    lock_who: None,
                })
            }
            ConfirmKind::Destroy | ConfirmKind::InitUpgrade => Some(ConfirmTarget {
                module_idx: idx,
                module_name: m.display_name.clone(),
                plan_age: None,
                plan_targets: Vec::new(),
                lock_id: None,
                lock_who: None,
            }),
            ConfirmKind::ForceUnlock => {
                let lock = read_lock_info(&m.path).or_else(|| self.detect_lock_from_tasks(idx))?;
                Some(ConfirmTarget {
                    module_idx: idx,
                    module_name: m.display_name.clone(),
                    plan_age: None,
                    plan_targets: Vec::new(),
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
            ids.push(self.push_task_for(
                idx,
                "init",
                vec!["-upgrade".to_string()],
                None,
                Vec::new(),
                None,
            ));
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
                Vec::new(),
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
            module_select: Vec::new(),
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
        let count = explorer.row_count();
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
            let count = explorer.row_count();
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
        let count = explorer.row_count();
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
            // Headers have no detail view — only resource rows drill down.
            let res_idx = match explorer.selected_row()? {
                ExplorerRow::Resource { res_idx, .. } => res_idx,
                ExplorerRow::ModuleHeader { .. } => return None,
            };
            let resources = explorer.resources()?;
            let resource = resources.get(res_idx)?;
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

    /// Toggle the currently highlighted row in the selection: a resource row
    /// toggles its index in `multi_select`; a module header toggles its prefix
    /// in `module_select`, and selecting a prefix drops any individually
    /// selected member it now covers (redundancy rule).
    pub fn state_explorer_toggle_select(&mut self) {
        let Some(explorer) = &mut self.state_explorer else {
            return;
        };
        match explorer.selected_row() {
            Some(ExplorerRow::Resource { res_idx, .. }) => {
                if let Some(pos) = explorer.multi_select.iter().position(|&i| i == res_idx) {
                    explorer.multi_select.remove(pos);
                } else {
                    explorer.multi_select.push(res_idx);
                }
            }
            Some(ExplorerRow::ModuleHeader { prefix, .. }) => {
                if let Some(pos) = explorer.module_select.iter().position(|p| p == &prefix) {
                    explorer.module_select.remove(pos);
                } else {
                    // Compute covered members under an immutable borrow first.
                    let covered: Vec<usize> = match &explorer.content {
                        StateContent::Resources(rs) => explorer
                            .multi_select
                            .iter()
                            .copied()
                            .filter(|&i| {
                                rs.get(i)
                                    .map(|r| crate::state::is_covered_by(&r.address, &prefix))
                                    .unwrap_or(false)
                            })
                            .collect(),
                        _ => Vec::new(),
                    };
                    explorer.multi_select.retain(|i| !covered.contains(i));
                    explorer.module_select.push(prefix);
                }
            }
            None => {}
        }
    }

    pub fn state_explorer_clear_select(&mut self) {
        if let Some(explorer) = &mut self.state_explorer {
            explorer.multi_select.clear();
            explorer.module_select.clear();
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
        let task_id = self.push_task_for(
            module_idx,
            "plan",
            args,
            Some(plan_path),
            targets.clone(),
            None,
        );
        self.record_explorer_task(module_idx, task_id);
        if let Some(explorer) = self.state_explorer.as_mut() {
            explorer.plan_queued_notice = true;
        }
    }

    /// Stage a confirmation for the given operation on the selected (or multi-selected) resources.
    pub fn request_op_confirm(&mut self, kind: ExplorerOpKind) {
        let Some(explorer) = &mut self.state_explorer else {
            return;
        };
        let targets = explorer.op_targets_for(kind);
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
                let task_id = self.push_task_for(
                    module_idx,
                    kind.command(),
                    args,
                    None,
                    vec![first.clone()],
                    None,
                );
                self.record_explorer_task(module_idx, task_id);
                if let Some(explorer) = self.state_explorer.as_mut() {
                    explorer.pending_op = Some(PendingOp {
                        kind,
                        queue: remaining,
                        running: Some((task_id, first)),
                        done: Vec::new(),
                    });
                    explorer.multi_select.clear();
                    explorer.module_select.clear();
                }
            }
            ExplorerOpKind::TargetedDestroy | ExplorerOpKind::TargetedApply => {
                // Single batch: all -target flags in one command. Runs
                // `<command> -auto-approve -target=…` directly; a targeted apply
                // deliberately does NOT consume the module's cached plan entry.
                let n = targets.len();
                let task_targets = targets.clone();
                let mut args = vec!["-auto-approve".to_string()];
                for addr in &targets {
                    args.push(format!("-target={}", addr));
                }
                let label = if n == 1 {
                    targets.remove(0)
                } else {
                    format!("{} targeted resources", n)
                };
                let task_id =
                    self.push_task_for(module_idx, kind.command(), args, None, task_targets, None);
                self.record_explorer_task(module_idx, task_id);
                if let Some(explorer) = self.state_explorer.as_mut() {
                    explorer.pending_op = Some(PendingOp {
                        kind,
                        queue: vec![],
                        running: Some((task_id, label)),
                        done: Vec::new(),
                    });
                    explorer.multi_select.clear();
                    explorer.module_select.clear();
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
                let new_task_id = self.push_task_for(
                    module_idx,
                    kind.command(),
                    args,
                    None,
                    vec![addr.clone()],
                    None,
                );
                self.record_explorer_task(module_idx, new_task_id);
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
        explorer.module_select.clear();
        let count = explorer.row_count();
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

/// Shared resolution behind `App::display_task_for`: the task shown for
/// `path`, plus whether it is a background task from a previous session.
/// Resolution: `session`'s latest task for the path, else the newest
/// non-terminal task for it in `engine`, else none (idle).
///
/// A free function (rather than an `&self` method) so callers that need a
/// simultaneous `&mut` borrow of another `App` field — see
/// `App::sync_output_layout` — can call it without borrowing all of `self`.
fn display_task<'a>(
    engine: &'a TaskEngine,
    session: Option<&RunSession>,
    path: &Path,
) -> Option<(&'a Task, bool)> {
    if let Some(s) = session {
        if let Some(&id) = s.latest_task.get(path) {
            if let Some(t) = engine.task(id) {
                return Some((t, false));
            }
        }
    }
    engine
        .tasks
        .iter()
        .filter(|t| t.module_path == *path && !t.status.is_terminal())
        .max_by_key(|t| t.id)
        .map(|t| (t, true))
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
            ..Default::default()
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
            targets: Vec::new(),
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
            ..Default::default()
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
            .register(module_path, plan_path, app.engine.tasks[1].id, Vec::new());

        assert!(app.ready_plan_for_task(&app.engine.tasks[0]).is_none());
        assert!(app.ready_plan_for_task(&app.engine.tasks[1]).is_some());
    }

    // ── State explorer grouped rows & module targeting ───────────────────────

    fn explorer_with(addrs: &[&str]) -> StateExplorer {
        let resources = addrs
            .iter()
            .map(|a| crate::state::StateResource {
                address: a.to_string(),
                instance: serde_json::json!({}),
            })
            .collect();
        StateExplorer {
            module_idx: 0,
            module_name: "app".to_string(),
            content: StateContent::Resources(resources),
            selected: 0,
            filter: String::new(),
            filter_active: false,
            detail_view: None,
            multi_select: Vec::new(),
            module_select: Vec::new(),
            op_confirm: None,
            op_targets: Vec::new(),
            pending_op: None,
            op_result: None,
            plan_queued_notice: false,
            load_rx: None,
        }
    }

    #[test]
    fn build_explorer_rows_groups_root_first_and_sorts() {
        let ex = explorer_with(&[
            "aws_vpc.main",
            "module.net.null_resource.a",
            "module.app.null_resource.b",
            "module.net.null_resource.c",
        ]);
        assert_eq!(
            ex.rows(),
            vec![
                ExplorerRow::Resource {
                    res_idx: 0,
                    indent: false
                },
                ExplorerRow::ModuleHeader {
                    prefix: "module.app".into(),
                    count: 1
                },
                ExplorerRow::Resource {
                    res_idx: 2,
                    indent: true
                },
                ExplorerRow::ModuleHeader {
                    prefix: "module.net".into(),
                    count: 2
                },
                ExplorerRow::Resource {
                    res_idx: 1,
                    indent: true
                },
                ExplorerRow::Resource {
                    res_idx: 3,
                    indent: true
                },
            ]
        );
    }

    #[test]
    fn build_explorer_rows_filter_prunes_empty_groups_and_adjusts_count() {
        let mut ex = explorer_with(&[
            "module.net.null_resource.alpha",
            "module.net.null_resource.beta",
            "module.app.null_resource.c",
        ]);
        ex.filter = "alpha".to_string();
        // module.app pruned; module.net count drops to the single match.
        assert_eq!(
            ex.rows(),
            vec![
                ExplorerRow::ModuleHeader {
                    prefix: "module.net".into(),
                    count: 1
                },
                ExplorerRow::Resource {
                    res_idx: 0,
                    indent: true
                },
            ]
        );
    }

    #[test]
    fn header_toggle_drops_covered_member() {
        let mut app = test_app();
        app.state_explorer = Some(explorer_with(&[
            "module.net.null_resource.a",
            "module.net.null_resource.b",
        ]));
        // rows: [Header, Res0, Res1]. Select member Res0 first.
        app.state_explorer.as_mut().unwrap().selected = 1;
        app.state_explorer_toggle_select();
        assert_eq!(app.state_explorer.as_ref().unwrap().multi_select, vec![0]);
        // Now select the header — the covered member is dropped.
        app.state_explorer.as_mut().unwrap().selected = 0;
        app.state_explorer_toggle_select();
        let ex = app.state_explorer.as_ref().unwrap();
        assert!(ex.multi_select.is_empty());
        assert_eq!(ex.module_select, vec!["module.net".to_string()]);
    }

    #[test]
    fn target_addresses_dedups_covered_prefixes_and_members() {
        let mut ex = explorer_with(&[
            "aws_vpc.main",
            "module.a.null_resource.m",
            "module.a.module.b.null_resource.n",
        ]);
        ex.module_select = vec!["module.a".into(), "module.a.module.b".into()];
        ex.multi_select = vec![0, 1];
        assert_eq!(
            ex.target_addresses(),
            vec!["module.a".to_string(), "aws_vpc.main".to_string()]
        );
    }

    #[test]
    fn op_targets_taint_expands_module_skipping_data() {
        let mut ex = explorer_with(&[
            "module.net.null_resource.a",
            "module.net.data.aws_ami.u",
            "module.net.null_resource.b",
        ]);
        ex.module_select = vec!["module.net".into()];
        assert_eq!(
            ex.op_targets_for(ExplorerOpKind::Taint),
            vec![
                "module.net.null_resource.a".to_string(),
                "module.net.null_resource.b".to_string(),
            ]
        );
        // Non-taint ops accept the module address itself.
        assert_eq!(
            ex.op_targets_for(ExplorerOpKind::StateRm),
            vec!["module.net".to_string()]
        );
    }

    #[test]
    fn open_detail_noop_on_header() {
        let mut app = test_app();
        app.state_explorer = Some(explorer_with(&["module.net.null_resource.a"]));
        // rows: [Header, Res0]. Cursor on the header → no detail opens.
        app.state_explorer.as_mut().unwrap().selected = 0;
        app.open_resource_detail();
        assert!(app.state_explorer.as_ref().unwrap().detail_view.is_none());
        // On the resource row it opens.
        app.state_explorer.as_mut().unwrap().selected = 1;
        app.open_resource_detail();
        assert!(app.state_explorer.as_ref().unwrap().detail_view.is_some());
    }

    // ── Targeted plan/apply threading (Package 2) ─────────────────────────────

    /// App whose engine spawns `echo` (in a real temp dir) instead of a real
    /// binary. `echo` prints the command line back on stdout, so the `-target=`
    /// args a task was spawned with surface in `Task::output_lines` — the only
    /// way to observe them (`Task` doesn't retain `args`).
    fn echo_app_at(dir: PathBuf) -> App {
        let modules = vec![Module {
            path: dir.clone(),
            display_name: "app".to_string(),
            kind: ModuleKind::Root,
        }];
        let config = Config {
            binary: "echo".to_string(),
            parallelism: 1,
            ignore_dirs: Vec::new(),
            show_library_modules: false,
            ..Default::default()
        };
        App::new(config, dir, modules).unwrap()
    }

    /// Drive the engine to quiescence (like the engine's own queue test).
    async fn drain_engine(app: &mut App) {
        use std::time::Duration;
        let drain = async {
            while app.engine.has_active_tasks() {
                app.engine.next_update().await;
            }
        };
        tokio::time::timeout(Duration::from_secs(10), drain)
            .await
            .expect("engine did not settle within 10s");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn targeted_plan_threads_target_args_and_plan_targets() {
        let tmp = tempfile::tempdir().unwrap();
        let mut app = echo_app_at(tmp.path().to_path_buf());
        // rows: [Header(module.net), Res0]; highlight the header → whole-module
        // target.
        app.state_explorer = Some(explorer_with(&["module.net.null_resource.a"]));
        app.state_explorer.as_mut().unwrap().selected = 0;

        app.enqueue_targeted_plan();

        // Metadata (`targets`) is threaded synchronously at enqueue time.
        let id = app.engine.tasks.iter().max_by_key(|t| t.id).unwrap().id;
        assert_eq!(
            app.engine.task(id).unwrap().targets,
            vec!["module.net".to_string()]
        );

        // The `-target=` CLI arg is observable only via echoed output.
        drain_engine(&mut app).await;
        let task = app.engine.task(id).unwrap();
        assert_eq!(task.command, "plan");
        assert!(task
            .output_lines
            .iter()
            .any(|l| l.contains("-target=module.net")));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn targeted_apply_runs_batched_and_keeps_cached_plan() {
        let tmp = tempfile::tempdir().unwrap();
        let mut app = echo_app_at(tmp.path().to_path_buf());
        let module_path = app.modules[0].path.clone();

        // Pre-register a cached plan entry; a targeted apply must NOT consume it.
        let plan_path = app.engine.plan_cache.plan_path_for(&module_path);
        app.engine.plan_cache.register(
            module_path.clone(),
            plan_path,
            99,
            vec!["null_resource.a".into()],
        );
        let before = app.engine.plan_cache.entry_count();

        // Stage a targeted apply for a whole module.
        let mut explorer = explorer_with(&["module.net.null_resource.a"]);
        explorer.op_confirm = Some(ExplorerOpKind::TargetedApply);
        explorer.op_targets = vec!["module.net".to_string()];
        explorer.module_select = vec!["module.net".to_string()];
        app.state_explorer = Some(explorer);

        app.start_op();

        // A single batched apply task; PendingOp is set and selection cleared.
        let id = app.engine.tasks.iter().max_by_key(|t| t.id).unwrap().id;
        assert_eq!(app.engine.task(id).unwrap().command, "apply");
        let ex = app.state_explorer.as_ref().unwrap();
        assert!(ex.pending_op.is_some());
        assert!(ex.module_select.is_empty());

        // The cached plan entry survives (apply ran -target directly, not from it).
        assert_eq!(app.engine.plan_cache.entry_count(), before);

        // Batched form: `-auto-approve -target=…`, in that order.
        drain_engine(&mut app).await;
        assert!(app
            .engine
            .task(id)
            .unwrap()
            .output_lines
            .iter()
            .any(|l| l.contains("-auto-approve -target=module.net")));
    }

    #[test]
    fn apply_confirm_carries_plan_targets() {
        let mut app = test_app();
        let module_path = app.modules[0].path.clone();
        let plan_path = app.engine.plan_cache.plan_path_for(&module_path);
        app.engine.plan_cache.register(
            module_path,
            plan_path,
            7,
            vec!["module.net".into(), "null_resource.a".into()],
        );

        app.request_apply_confirm(&[0]);

        let Some(Modal::Confirm(confirm)) = &app.modal else {
            panic!("apply confirm not staged");
        };
        assert_eq!(confirm.kind, ConfirmKind::Apply);
        assert_eq!(
            confirm.targets[0].plan_targets,
            vec!["module.net".to_string(), "null_resource.a".to_string()]
        );
    }

    // ── Explorer tasks become first-class on the Run board (Package 3) ────────

    /// Multi-module `echo` app; each module lives in a real subdir of `root`.
    fn echo_multi_app(root: PathBuf, n: usize) -> App {
        let modules: Vec<Module> = (0..n)
            .map(|i| {
                let path = root.join(format!("m{i}"));
                std::fs::create_dir_all(&path).unwrap();
                Module {
                    path,
                    display_name: format!("m{i}"),
                    kind: ModuleKind::Root,
                }
            })
            .collect();
        let config = Config {
            binary: "echo".to_string(),
            parallelism: 1,
            ignore_dirs: Vec::new(),
            show_library_modules: false,
            ..Default::default()
        };
        App::new(config, root, modules).unwrap()
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn explorer_plan_creates_session_without_switching_screen() {
        let tmp = tempfile::tempdir().unwrap();
        let mut app = echo_app_at(tmp.path().to_path_buf());
        assert!(app.session.is_none());
        assert_eq!(app.screen, Screen::Select);

        app.state_explorer = Some(explorer_with(&["module.net.null_resource.a"]));
        app.state_explorer.as_mut().unwrap().selected = 0;
        app.enqueue_targeted_plan();

        // A session was created holding just this module; screen unchanged.
        assert_eq!(app.screen, Screen::Select);
        let path = app.modules[0].path.clone();
        let id = app.engine.tasks.iter().max_by_key(|t| t.id).unwrap().id;
        let session = app.session.as_ref().expect("session created");
        assert_eq!(session.modules.len(), 1);
        assert_eq!(session.modules[0].path, path);
        assert_eq!(session.latest_task.get(&path), Some(&id));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn explorer_apply_appends_module_and_records_latest() {
        let tmp = tempfile::tempdir().unwrap();
        let mut app = echo_multi_app(tmp.path().to_path_buf(), 2);

        // Existing session holding only module 0, with board state set.
        let mut session = RunSession::new(vec![SessionModule {
            module_idx: 0,
            path: app.modules[0].path.clone(),
            name: app.modules[0].display_name.clone(),
        }]);
        session.cursor = 0;
        session.selected = vec![0];
        app.session = Some(session);

        // Targeted apply on module 1 via the explorer.
        let stage_apply = |app: &mut App| {
            let mut explorer = explorer_with(&["null_resource.a"]);
            explorer.module_idx = 1;
            explorer.op_confirm = Some(ExplorerOpKind::TargetedApply);
            explorer.op_targets = vec!["null_resource.a".to_string()];
            app.state_explorer = Some(explorer);
            app.start_op();
        };
        stage_apply(&mut app);

        let m1 = app.modules[1].path.clone();
        let id = app.engine.tasks.iter().max_by_key(|t| t.id).unwrap().id;
        {
            let session = app.session.as_ref().unwrap();
            assert_eq!(session.modules.len(), 2, "module 1 appended");
            assert_eq!(session.modules[1].path, m1);
            // Earlier cursor/selection indices remain valid after the append.
            assert_eq!(session.cursor, 0);
            assert_eq!(session.selected, vec![0]);
            assert_eq!(session.latest_task.get(&m1), Some(&id));
        }

        // Running an op on a module already in the session: no duplicate row,
        // latest_task updated to the newest task.
        stage_apply(&mut app);
        let id2 = app.engine.tasks.iter().max_by_key(|t| t.id).unwrap().id;
        let session = app.session.as_ref().unwrap();
        assert_eq!(session.modules.len(), 2, "module 1 not duplicated");
        assert_eq!(session.latest_task.get(&m1), Some(&id2));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn targeted_ops_carry_targets_full_plan_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let mut app = echo_app_at(tmp.path().to_path_buf());

        // Full (untargeted) plan carries an empty target list.
        let ids = app.enqueue_plan(&[0]);
        assert!(app.engine.task(ids[0]).unwrap().targets.is_empty());

        // Targeted destroy via the explorer carries the `-target=` addresses.
        let mut explorer = explorer_with(&["module.net.null_resource.a"]);
        explorer.op_confirm = Some(ExplorerOpKind::TargetedDestroy);
        explorer.op_targets = vec!["module.net".to_string()];
        app.state_explorer = Some(explorer);
        app.start_op();
        let id = app.engine.tasks.iter().max_by_key(|t| t.id).unwrap().id;
        assert_eq!(app.engine.task(id).unwrap().command, "destroy");
        assert_eq!(
            app.engine.task(id).unwrap().targets,
            vec!["module.net".to_string()]
        );
    }

    // ── Drag-only output selection ─────────────────────────────────────────

    /// A Run session over `n` modules with some running output attached to
    /// module 0 (the initial cursor), so `run_display_task_id()` resolves.
    fn selection_test_app(n: usize) -> App {
        let mut app = multi_module_app(n);
        app.multi_select = (0..n).collect();
        app.enter_run();
        let path = app.session.as_ref().unwrap().modules[0].path.clone();
        let id = app.engine.tasks.len();
        app.engine.tasks.push(Task {
            id,
            module_path: path.clone(),
            module_name: "m0".to_string(),
            command: "apply".to_string(),
            status: TaskStatus::Running,
            output_lines: vec!["hello world".to_string(), "second line".to_string()],
            started_at: Some(Instant::now()),
            finished_at: None,
            plan_output_path: None,
            targets: Vec::new(),
            cleanup_plan_path: None,
            resource_counts: None,
            cancel_handle: None,
        });
        app.session.as_mut().unwrap().latest_task.insert(path, id);
        app
    }

    #[test]
    fn click_arms_but_does_not_select() {
        let mut app = selection_test_app(1);
        let pos = SelPos { line: 0, ch: 0 };

        app.run_selection_arm(pos);
        let session = app.session.as_ref().unwrap();
        assert!(session.selection.is_none(), "a plain press must not select");
        assert!(
            session.pending_sel.is_some(),
            "press should arm a pending anchor"
        );

        assert!(app.run_discard_pending_sel(), "pending anchor existed");
        assert!(app.session.as_ref().unwrap().pending_sel.is_none());
    }

    #[test]
    fn drag_materializes_selection() {
        let mut app = selection_test_app(1);
        let pos = SelPos { line: 0, ch: 0 };
        let pos2 = SelPos { line: 0, ch: 3 };

        app.run_selection_arm(pos);
        assert!(app.run_selection_begin_from_pending());
        {
            let sel = app
                .session
                .as_ref()
                .unwrap()
                .selection
                .expect("selection created");
            assert!(sel.dragging);
            assert_eq!(sel.anchor, pos);
            assert_eq!(sel.head, pos);
        }
        assert!(app.session.as_ref().unwrap().pending_sel.is_none());

        app.run_selection_drag(pos2);
        app.run_selection_end();

        let sel = app
            .session
            .as_ref()
            .unwrap()
            .selection
            .expect("non-empty drag retains its selection");
        assert!(!sel.dragging);
        assert_eq!(sel.head, pos2);
    }

    #[test]
    fn click_deselects_existing() {
        let mut app = selection_test_app(1);
        let pos = SelPos { line: 0, ch: 0 };
        let pos2 = SelPos { line: 0, ch: 3 };

        app.run_selection_arm(pos);
        app.run_selection_begin_from_pending();
        app.run_selection_drag(pos2);
        app.run_selection_end();
        assert!(app.session.as_ref().unwrap().selection.is_some());

        // A fresh press elsewhere clears the old selection outright.
        app.run_selection_arm(SelPos { line: 1, ch: 0 });
        assert!(app.session.as_ref().unwrap().selection.is_none());
        assert!(app.session.as_ref().unwrap().pending_sel.is_some());
    }

    #[test]
    fn empty_drag_drops_selection() {
        let mut app = selection_test_app(1);
        let pos = SelPos { line: 0, ch: 0 };

        app.run_selection_arm(pos);
        assert!(app.run_selection_begin_from_pending());
        // No `run_selection_drag` call: the release lands right where it began.
        app.run_selection_end();

        assert!(
            app.session.as_ref().unwrap().selection.is_none(),
            "a zero-length drag must not leave a lingering selection"
        );
    }

    #[test]
    fn cursor_move_clears_pending() {
        let mut app = selection_test_app(2);
        app.run_selection_arm(SelPos { line: 0, ch: 0 });
        assert!(app.session.as_ref().unwrap().pending_sel.is_some());

        assert!(app.run_move_cursor(1));
        assert!(app.session.as_ref().unwrap().pending_sel.is_none());
    }

    #[test]
    fn stale_task_pending() {
        let mut app = selection_test_app(1);
        app.run_selection_arm(SelPos { line: 0, ch: 0 });

        // Simulate the display task changing out from under the pending
        // anchor without going through a cleanup path that would already
        // discard it — `run_selection_begin_from_pending` itself must refuse.
        app.session
            .as_mut()
            .unwrap()
            .pending_sel
            .as_mut()
            .unwrap()
            .task = Some(999_999);

        assert!(!app.run_selection_begin_from_pending());
        assert!(app.session.as_ref().unwrap().selection.is_none());
    }
}
