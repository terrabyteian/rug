# rug — terraform/tofu multiplexer

Run terraform/tofu commands across a directory tree of modules. Interactive TUI by default; headless CLI for scripting.

![TUI demo](docs/demo-tui.gif)

## Install

```sh
curl -fsSL https://raw.githubusercontent.com/terrabyteian/rug/master/install.sh | sh
```

Pin a specific version:

```sh
curl -fsSL https://raw.githubusercontent.com/terrabyteian/rug/master/install.sh | RUG_VERSION=v0.6.0 sh
```

**Build from source** (requires Rust):

```sh
cargo install --path .
```

## What it does

`rug` discovers terraform/tofu root modules under a directory tree and lets you run operations on them in parallel. In TUI mode you navigate modules, multi-select, and watch live output side by side. In headless mode you pipe it into CI scripts with `--all` or narrow the scope with `--filter`.

## Usage

### TUI

```sh
rug                    # discover modules under current directory
rug --dir infra/       # start from a specific root
rug --show-library     # also show library modules (no backend/state signals)
```

### Headless subcommands

`--dir` must come **before** the subcommand.

| Subcommand | What it runs | Notes |
|---|---|---|
| `init` | `terraform init` | |
| `plan` | `terraform plan` | |
| `apply` | `terraform apply -auto-approve` | prompts for confirmation unless `-y` |
| `destroy` | `terraform destroy -auto-approve` | prompts for confirmation unless `-y` |
| `exec <cmd> [args...]` | arbitrary subcommand | |
| `list` | prints discovered modules and exits | |

**Common flags** (init/plan/apply/destroy/exec):

| Flag | Description |
|---|---|
| `--all` | Run on all discovered root modules |
| `--filter <string>` | Only run on modules whose path contains this substring |
| `-y` / `--yes` | Skip confirmation prompt (apply and destroy) |

**Examples:**

```sh
rug --dir infra/ plan --all
rug --dir infra/ apply --filter vpc -y
rug --dir infra/ exec validate --all
rug --dir infra/ list
```

![Headless plan demo](docs/demo-headless.gif)

## TUI key bindings

The TUI has two screens. The **Select** screen is a full-window module picker: use
`/` to filter by name and `Space` to multi-select. Press `Enter` (or any action key)
to move to the **Run** screen — a status board of the modules you brought over, with a
live output pane for the highlighted module. Actions on the Run screen apply to the
whole session (or a board subset you mark with `Space`); the `Shift` variant targets
just the highlighted row. Press `Esc` to return to Select while tasks keep running,
and `Tab` from Select to jump back into the running session.

Modules with a successful cached plan ready to apply are marked `P:{age}`; a later
apply consumes that cached plan file automatically.

![Filter and select demo](docs/demo-filter.gif)

**Select screen**

| Key | Action |
|---|---|
| `j` / `k` / `↑` / `↓` | Move cursor |
| `PgUp` / `PgDn` | Page up / down |
| `g` / `G` | Jump to first / last |
| `Space` | Toggle multi-select |
| `Ctrl+Space` | Range-select |
| `*` / `c` | Select all visible / clear selection |
| `/` | Filter modules by name (`Enter` keep, `Esc` clear) |
| `Esc` | Clear the applied filter |
| `[` / `]` | Decrease / increase depth |
| `Enter` | Run the current selection |
| `Tab` | Resume the existing run session |
| `i` / `u` | Init / init `-upgrade` |
| `p` / `a` | Plan / apply |
| `d` / `U` | Destroy / force-unlock |
| `s` | State explorer for the highlighted module |
| `r` / `R` | Refresh modules / reset session |
| `?` | Help |
| `q` / `Ctrl-C` | Quit |

**Run screen**

| Key | Action |
|---|---|
| `j` / `k` / `↑` / `↓` | Move board cursor |
| `g` / `G` | Jump to first / last |
| `PgUp` / `PgDn` | Scroll output pane |
| `Space` | Toggle row in the board subset |
| `Ctrl+Space` | Range-select rows |
| `*` / `c` | Select all rows / clear subset |
| `i` / `p` / `a` / `d` | Init / plan / apply / destroy (subset, or all if none marked) |
| `I` / `P` / `A` / `D` | Same, highlighted row only |
| `u` / `U` | Init `-upgrade` / force-unlock |
| `C` | Cancel active tasks in scope |
| `x` | Clear completed task history |
| `Enter` | Fullscreen output |
| `w` | Toggle output wrap |
| `s` | State explorer for the highlighted module |
| `Esc` | Back to Select (tasks keep running) |
| `?` | Help |
| `q` / `Ctrl-C` | Quit |

**State explorer**

Press `s` on either screen to browse the highlighted module's state. Resources are
grouped by child module: a header row like `▸ module.net (3)` sits above its indented
member resources. Pressing `Space` on a header selects the whole module as a single
`-target=module.net`; pressing it on a resource selects that resource individually.
Targeted operations act on the current selection (or the highlighted row if nothing
is selected).

| Key | Action |
|---|---|
| `j` / `k` / `↑` / `↓` | Move cursor |
| `Enter` | Inspect resource attributes (no-op on header rows) |
| `Space` | Select resource — or the whole module on a header row |
| `c` | Clear selection |
| `/` | Filter resources by address |
| `p` | Targeted plan (`plan -target=…`) |
| `a` | Targeted apply (`apply -target=…`, with confirmation) |
| `d` | Targeted destroy (`destroy -target=…`, with confirmation) |
| `t` | Taint (module selections expand to member resources; data sources skipped) |
| `D` | Remove from state (`state rm`, accepts module addresses) |
| `r` | Refresh state |
| `Esc` / `q` | Close |

Every operation launched from the state explorer — targeted plan, apply, destroy,
taint, and state rm — appears on the Run screen task board. If you have no active
Run session a fresh one is created automatically (containing the module you acted
on); if a session already exists, the module is added to it. You stay in the state
explorer either way, and reach the board with `Tab` or `Enter` as usual. A targeted
task shows a `·T{n}` count next to its command (e.g. `apply·T2`) while it runs and in
its finished result row, where `n` is the number of `-target=` addresses.

Apply and destroy prompt for confirmation. `apply` consumes a cached plan (`P:{age}`)
per module when one is available. A **targeted** plan (made with `p` in the state
explorer) marks the module's plan badge with a `T{n}` suffix — `P:{age}·T{n}` — where
`n` is the number of `-target=` addresses. (This plan-cache badge is distinct from the
per-task `·T{n}` count above.) Applying a targeted cached plan warns you
in the confirm dialog and lists exactly which addresses the apply covers. Targeted
apply and destroy from the state explorer run `apply`/`destroy -target=…` directly
and never consume the cached plan. The state explorer opens with `s` on either
screen — `Enter` no longer opens it. Pane dragging has been removed. The minimum
usable terminal size is 40×10; below that the TUI shows a resize prompt.

## Configuration

`rug.toml` in the working directory (all fields optional):

```toml
# Path to the terraform/tofu binary.
# Overridden by TF_BINARY env var; auto-detected if omitted.
binary = "tofu"

# Maximum number of concurrent terraform processes (default: 4).
parallelism = 4

# Directories to skip during module discovery.
ignore_dirs = [".terraform", ".git", "node_modules", ".terragrunt-cache"]

# Show library modules (no backend/lock signals) in the TUI (default: false).
show_library_modules = false
```

**Binary detection priority:**

1. `TF_BINARY` environment variable
2. `binary` field in `rug.toml`
3. `tofu` on PATH
4. `terraform` on PATH

## Supported platforms

| OS | Architecture |
|---|---|
| macOS | arm64 |
| Linux | x86_64 |
| Linux | arm64 |

Intel Macs can run the arm64 binary via Rosetta 2.
