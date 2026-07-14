# PLAN A — Structural refactor (execute with Sonnet 5)

## Context for the implementer

You are refactoring **rug**, a Rust terraform/tofu multiplexer TUI at the repo root. It builds clean today. A follow-up plan (Plan B) will completely replace the current 3-pane TUI with a two-screen flow.

### Global do-NOT-touch list (Plan B deletes/replaces these)
- Pane split/drag code: `h_split_col`, `v_split_row`, `DragHandle`, `dragging`, `effective_h_split`, `effective_v_split`, `pane_for_click`, `PaneHeights` (in `src/app.rs` and `src/ui/mod.rs`).
- Rendering internals of `src/ui/tree.rs`, `src/ui/tasks.rs`, `src/ui/output.rs` — only mechanical style substitutions (Step 2) and field-path renames forced by Step 5 (`app.tasks` → `app.engine.tasks`). No restructuring.
- `Focus` enum / Tab cycling, `output_fullscreen`, `output_wrap`; `state_explorer: Option<StateExplorer>` stays a separate overlay (do NOT fold into `Modal`).
- README key-binding docs.
- `parse_ansi` / `apply_sgr` in `src/ui/output.rs` (ANSI passthrough, not theming).
- `src/runner.rs` behavior. The unix-only `libc::kill` at runner.rs:107 has no non-unix fallback — do not fix; add comment: `// NOTE: no non-unix cancel path; SIGINT escalation is unix-only by design.`

### Ground rules
- After **every** step: `cargo build && cargo test` pass with no new warnings. Do steps in order.
- Behavior identical unless a step explicitly says otherwise.
- Baseline before starting: `cargo build`, `cargo test` (6 tests), `cargo run -- --dir fixtures/ list`.

## Step 1 — Shared text utilities: `src/util.rs`

`strip_ansi` is duplicated at `src/task.rs:154` and `src/lock.rs:75`.

1. Create `src/util.rs` with `pub fn strip_ansi(s: &str) -> String` — move the exact body from task.rs:154-171.
2. `mod util;` in `src/main.rs`. Both task.rs and lock.rs delete their copies and `use crate::util::strip_ansi;`.
3. Tests in util.rs: plain text passthrough; strips `\x1b[1m\x1b[32m…\x1b[0m`; handles unterminated `abc\x1b[`.

Accept: one `fn strip_ansi` definition in `src/` (grep).

## Step 2 — Theme module: `src/ui/theme.rs`

Capture every styling convention in one place so Plan B restyles by editing this file only. **Mechanical substitution only — output pixel-identical.**

Create `src/ui/theme.rs` (declare `pub mod theme;` in ui/mod.rs):

```rust
use ratatui::style::{Color, Modifier, Style};
use crate::task::TaskStatus;

pub const ACCENT: Color = Color::Cyan;

pub fn pane_border(focused: bool) -> Style { if focused { Style::default().fg(ACCENT) } else { Style::default() } }
pub fn overlay_border_warn() -> Style { Style::default().fg(Color::Yellow) }     // cancel/clear/reset/quit/help
pub fn overlay_border_danger() -> Style { Style::default().fg(Color::Red) }      // destructive confirms
pub fn overlay_border_success() -> Style { Style::default().fg(Color::Green) }
pub fn overlay_border_explorer() -> Style { Style::default().fg(Color::Blue) }
pub fn overlay_border_filter() -> Style { Style::default().fg(ACCENT) }

pub fn selected_row() -> Style { Style::default().fg(Color::Black).bg(ACCENT).add_modifier(Modifier::BOLD) }
pub fn selected_task_row() -> Style { Style::default().bg(Color::DarkGray).add_modifier(Modifier::BOLD) }
pub fn multi_select_marker() -> Style { Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD) }
pub fn multi_select_item() -> Style { Style::default().fg(Color::Yellow) }
pub fn plan_marker() -> Style { Style::default().fg(Color::Green).add_modifier(Modifier::BOLD) }
pub fn command_text() -> Style { Style::default().fg(Color::Blue) }
pub fn dim() -> Style { Style::default().fg(Color::DarkGray) }

pub fn status_style(status: &TaskStatus) -> Style { /* Pending/Cancelled DarkGray, Running Yellow, Cancelling Magenta, Success Green, Failed Red */ }
/// Presentation icon (moved out of the domain layer — delete TaskStatus::icon()).
pub fn status_icon(status: &TaskStatus) -> &'static str { /* ○ ⟳ ◐ ✓ ✗ ⊘ per current task.rs:19-28 */ }
pub const SPINNER_FRAMES: &[&str] = &["◐", "◓", "◑", "◒"];

pub const COUNT_ADD: Color = Color::Green;
pub const COUNT_CHANGE: Color = Color::Yellow;
pub const COUNT_DESTROY: Color = Color::Red;
pub const COUNT_IMPORT: Color = Color::Cyan;
pub const COUNT_FORGET: Color = Color::DarkGray;
pub const COUNT_NONE: Color = Color::DarkGray;
```

- Move `TaskStatus::icon()` (task.rs:19-28) → `theme::status_icon`; update call sites tasks.rs:53 and mod.rs:1204 (`render_quit_wait`).
- Mechanical substitutions: tree.rs (border, selected row, multi item, plan marker); tasks.rs (border, status match → `status_style`, delete `CANCEL_FRAMES` → `SPINNER_FRAMES`, marker/command/dim/plan-marker/selected-row, `count_spans` colors → `COUNT_*`); output.rs (border ONLY); mod.rs overlay `border_style` calls per the comment mapping above + `fg(Color::DarkGray)` spans → `theme::dim()`.
- Leave alone: `style_json_line`, `json_value_span`, `highlight_filter_match`, and the do-not-touch list.

Accept: zero `Color::` literals left in tree.rs/tasks.rs; task.rs no longer defines `icon()`; TUI looks identical.

## Step 3 — Single `Modal` enum

Six parallel flags on `App` (`show_help`, `pending_confirm`, `pending_cancel_task`, `pending_clear_tasks`, `pending_reset`, `pending_quit`) each have their own interception branch (ui/mod.rs:205-291) and draw check (:642-685). They are already mutually exclusive.

1. In app.rs:
```rust
pub enum Modal {
    Help,
    Confirm(PendingConfirm),
    CancelTasks(Vec<usize>), // always non-empty
    ClearTasks,
    Reset,
    Quit,
}
```
2. Replace the six fields with `pub modal: Option<Modal>`. Update all producers/consumers: `request_*_confirm` → `Modal::Confirm(...)`; `confirm_execute` via `match self.modal.take() { Some(Modal::Confirm(c)) => {…} other => self.modal = other }`; `cancel_staged_tasks` via `let Some(Modal::CancelTasks(ids)) = self.modal.take() else { return };`; `clear_completed_tasks` sets `modal = None` and DELETES the now-impossible `pending_cancel_task.retain` line (app.rs:1078); `reset_session` collapses its five flag-resets to `self.modal = None`.
3. ui/mod.rs: one interception block replacing the five (Quit: q breaks/Esc cancels; Confirm: y/Y/Enter execute else cancel; CancelTasks/ClearTasks/Reset: y/Y/Enter act else dismiss; Help: any key closes). Ctrl-C uses `matches!(app.modal, Some(Modal::Quit))`. Mouse absorption preserves current behavior exactly — help does NOT absorb mouse, the other five do:
   `if app.filter_active || app.modal.as_ref().is_some_and(|m| !matches!(m, Modal::Help)) { continue; }`
4. `draw()`: single `match &app.modal` rendering the corresponding overlay; keep the separate `if app.filter_active` filter-bar check.

Accept: grep for the six old field names returns nothing; manual smoke (`?` help, `d` confirm + Esc, `R` reset, `q` quits).

## Step 4 — Consolidate confirm staging + explorer selection helpers (app.rs only)

**4a.** Keep the four public `request_*_confirm` wrappers; collapse bodies into:
- `fn annotate_confirm_target(&self, kind: &ConfirmKind, idx: usize, m: &Module) -> Option<ConfirmTarget>` — Apply fills `plan_age` from plan_cache; ForceUnlock fills lock id/who via `read_lock_info(...).or_else(|| self.detect_lock_from_tasks(idx))?` (None skips the module); Destroy/InitUpgrade plain.
- `fn stage_module_confirm(&mut self, kind: ConfirmKind)` — targets from `target_indices()` filtered through the annotator; empty → stage nothing.
- `request_apply_confirm` keeps its plan-task special case (`apply_targets_from_plan_tasks`, Focus::Tasks branch — Plan B deletes it later): if the Tasks-focused derivation returns Some, stage those (may be empty → return); else fall through to `stage_module_confirm(ConfirmKind::Apply)`. The existing test `stale_plan_task_apply_does_not_fallback_to_module_selection` must still pass.

**4b.** Dedupe the 4 copies of filtered-resource/target logic (open_resource_detail :1330, state_explorer_toggle_select :1406, enqueue_targeted_plan :1443, request_op_confirm :1500) with methods on `StateExplorer`: `resources()`, `filtered_indices()`, `filtered_count()`, `selected_real_index()`, `target_addresses()` (multi_select non-empty → unfiltered multi-select indices, IGNORING the filter — preserve this nuance; else highlighted filtered row). Rewrite call sites + `state_explorer_move`/`go_last`/`clamp_state_explorer_selection`/`refresh_state_explorer`; delete free fn `explorer_filtered_count` (:1848).

Accept: build+tests; `grep -n "to_lowercase().contains" src/app.rs` → only `visible_module_indices` + `filtered_indices`.

## Step 5 — Extract `TaskEngine` (the big one)

Create `src/engine.rs` (add `mod engine;` in main.rs). Move from App: fields `tasks`, `next_task_id`, `event_tx/event_rx`, `semaphore`, `plan_cache`, `running_modules`, `module_queues`; method bodies `push_task` (:660), `cancel_task` (:1158), event-handling core of `drain_events` (:1691).

```rust
pub struct TaskSpec {
    pub module_path: PathBuf,
    pub module_name: String,
    pub command: String,
    pub args: Vec<String>,
    pub plan_output_path: Option<PathBuf>,
    pub cleanup_plan_path: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy)]
pub enum EngineUpdate {
    Started { task_id: usize },
    Line { task_id: usize },
    Finished { task_id: usize, success: bool },
}

pub struct TaskEngine {
    pub tasks: Vec<Task>,
    pub plan_cache: PlanCache,
    // private: next_task_id, event_tx, event_rx, semaphore, binary, running_modules, module_queues
}

impl TaskEngine {
    pub fn new(binary: String, parallelism: usize) -> std::io::Result<Self>;
    pub fn push_task(&mut self, spec: TaskSpec) -> usize;          // returns task id
    pub fn drain_events(&mut self) -> Vec<EngineUpdate>;           // non-blocking (TUI)
    pub async fn next_update(&mut self) -> Option<EngineUpdate>;   // blocking (headless)
    fn apply_event(&mut self, event: TaskEvent) -> Option<EngineUpdate>;
    pub fn cancel_task(&mut self, task_id: usize);
    pub fn task(&self, id: usize) -> Option<&Task>;
    pub fn has_active_tasks(&self) -> bool;
}
```

Move notes:
- Internal `QueuedTask` drops its never-read `plan_output_path` field and `module_idx` (store name).
- `apply_event` Finished: keep exact order — stale-Cancelled skip returns `None` BEFORE any bookkeeping (including dequeue), exactly like today's `continue`; then set status, register plan in cache, remove cleanup file, dequeue/spawn next queued or remove from running_modules, return the update.
- App gets `pub engine: TaskEngine`; a private bridge `fn push_task_for(&mut self, module_idx, command, args, plan_output_path, cleanup_plan_path) -> usize` builds the TaskSpec from `self.modules[module_idx]`.
- New `App::drain_events`: bump spinner_tick, loop engine updates, call `check_op_completion(task_id, success)` on Finished.
- `start_op` (:1538): replace the fragile `let task_id = self.next_task_id;` pattern with the id returned by `push_task_for`.
- Fold in: drop `_first_new_idx` param from `maybe_auto_select_task` and its 6 call-site locals.
- Fix stale doc comment at app.rs:210 → `/// TUI application state: module selection, UI modes, and a TaskEngine.`
- Rename-only updates in ui/tree.rs (:31), ui/tasks.rs (3 sites), ui/mod.rs (CancelTasks name lookup via `app.engine.task(id)`), app.rs tests.

Guardrails: do NOT change queueing semantics (module busy synchronously; one queued slot per module; new enqueue replaces AND cancels the displaced queued task). Do NOT move `check_op_completion` into the engine. Do NOT touch runner.rs.

Verify: build+tests; TUI smoke: `i` on a module runs a task; `i` twice fast on one module → second shows Pending then runs after (queue works).

## Step 6 — Fix the `check_op_completion` unwrap cluster (app.rs)

Rewrite with zero `unwrap()` as two phases: Phase 1 under one scoped borrow (`let Some(explorer) … else return`; `let Some(pt) … else return`; `let Some((rid, addr)) = pt.running.take() else return`; wrong id → put back + return; push to `done`; compute `next_action = Option<(module_idx, kind, next_addr)>`). Phase 2: Some → build args from `kind.pre_args()` + addr, `push_task_for`, store `pt.running = Some((new_id, addr))`; None → move `pending_op.take()` into `op_result`, then `refresh_state_explorer()`.

Accept: unwrap count in app.rs drops by 6.

## Step 7 — Headless uses `TaskEngine` (main.rs:220-285)

Replace the parallel channel/semaphore implementation: build a `TaskEngine`, `push_task` per module, then `while engine.has_active_tasks() { next_update().await }`, printing `[{module_name}] {line}` on `Line` updates (read `output_lines.last()` from the task). Fail via `anyhow::bail!("{failed} task(s) failed")`.

Accepted trade-offs (comment them): termination via `has_active_tasks()` (sender stays alive so channel-close no longer signals done); lines now also buffered in `Task.output_lines`; failure exits through anyhow (`Error: …`, exit 1); per-module queue never engages headless (paths distinct) so semantics match.

Verify: `cargo run -- --dir fixtures/ list`; `cargo run -- --dir fixtures/ exec version --all` → interleaved output, exit 0.

## Step 8 — Non-blocking state reads (spawn_blocking + Loading state)

Decision: keep `state.rs` synchronous; run `read_state` via `tokio::task::spawn_blocking` + `oneshot`, polled each tick. Do NOT rewrite with tokio::process.

- `StateContent` gains `Loading` and `Error(String)`. `parse_state_from_str`: serde error → `Error(…)` not `NoState`; `parse_state_content`: read error → `Error`; `pull_remote_state`: non-zero exit / spawn failure → `Error` with stderr.
- `StateExplorer` gains `pub load_rx: Option<oneshot::Receiver<StateContent>>`. New `App::spawn_state_load()` (no-op if load in flight; clones path+binary; `spawn_blocking(move || { let _ = tx.send(read_state(&path, &binary)); })`; sets `content = Loading`). `open_state_explorer` and `refresh_state_explorer` both use it — refresh now shows the loading view instead of freezing (intended change).
- `App::poll_state_load()` called at the end of `drain_events`: `try_recv` Ok → install content, clear multi_select, clamp `selected` via `filtered_count()`; Closed → `Error("state load task failed")`.
- ui/mod.rs: `render_state_explorer` gains a `spinner: &str` param (pass `theme::SPINNER_FRAMES[(spinner_tick/2) as usize % 4]`); add `Loading` (dim `◐ Loading state…`) and `Error` (red title + dim msg) arms.

Guardrail: after this step, `grep -n "read_state" src/app.rs src/ui/` shows only the spawn_blocking closure.

## Step 9 — Config layering + kill scattered `process::exit`

- `Config::load(dir: &Path)`: try `<dir>/rug.toml` first, fall back to `./rug.toml`, then default; detect binary if unset. (Rationale: config belongs to the project `--dir` names; CWD fallback preserves current behavior.) main() canonicalizes root first, then loads; the `show_library_modules` CLI override stays after load.
- Replace `fn which` (subprocess) with a pure PATH scan `binary_on_path(bin) -> bool` (unix: file + any exec bit; non-unix: is_file), and split detection into testable `detect_binary_impl(env_override: Option<&str>, on_path: impl Fn(&str) -> bool) -> Result<String>` (non-empty TF_BINARY wins → tofu → terraform → bail).
- All `process::exit(1)` sites → `anyhow::bail!` bubbled to main: no-root-modules (main.rs:100,:106), no-matching-modules (:182), `confirm_headless` decline → `bail!("aborted by user")` (returns Result now).

Accept: `grep -rn "process::exit" src/` → nothing; `cargo run -- --dir fixtures/ plan --filter zzz; echo $?` → `Error: no matching root modules found`, `1`.

## Step 10 — Remove `#![allow(dead_code)]` blankets (app.rs:1, task.rs:1)

Delete both; handle every warning individually: genuinely unused → delete (grep first); test-only → fine; needed by Plan B → targeted `#[allow(dead_code)] // kept for Plan B: <reason>`. Keep the targeted allow on `LockInfo.operation`. If `EngineUpdate::Started` warns, destructure it in headless (`Started { .. } => {}`) rather than deleting.

Accept: no blanket allows; zero warnings.

## Step 11 — Tests (hermetic; tempfile already a dependency; don't mutate real env vars; don't depend on fixtures/ in unit tests)

- `discovery.rs::classify_dir` (5): no .tf → None; .tf + lock.hcl → Root; .tf + tfstate → Root; .tf + backend block → Root; .tf only → Library.
- `task.rs::parse_counts_from_line` (7): plan summary; import+forget; apply complete; destroy complete; "No changes."; ANSI-wrapped; ordinary line → None.
- `config.rs::detect_binary_impl` (4): env wins; empty env ignored; tofu preferred; nothing → err. Plus one `Config::load` layering test with a tempdir rug.toml (`binary = "from-dir"`).
- `lock.rs::parse_lock_from_output` (4): full block parses id/who; no block → None; missing ID → None; last block wins.
- `state.rs` (6): corrupt JSON → `Error` not `NoState`; empty → NoState; unindexed instance address; count `[0]`/`[1]`; for_each `["key"]`; module+data address forms.
- `engine.rs` queueing (1, `#[cfg(unix)] #[tokio::test]`): binary `echo`, parallelism 1, same module path ×3 → a runs, b queued, c replaces b (b Cancelled, c Pending); drain with 10s timeout → a and c Success.

Accept: `cargo test` ≥ 30 tests green.

## Plan A final verification

```bash
cargo build            # zero warnings
cargo test             # all green
cargo clippy           # clean (fix anything this refactor introduced)
cargo run -- --dir fixtures/ list
cargo run -- --dir fixtures/ plan --filter zzz-nomatch; echo $?   # Error + exit 1
cargo run -- --dir fixtures/ exec version --all                   # interleaved output, exit 0
cargo run -- --dir fixtures/   # TUI: ? help · d confirm/Esc · i twice on one module (queue) · Enter explorer (loading spinner, then content) · r refresh · q
```

## Likely failure modes (guardrails for the implementer)
1. Step 5 scope creep — only the listed 8 fields + 3 method bodies move; App keeps everything selection/UI/explorer related, including `check_op_completion` and `sorted_task_display`.
2. Step 3 mouse — help must NOT absorb mouse; the other five modals must.
3. Step 5 stale-Finished — skip must return before ANY bookkeeping including dequeue.
4. Step 2 restraint — if a style substitution isn't 1:1, leave the inline style alone.
5. Step 8 — no `read_state` on the UI thread; keep state.rs sync.
6. Never touch the do-not-touch list.

---

