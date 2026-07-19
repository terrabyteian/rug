mod app;
mod config;
mod discovery;
mod engine;
mod lock;
mod module;
mod plan_cache;
mod runner;
mod state;
mod task;
mod ui;
mod util;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::path::PathBuf;

use app::App;
use config::Config;

#[derive(Parser, Debug)]
#[command(
    name = "rug",
    version,
    about = "Terraform/tofu CLI multiplexer across a module tree",
    long_about = None,
)]
struct Cli {
    /// Root directory to discover modules from (default: current directory).
    #[arg(short, long, default_value = ".")]
    dir: PathBuf,

    /// Include library modules (those without backend/state signals).
    #[arg(long)]
    show_library: bool,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Run `init` on matching modules.
    Init(RunArgs),
    /// Run `plan` on matching modules.
    Plan(RunArgs),
    /// Run `apply -auto-approve` on matching modules.
    Apply(RunArgs),
    /// Run `destroy -auto-approve` on matching modules.
    Destroy(RunArgs),
    /// Run an arbitrary terraform subcommand on matching modules.
    Exec(ExecArgs),
    /// List discovered modules and exit.
    List,
}

#[derive(Parser, Debug)]
struct RunArgs {
    /// Run on all discovered modules (root modules).
    #[arg(long)]
    all: bool,

    /// Only run on modules whose path contains this substring.
    #[arg(long)]
    filter: Option<String>,

    /// Skip confirmation prompt (for apply and destroy).
    #[arg(long, short = 'y')]
    yes: bool,
}

#[derive(Parser, Debug)]
struct ExecArgs {
    /// The terraform subcommand to run.
    subcommand: String,

    /// Extra arguments passed to the subcommand.
    #[arg(trailing_var_arg = true)]
    extra: Vec<String>,

    #[arg(long)]
    all: bool,

    #[arg(long)]
    filter: Option<String>,
}

/// Raise the soft fd limit toward the hard limit. Each cached plan pins one
/// anonymous fd, and macOS defaults the soft limit to 256 — tight for a big
/// monorepo session plus the pipes of concurrently running tasks.
#[cfg(unix)]
fn raise_nofile_soft_limit() {
    unsafe {
        let mut lim = libc::rlimit {
            rlim_cur: 0,
            rlim_max: 0,
        };
        if libc::getrlimit(libc::RLIMIT_NOFILE, &mut lim) == 0 {
            let want = std::cmp::min(4096, lim.rlim_max);
            if lim.rlim_cur < want {
                lim.rlim_cur = want;
                libc::setrlimit(libc::RLIMIT_NOFILE, &lim);
            }
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    #[cfg(unix)]
    raise_nofile_soft_limit();

    let cli = Cli::parse();

    let root = cli.dir.canonicalize().unwrap_or(cli.dir.clone());

    let mut config = Config::load(&root).context("loading config")?;
    config.show_library_modules = cli.show_library;

    let all_modules = discovery::discover(&root, &config).context("discovering modules")?;

    match cli.command {
        None => {
            // TUI mode.
            if all_modules.is_empty() {
                anyhow::bail!("No terraform root modules found under {}", root.display());
            }
            let root_modules: Vec<_> = all_modules.into_iter().filter(|m| m.is_root()).collect();
            if root_modules.is_empty() {
                anyhow::bail!("No terraform root modules found under {}", root.display());
            }
            let mut app = App::new(config, root.clone(), root_modules);
            ui::run_tui(&mut app)?;
        }

        Some(Commands::List) => {
            match &config.source {
                Some(path) => println!("config: {}", path.display()),
                None => println!("config: (defaults)"),
            }
            if all_modules.is_empty() {
                println!("No modules found under {}", root.display());
            } else {
                for m in &all_modules {
                    let tag = if m.is_root() { "" } else { " (library)" };
                    println!("{}{}", m.display_name, tag);
                }
            }
        }

        Some(Commands::Init(args)) => {
            run_headless_cmd(&config, &all_modules, "init", &[], &args, false).await?;
        }
        Some(Commands::Plan(args)) => {
            run_headless_cmd(&config, &all_modules, "plan", &[], &args, false).await?;
        }
        Some(Commands::Apply(args)) => {
            run_headless_cmd(
                &config,
                &all_modules,
                "apply",
                &["-auto-approve".to_string()],
                &args,
                true,
            )
            .await?;
        }
        Some(Commands::Destroy(args)) => {
            run_headless_cmd(
                &config,
                &all_modules,
                "destroy",
                &["-auto-approve".to_string()],
                &args,
                true,
            )
            .await?;
        }
        Some(Commands::Exec(e)) => {
            let ra = RunArgs {
                all: e.all,
                filter: e.filter,
                yes: true,
            };
            run_headless_cmd(&config, &all_modules, &e.subcommand, &e.extra, &ra, false).await?;
        }
    }

    Ok(())
}

async fn run_headless_cmd(
    config: &Config,
    all_modules: &[module::Module],
    command: &str,
    extra_args: &[String],
    run_args: &RunArgs,
    needs_confirm: bool,
) -> Result<()> {
    let filter = run_args.filter.as_deref().unwrap_or("");
    let modules: Vec<&module::Module> = all_modules
        .iter()
        .filter(|m| m.is_root())
        .filter(|m| filter.is_empty() || m.display_name.contains(filter))
        .collect();

    if modules.is_empty() {
        anyhow::bail!("no matching root modules found");
    }

    if needs_confirm && !run_args.yes {
        confirm_headless(command, &modules)?;
    }

    run_headless(config, &modules, command, extra_args).await
}

/// Print a confirmation prompt to stderr and read a response from stdin.
/// Returns Ok(()) if the user confirmed, `Err` if they declined.
fn confirm_headless(command: &str, modules: &[&module::Module]) -> Result<()> {
    use std::io::{self, BufRead, Write};

    let cmd = command.to_uppercase();
    let n = modules.len();
    let noun = if n == 1 { "module" } else { "modules" };

    eprintln!("\nAbout to run {cmd} on {n} {noun}:");
    for m in modules {
        eprintln!("  • {}", m.display_name);
    }
    eprint!("\nContinue? [y/N] ");
    io::stderr().flush().ok();

    let mut line = String::new();
    io::stdin().lock().read_line(&mut line)?;
    let answer = line.trim().to_lowercase();

    if answer != "y" && answer != "yes" {
        anyhow::bail!("aborted by user");
    }

    Ok(())
}

/// Run `command` on every module in `modules`, streaming output to stdout,
/// via the same `TaskEngine` the TUI uses.
///
/// Accepted trade-offs versus the old hand-rolled channel/semaphore loop:
/// - Termination is driven by `engine.has_active_tasks()` rather than the
///   event channel closing — the engine holds its own sender for the whole
///   run, so the channel never closes on its own.
/// - Output lines are now also buffered in `Task.output_lines` (previously
///   headless mode only ever printed them and threw them away); harmless,
///   just a bit more memory retained for very chatty commands.
/// - Failures now propagate through `anyhow::bail!` instead of an immediate
///   forced-exit call, so a failure prints as `Error: ...` and the process
///   exits 1 via `main`'s `Result` return path.
/// - `TaskEngine`'s per-module run/queue bookkeeping never actually engages
///   here, because every module in `modules` has a distinct path, so at most
///   one task per module is ever pushed — semantics match the old
///   implementation, which had no queueing at all.
async fn run_headless(
    config: &Config,
    modules: &[&module::Module],
    command: &str,
    extra_args: &[String],
) -> Result<()> {
    use engine::{EngineUpdate, TaskEngine, TaskSpec};
    use task::TaskStatus;

    let mut engine = TaskEngine::new(config.binary.clone(), config.parallelism);

    let task_ids: Vec<usize> = modules
        .iter()
        .map(|module| {
            engine.push_task(TaskSpec {
                module_path: module.path.clone(),
                module_name: module.display_name.clone(),
                command: command.to_string(),
                args: extra_args.to_vec(),
                plan_output: None,
                targets: Vec::new(),
                apply_plan: None,
            })
        })
        .collect();

    while engine.has_active_tasks() {
        match engine.next_update().await {
            Some(EngineUpdate::Line { task_id }) => {
                if let Some(task) = engine.task(task_id) {
                    if let Some(line) = task.output_lines.last() {
                        println!("[{}] {}", task.module_name, line);
                    }
                }
            }
            Some(EngineUpdate::Started { .. }) | Some(EngineUpdate::Finished { .. }) => {}
            // Channel closed unexpectedly (shouldn't happen — the engine
            // holds its own sender for as long as it's alive).
            None => break,
        }
    }

    let failed = task_ids
        .iter()
        .filter(|&&id| {
            engine
                .task(id)
                .map(|t| t.status == TaskStatus::Failed)
                .unwrap_or(false)
        })
        .count();

    if failed > 0 {
        anyhow::bail!("{failed} task(s) failed");
    }
    Ok(())
}
