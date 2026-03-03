# rug — terraform/tofu multiplexer

Run terraform/tofu commands across a directory tree of modules. Interactive TUI by default; headless CLI for scripting.

## Install

```sh
curl -fsSL https://raw.githubusercontent.com/terrabyteian/rug/master/install.sh | sh
```

Pin a specific version:

```sh
curl -fsSL https://raw.githubusercontent.com/terrabyteian/rug/master/install.sh | RUG_VERSION=v0.3.0 sh
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

## TUI key bindings

| Key | Action |
|---|---|
| `j` / `k` / `↑` / `↓` | Navigate lists or scroll output |
| `PgUp` / `PgDn` | Page up / page down |
| `g` / `G` | Jump to first / last |
| `Space` | Toggle multi-select (Modules or Tasks pane) |
| `Ctrl+Space` | Range-select modules |
| `c` | Clear selection (current pane) |
| `Enter` | State explorer (Modules pane) / Fullscreen (Output pane) |
| `Esc` | Close overlay / clear filter |
| `i` | Init selected modules |
| `u` | Init -upgrade selected modules |
| `p` | Plan selected modules |
| `a` | Apply selected modules |
| `d` | Destroy selected modules |
| `U` | Force-unlock state (if locked) |
| `C` | Cancel selected task |
| `/` | Filter modules by name |
| `[` / `]` | Decrease / increase depth |
| `r` | Refresh module list |
| `Tab` | Cycle focus between panes |
| `h` / `?` | Toggle help |
| `q` / `Ctrl-C` | Quit |

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
