mod app;
mod config;
mod discovery;
mod lock;
mod module;
mod plan_cache;
mod runner;
mod state;
mod task;
mod ui;

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

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    let mut config = Config::load().context("loading config")?;
    config.show_library_modules = cli.show_library;

    let root = cli.dir.canonicalize().unwrap_or(cli.dir.clone());
    let all_modules = discovery::discover(&root, &config).context("discovering modules")?;

    match cli.command {
        None => {
            // TUI mode.
            if all_modules.is_empty() {
                eprintln!(
                    "No terraform root modules found under {}",
                    root.display()
                );
                std::process::exit(1);
            }
            let root_modules: Vec<_> = all_modules.into_iter().filter(|m| m.is_root()).collect();
            if root_modules.is_empty() {
                eprintln!("No terraform root modules found under {}", root.display());
                std::process::exit(1);
            }
            let mut app = App::new(config, root.clone(), root_modules);
            ui::run_tui(&mut app)?;
        }

        Some(Commands::List) => {
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
            let ra = RunArgs { all: e.all, filter: e.filter, yes: true };
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
        eprintln!("No matching root modules found.");
        std::process::exit(1);
    }

    if needs_confirm && !run_args.yes {
        confirm_headless(command, &modules)?;
    }

    run_headless(config, &modules, command, extra_args).await
}

/// Print a confirmation prompt to stderr and read a response from stdin.
/// Returns Ok(()) if the user confirmed, exits the process if they declined.
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
        eprintln!("Aborted.");
        std::process::exit(1);
    }

    Ok(())
}

async fn run_headless(
    config: &Config,
    modules: &[&module::Module],
    command: &str,
    extra_args: &[String],
) -> Result<()> {
    use std::sync::Arc;
    use task::{TaskEvent, TaskStatus};
    use tokio::sync::{mpsc, Semaphore};

    let (tx, mut rx) = mpsc::unbounded_channel::<TaskEvent>();
    let semaphore = Arc::new(Semaphore::new(config.parallelism));

    let task_names: Vec<String> = modules.iter().map(|m| m.display_name.clone()).collect();

    // Keep handles alive for the duration of the run — dropping them early would
    // close the oneshot senders and immediately fire the cancel branch in the runner.
    let _handles: Vec<_> = modules
        .iter()
        .enumerate()
        .map(|(i, module)| {
            runner::spawn_task(
                i,
                module.path.clone(),
                module.display_name.clone(),
                config.binary.clone(),
                command.to_string(),
                extra_args.to_vec(),
                tx.clone(),
                semaphore.clone(),
            )
        })
        .collect();

    let mut statuses: Vec<TaskStatus> =
        (0..modules.len()).map(|_| TaskStatus::Pending).collect();
    let mut done_count = 0;

    // Drop original tx so the channel closes when all spawned tasks finish.
    drop(tx);

    while let Some(event) = rx.recv().await {
        match event {
            TaskEvent::Started { .. } => {}
            TaskEvent::Line { task_id, line } => {
                let name = &task_names[task_id];
                println!("[{name}] {line}");
            }
            TaskEvent::Finished { task_id, success } => {
                statuses[task_id] =
                    if success { TaskStatus::Success } else { TaskStatus::Failed };
                done_count += 1;
                if done_count == modules.len() {
                    break;
                }
            }
        }
    }

    if statuses.iter().any(|s| *s == TaskStatus::Failed) {
        std::process::exit(1);
    }
    Ok(())
}
