use std::process::Stdio;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::Semaphore;

use crate::task::{CancelHandle, TaskEvent, TaskEventSender};

/// Spawn a task in a background tokio task, emitting events to `tx`.
///
/// Returns a `CancelHandle` with two escalation levels:
///   1. `cancel()`     → sends SIGINT (same as Ctrl+C); waits indefinitely for
///                        the process to exit gracefully so tofu can release
///                        locks and print its interrupt message.
///   2. `force_kill()` → sends SIGKILL immediately if the process is still
///                        running after a graceful-cancel request.
pub fn spawn_task(
    task_id: usize,
    module_path: std::path::PathBuf,
    _module_name: String,
    binary: String,
    command: String,
    args: Vec<String>,
    tx: TaskEventSender,
    semaphore: Arc<Semaphore>,
) -> CancelHandle {
    let (sigint_tx,  mut sigint_rx)  = tokio::sync::oneshot::channel::<()>();
    let (sigkill_tx, mut sigkill_rx) = tokio::sync::oneshot::channel::<()>();

    tokio::spawn(async move {
        // Acquire semaphore slot; allow both cancel signals to abort the wait.
        let _permit = tokio::select! {
            result = semaphore.acquire_owned() => match result {
                Ok(p) => p,
                Err(_) => return,
            },
            _ = &mut sigint_rx  => {
                let _ = tx.send(TaskEvent::Finished { task_id, success: false });
                return;
            },
            _ = &mut sigkill_rx => {
                let _ = tx.send(TaskEvent::Finished { task_id, success: false });
                return;
            },
        };

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
                let _ = tx.send(TaskEvent::Line {
                    task_id,
                    line: format!("error: failed to spawn: {e}"),
                });
                let _ = tx.send(TaskEvent::Finished { task_id, success: false });
                return;
            }
        };

        let pid = child.id();

        let stdout = child.stdout.take().unwrap();
        let stderr = child.stderr.take().unwrap();

        let tx_out = tx.clone();
        let tx_err = tx.clone();

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

        let success = tokio::select! {
            // Natural completion.
            status = child.wait() => {
                out_task.await.ok();
                err_task.await.ok();
                status.map(|s| s.success()).unwrap_or(false)
            },

            // Graceful cancel: send SIGINT then wait indefinitely.
            // A second escalation (force_kill) can arrive via sigkill_rx.
            _ = &mut sigint_rx => {
                if let Some(pid) = pid {
                    #[cfg(unix)]
                    unsafe { libc::kill(pid as i32, libc::SIGINT); }
                }
                // Wait for graceful exit OR an explicit force-kill.
                tokio::select! {
                    _ = child.wait() => {},
                    _ = sigkill_rx => {
                        child.kill().await.ok();
                        child.wait().await.ok();
                    },
                }
                out_task.await.ok();
                err_task.await.ok();
                false
            },

            // Direct force-kill (e.g. second cancel before first completes,
            // or sigkill sent without a prior sigint).
            _ = &mut sigkill_rx => {
                child.kill().await.ok();
                child.wait().await.ok();
                out_task.await.ok();
                err_task.await.ok();
                false
            },
        };

        let _ = tx.send(TaskEvent::Finished { task_id, success });
    });

    CancelHandle::new(sigint_tx, sigkill_tx)
}
