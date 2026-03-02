use std::process::Stdio;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::Semaphore;

use crate::task::{TaskEvent, TaskEventSender};

/// Spawn a task in a background tokio task, emitting events to `tx`.
/// Per-module sequencing is handled at the app level; by the time this is
/// called the task is ready to compete for a semaphore slot and run.
///
/// Returns an `AbortHandle` that can be used to cancel the task at any point
/// (semaphore-waiting, process-running, or I/O-draining).
pub fn spawn_task(
    task_id: usize,
    module_path: std::path::PathBuf,
    _module_name: String,
    binary: String,
    command: String,
    args: Vec<String>,
    tx: TaskEventSender,
    semaphore: Arc<Semaphore>,
) -> tokio::task::AbortHandle {
    tokio::spawn(async move {
        let _permit = semaphore.acquire_owned().await.unwrap();

        let tx_line = tx.clone();
        let tx_fin = tx.clone();

        let mut cmd = Command::new(&binary);
        cmd.arg(&command)
            .args(&args)
            .current_dir(&module_path)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        let _ = tx.send(TaskEvent::Started { task_id });

        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                let _ = tx_fin.send(TaskEvent::Line {
                    task_id,
                    line: format!("error: failed to spawn: {e}"),
                });
                let _ = tx_fin.send(TaskEvent::Finished { task_id, success: false });
                return;
            }
        };

        let stdout = child.stdout.take().unwrap();
        let stderr = child.stderr.take().unwrap();

        let tx_out = tx_line.clone();
        let tx_err = tx_line.clone();

        let out_task = tokio::spawn(async move {
            let mut reader = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                let _ = tx_out.send(TaskEvent::Line { task_id, line });
            }
        });

        let err_task = tokio::spawn(async move {
            let mut reader = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                let _ = tx_err.send(TaskEvent::Line { task_id, line });
            }
        });

        let status = child.wait().await;
        out_task.await.ok();
        err_task.await.ok();

        let success = status.map(|s| s.success()).unwrap_or(false);
        let _ = tx_fin.send(TaskEvent::Finished { task_id, success });
    }).abort_handle()
}
