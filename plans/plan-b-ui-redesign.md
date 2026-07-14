# PLAN B — Two-screen TUI redesign (execute with Opus 4.8)

## Context for the implementer

Replace rug's 3-pane layout ENTIRELY (no legacy fallback) with: **Select** screen (full-window module picker: `/` search, Space multi-select) → Enter → **Run** screen (status board top, live output bottom, all actions, Esc back while tasks keep running).

**Prerequisites (Plan A is done; verify, then adapt if names drifted):** `src/engine.rs` `TaskEngine` accessed as `app.engine` (owns tasks/plan_cache/push_task/drain/cancel; enqueues return created task ids); `app.modal: Option<Modal>` (Help/Confirm/CancelTasks/ClearTasks/Reset/Quit) with one interception point; `src/ui/theme.rs`; async state loads with `StateContent::Loading/Error`; `crate::util::strip_ansi`. Where this plan says `app.tasks`/`app.plan_cache`, use `app.engine.…`; where it says "set pending_confirm", use `Modal::Confirm`. Do not reintroduce flat booleans.

Verify environment first: `cargo build && cargo test`, `cargo run -- --dir fixtures/` (TUI), `cargo run -- --dir fixtures/ list`. **CLAUDE.md rule: README.md must be updated (step 12).** No new crate dependencies.

## 1. Route model

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen { Select, Run }

#[derive(Debug, Clone)]
pub struct SessionModule {
    pub module_idx: usize,   // raw index into app.modules (cache; re-resolved after refresh)
    pub path: PathBuf,       // source of truth
    pub name: String,
}

pub struct RunSession {
    pub modules: Vec<SessionModule>,       // display order = selection order at creation
    pub cursor: usize,                     // index into `modules`, NOT app.modules
    pub selected: Vec<usize>,              // board multi-select: indices into `modules`
    pub select_anchor: Option<usize>,
    pub latest_task: HashMap<PathBuf, usize>,  // latest task id started BY THIS SESSION per path
    pub fullscreen: bool,
    pub output_scroll: u16,                // 0 = tail-follow
    pub output_wrap: bool,
    pub created_at: Instant,
}
```

`App` gains `pub screen: Screen` (init Select) and `pub session: Option<RunSession>`. Invariant: `screen == Run` ⇒ `session.is_some()` — all transitions go through `enter_run`.

Fold-ins: fullscreen output → `RunSession.fullscreen` (delete `App.output_fullscreen/output_scroll/output_wrap`). State explorer stays `Option<StateExplorer>`, full-window overlay openable from BOTH screens via `s`; closing returns to whichever screen. Modals unchanged, rendered over either screen. **`Focus` is deleted entirely.**

Event dispatch order (first match wins): (1) Ctrl-C as today; (2) min-size guard (<40×10) swallows all but q/Ctrl-C; (3) modal interception (unchanged); (4) state explorer handler (move existing block verbatim into `handle_explorer_key`); (5) screen dispatch → `select::handle_key` (checks `filter_active` internally) / `run::handle_key` (checks `session.fullscreen` internally).

Draw order: too-small guard → explorer full-window (+op overlays, modal on top) → screen render (Run: fullscreen variant if set) → modal overlay.

Mouse: keep wheel scroll + click-to-highlight-row only. Select: wheel ±1 cursor, click sets cursor. Run: wheel over board ±1 cursor, over output ±3 scroll; click sets cursor. Fullscreen: keep the existing capture-off-on-entry/on-exit behavior (text selection); re-enable on Esc AND in run_tui teardown. Delete divider drag + click-to-focus.

## 2. Select screen — new `src/ui/select.rs` (replaces tree.rs; reuse `wrap::wrap_line`)

Layout: header line 1 (`rug` accent bold + root dim; right-aligned session indicator); header line 2 (`{binary} · {n} modules · depth/filter tags`, dim); spacer (dropped <20 rows); module list (borderless, 2-col left pad); status line (`●{k} selected` yellow, else dim hint); keybar (1 line). When `filter_active`, keybar row becomes inline filter input `/{filter}▌  enter keep · esc clear` (delete the floating filter popup).

Row: `▍`(cursor bar, accent) + `●`(multi, yellow) + name + running indicator (spinner ◐◓◑◒ + command, yellow — via new `App::module_activity(&Path) -> Option<(frame, command)>`) + `P:{age}` (green bold) + last-plan counts (via new `App::ready_plan_counts(&Path)` = plan_cache entry → task by id → resource_counts; render with existing `count_spans`, moved to shared `src/ui/widgets.rs`).

Width tiers: ≥100 all columns; 80–99 drop command word; 60–79 drop age; <60 name+spinner only, single-line truncate.

**Selection model — reuse, do not rewrite** (the #1 bug source): `selected_module` indexes the FILTERED list; `multi_select` holds RAW module indices; `multi_select_anchor` is a visible position. Reuse `visible_module_indices`, `toggle_multi_select`, `range_select`, `target_indices`, `move_module_selection`, depth fns, `refresh_modules`. Add `App::toggle_select_all_visible()` (`*` key).

Select keys: j/k/↑/↓, PgUp/PgDn, g/G, Space toggle, Ctrl+Space range, `*` all/none, `c` clear (+depth reset), `/` filter (Enter keep, Esc clear), Esc clear applied filter, `[`/`]` depth, **Enter → Run** (via target_indices; no-op if empty), **Tab → jump to existing session** (no-op if none), `i u p a d U` = shortcut: enter Run + trigger that action all-scope (confirms render over Run), `s` state explorer (highlighted), `r` refresh (+session remap), `R` reset (also drops session), `?` help, `q`/Ctrl-C quit. Dropped: `h` help alias, Tab cycling, `x`/`w` (move to Run). **Enter no longer opens the state explorer** (README callout).

Session indicator (right of header): running `⟳ session · {n} modules · {r} running  ⇥` (yellow); done `✓ session · {n} modules · done  ⇥` (dim green); <80 cols `⟳ {r}/{n} ⇥`.

Enter transition:
```
enter_run(app):
  targets = target_indices();  empty → return
  if session exists AND set(session paths) == set(target paths) → screen = Run  (RESUME, keep board state)
  else session = RunSession::new(targets in visible display order); screen = Run
```
Fresh session never cancels old tasks — they finish in the engine and stay in history.

## 3. Run screen — new `src/ui/run.rs`

Layout: header (`Run · {n} modules · {r} running · ✓{s} ✗{f} · {mm:ss}` + right `esc back` dim); board column header (dim UNDERLINED; omitted <20 rows); board; `Borders::TOP` dim separator titled `{module} · {command} · {status}`; output pane; keybar.

Board row shows the module's **display task**: `session.latest_task[path]` → else most recent non-terminal task for that path in the engine (background from previous session; dim `·prev` tag at ≥110 cols) → else idle. Row: `▍` + `●` + name + command + status icon+word (idle `· —` dim, `○ queued`, `◐ running` yellow animated, `◐ cancel` magenta, `✓ done` green, `✗ failed` red bold, `⊘ cancelled` dim) + elapsed + counts (`count_spans`) + `P:{age}`.

Width tiers: ≥110 all; 80–109 icon only, drop P/prev; 60–79 mark/name/icon/elapsed; <60 mark/name/icon.

Board/output sizing (`avail` = height − 3 chrome rows): ≥30 rows: board = min(n+1, max(5, avail·2/5)); 20–29: min(n+1, max(4, avail/3)), output ≥6; 15–19: no col header, board ≤5; **<15: output pane dropped** — board gets the space, plus a single dim status line showing the last output line (strip_ansi, truncated) of the cursor module with hint `⏎ output`. Board scrolls via ListState offset.

Output pane: reuse `output.rs` `parse_ansi` (do not fork); source = cursor module's display task; auto-tail semantics as today; scroll resets to 0 when cursor changes module. Fullscreen (Enter): same keys as current fullscreen handler (Esc, j/k, PgUp/PgDn, g/G, w wrap); mouse capture off/on.

**Action scope rule (one rule for all lowercase keys): board multi-selected subset if non-empty, else ALL session modules. Shift variant = highlighted row only.** (Rationale: running on everything you brought to the Run screen must be one keystroke; Shift is the "just this one" escape hatch.)

| Key | Action |
|---|---|
| `i`/`I`, `p`/`P`, `a`/`A`, `d`/`D` | init / plan / apply / destroy — all-or-subset / highlighted; apply+destroy confirm; apply uses cached plan file per module (existing `enqueue_apply_for`) |
| `u` | init -upgrade (confirm), all-or-subset |
| `U` | force-unlock (confirm; auto-filters locked), all-or-subset |
| `C` | cancel non-terminal display tasks in scope (existing CancelTasks modal) |
| `x` | clear completed history (existing confirm; engine-wide) |
| j/k g/G | board cursor · Space/Ctrl+Space/`*`/`c` board selection · PgUp/PgDn output scroll · Enter fullscreen · `w` wrap · `s` explorer (cursor module) · Esc back (tasks keep running) · `?` help · q quit |

Not bound on Run: `/ [ ] r R Tab` (discovery/selection live on Select).

Plumbing: `App::run_scope_indices() -> Vec<usize>` (raw indices via SessionModule.module_idx), `run_highlight_index()`. Action entry points (`enqueue_plan`, `enqueue_command`, `request_*_confirm`) change to take explicit `targets: &[usize]` and return `Vec<usize>` of created task ids; the old internal `target_indices()` call moves to the Select shortcut callers. Record returned ids into `session.latest_task`. **Delete `apply_targets_from_plan_tasks`** + its Focus::Tasks branch + its test (`stale_plan_task_apply_does_not_fallback_to_module_selection`; keep `ready_plan_for_task_requires_current_cache_owner`) — plan→apply linkage survives via `enqueue_apply_for`'s plan-cache lookup. `open_state_explorer` takes explicit `module_idx`.

Refresh guardrail: `refresh_modules` shifts raw indices — after it, remap session by path lookup, drop missing, clamp cursor, rebuild `selected`; empty → `session = None`. `reset_session` additionally sets `session = None; screen = Select`.

## 4. Responsive rules — new `src/ui/layout.rs`

```rust
pub enum WidthTier  { W1, W2, W3, W4 }   // ≥110, 80–109, 60–79, <60
pub enum HeightTier { H1, H2, H3, H4 }   // ≥30, 20–29, 15–19, <15
pub struct Breakpoints { pub w: WidthTier, pub h: HeightTier }  // Breakpoints::of(Rect)
pub const MIN_W: u16 = 40;  pub const MIN_H: u16 = 10;
pub fn too_small(area: Rect) -> bool;
pub fn popup_rect(desired_w: u16, desired_h: u16, area: Rect) -> Rect;  // centered, clamped, ≥1 margin
```

Apply per the tier tables above; keybar always shown, truncates hints with `…`; help clamps (2 columns wide, 1 column narrow); ALL hand-rolled centered popup Rects (`render_confirm`, cancel/clear/reset/quit-wait, op confirm/progress/result, plan-queued notice) switch to `popup_rect`, target lists truncate with `… and {k} more`. Min-size guard: cleared centered `terminal too small / need ≥ 40×10 (now {w}×{h}) / resize, or q to quit`; no state mutated while up.

## 5. Styling — final theme.rs values (extends/supersedes Plan A's placeholder look)

ANSI-16 only. `ACCENT=Cyan, MUTED=DarkGray, MARK=Yellow, OK=Green, ERR=Red, WARN=Magenta`. `app_title()` accent bold; `title()` bold; `dim()`; `row_cursor()` DarkGray bg + bold; `col_header()` dim underlined; `key_hint(key,label)` → key bold accent + label muted; `status_style` as before (Failed adds BOLD). `CURSOR_BAR = "▍"`; `spinner(tick) = SPINNER_FRAMES[(tick/2) % 4]` — **Running is now animated too**, not just Cancelling. No borders on Select; Run has the single top separator; popups keep Borders::ALL dim with bold titles (destructive: red title). Keybar = one line, no bg, hints joined by two spaces. Glyphs: `● ✓ ✗ ⊘ ○ ◐◓◑◒ ⟳ ▍ ⇥` (⇥ may be replaced with `tab` text if it renders poorly — nothing else may change).

Mockups (target fidelity):

```
Select 120×30                                          Run 120×30
  rug  ~/…/fixtures · tofu · 9 modules   ⟳ session · 3 modules · 1 running ⇥
                                                         Run · 3 modules · 1 running · ✓1 ✗0 · 01:42      esc back
 ▍● infra/network/vpc      ◐ plan    P:2m  +4 ~1          MODULE                COMMAND  STATUS      TIME  CHANGES
  ● infra/network/peering            P:8m  =             ▍ infra/network/vpc    plan     ◐ running    12s
  ● platform/eks                                           infra/network/peer   plan     ✓ done        8s  +4 ~1 -0
    services/api                     P:1m  +2              platform/eks         —        · —
                                                         ───────── infra/network/vpc · plan · running ─────────
  ●3 selected                                             module.vpc.aws_subnet.private[1]: Refreshing state...
  j/k move  space select  * all  / filter  enter run …    p plan  a apply  P/A one  space subset  enter output …
```

## 6. Deletions (final step, complete)

From app.rs: `Focus` + field + `cycle_focus`; `DragHandle`/`dragging`/`h_split_col`/`v_split_row`/`effective_h_split`/`effective_v_split`; `PaneHeights` → new `ViewportHeights { list, output, board, explorer }` (page-size bookkeeping; keep explorer wired); `output_fullscreen/output_scroll/output_wrap`; `selected_task_id`, `task_multi_select`, `toggle_task_select`, `move_task_selection`, `set_selected_task`, `maybe_auto_select_task` (enqueues return ids now), `output_task/current_output/output_title`, `apply_targets_from_plan_tasks` (+test), Focus branches in `request_apply_confirm`, `sorted_task_display` (scope-based cancel doesn't need display order); rewrite `go_to_first/go_to_last` screen-aware.

From ui/: `tree.rs` (entire), `tasks.rs` (entire except `count_spans` → widgets.rs), mod.rs 3-pane draw body, `pane_for_click`, floating `render_filter_bar`, mouse drag/click-to-focus arms, Tab/focus key arms, old help table.

Then remove `#![allow(dead_code)]` remnants and let the compiler find orphans; grep must return nothing for `Focus`, `DragHandle`, `h_split`, `v_split`, `output_fullscreen`, `cycle_focus`, `pane_for_click`, `selected_task_id`.

## 7. Help + README

`Modal::Help` renders the Select table on Select, Run table on Run. README `## TUI key bindings` → flow description + two tables (Select / Run) + callouts: Enter no longer opens state explorer (now `s`); Tab now resumes the run session; pane dragging removed; apply still consumes cached plans (P:{age}); 40×10 minimum. Keep GIF references; add HTML comment `TODO: re-record docs/demo-filter.gif`.

## 8. Ordered steps (each ends with `cargo build` clean + runnable TUI)

1. **Scaffolding:** `layout.rs` (tiers/too_small/popup_rect), theme additions, `keybar.rs` (`render_keybar` with `…` truncation), min-size guard wired into draw+dispatch, convert existing popups to `popup_rect`. Accept: guard at <40×10 (`q` works); popups clamp at 45×12; old UI otherwise unchanged.
2. **Screen enum + Select screen as base; legacy panes temporarily = Run body.** `select::render`/`handle_key` complete except Enter/Tab/action-shortcuts just set `screen = Run`; filter handling moves inside; helpers `module_activity`/`ready_plan_counts`; `count_spans` → widgets.rs. Legacy handler gains temporary Esc → Select. Accept: full-window Select works (filter/multi-select/`*`); Enter → old 3-pane; Esc back; spinner + P:{age} appear on Select after running a plan from legacy screen.
3. **RunSession + target plumbing:** types, `enter_run`, `run_scope_indices`/`run_highlight_index`/`toggle_select_all_visible`; enqueue/request signatures take explicit targets + return ids (verify headless doesn't share these paths — grep `enqueue_` in main.rs); delete `apply_targets_from_plan_tasks` (+test); `open_state_explorer(module_idx)`; refresh/reset remap rules; Select header indicator. Accept: build+test; UI behaves as step 2 but session populates (indicator visible after Esc).
4. **Run screen rendering:** `run::render` (board+output+H4 collapse) + `render_fullscreen_output` + display-task resolution + a minimal `handle_key` stub (j/k, Esc, Enter fullscreen). Replaces legacy Run body. Accept: 3 idle rows; 80×24 → W2/H2; 100×13 → H4; fullscreen in/out.
5. **Run key handler (full):** scope rule + Shift variants, C/x, board selection keys, PgUp/PgDn, s, w, ids → `latest_task`; wire Select action shortcuts. Accept: `p` from Select plans 3 modules with animated board; Space+`i` inits one; `A` confirms highlighted only; `C`→y cancels (magenta → ⊘).
6. **Back-nav/resume/fresh-session:** indicator states, Tab resume, same-set-resume vs fresh, `·prev` tag. Accept: full scenario per verification §9.7.
7. **Explorer integration polish:** `s` both screens, op popups via popup_rect, returns to correct screen, PgUp/PgDn viewport.
8. **Responsiveness pass:** every tier-table cell on both screens; keybar truncation. Accept: 80×24, 60×18, 45×12, 40×10 — no panics (saturating math everywhere).
9. **Mouse:** wheel/click per §1; board/output hit-testing needs the split row stored during draw (`viewport.board`); delete drag arms. Accept: wheel behaviors; fullscreen capture off/on.
10. **Deletions** per §6. Accept: build/test/`cargo clippy --all-targets -- -D warnings` clean; greps empty.
11. **Styling polish:** sweep remaining `Color::` literals in ui/ into theme; animated Running spinner everywhere; mockup fidelity at 120×35 and 80×24. Accept: `grep -n "Color::" src/ui/select.rs src/ui/run.rs` → nothing direct.
12. **Help + README** per §7 — cross-check every table row by pressing the key.
13. **Final verification** (below).

## 9. Verification (Plan B)

Automated: `cargo build`, `cargo test`, `cargo clippy --all-targets -- -D warnings`, `cargo run -- --dir fixtures/ list`, `cargo run -- --dir fixtures/ plan --all` (headless untouched).

Manual against fixtures/ (local-state modules; `exec validate` is a safe workload):
1. Launch → Select, keybar visible. 2. `/a` Enter → filtered; `*`; Esc → filter cleared, selection retained (raw-index invariant). 3. Select 3, `i` → Run with 3 running rows, spinners animate, output follows cursor. 4. `p` → counts appear; Space row 2 + `a` → confirm lists ONLY it with P:{age}; `y` → applies from plan file. 5. `A` → 1 target. 6. `C` → confirm lists active, `y` → Cancelling → Cancelled. 7. Esc mid-run → indicator; Tab → back, still streaming; different selection + Enter → fresh board while old finishes; re-select old set → resumed terminal states; overlap shows `·prev`. 8. `s` from both screens → explorer → Esc returns correctly. 9. Fullscreen: text selection works (capture off), Esc restores wheel. 10. Sizes: 80×24, 60×16, 100×13 (H4), 38×9 (guard). No panics during live streaming at any size. 11. `x` clears, `R` resets to Select (session gone), `r` remaps session without panic. 12. `q` with tasks running → quit-wait; second `q` forces; terminal restored cleanly.

---

