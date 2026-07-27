// SPDX-License-Identifier: GPL-3.0-or-later
//! One-shot task runner: run a command to completion, streaming its output.
//!
//! Deliberately NOT a `Supervisor` entry. A supervised service has a state
//! machine where `Stopped` means something went away; a task that exits 0 has
//! simply finished, and rendering that as `Stopped`/`Failed` in the Services
//! panel would be worse than useless.
//!
//! What it does borrow is the containment: the child gets its own process
//! group (via the same `ProcessDriver` the supervisor uses) and the group is
//! killed if this future is dropped. `brew install` forks a tree — curl, tar,
//! ruby, sometimes a compiler — and abandoning that tree when the app quits
//! mid-install is exactly the orphan problem P0-8 closed.
//!
//! There is deliberately NO timeout: a twenty-minute compile is normal, so a
//! clock could only ever fire on legitimate work. Cancellation is expressed by
//! dropping the future (e.g. aborting the task it was spawned on).

use std::sync::Arc;

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::mpsc::Sender;

use crate::error::ProcError;
use crate::platform::{OutputStream, ProcessDriver, SpawnSpec, SpawnedChild};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stream {
    Stdout,
    Stderr,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskEvent {
    Line { stream: Stream, text: String },
    Finished { code: Option<i32> },
}

/// Kills the whole process group if the run is abandoned (dropped) before it
/// finishes. `Drop` cannot be async, which is why `ProcessDriver::kill` is a
/// synchronous call — best-effort, errors are swallowed the same way the
/// supervisor's own teardown paths treat "already gone" as fine.
struct KillOnDrop {
    driver: Arc<dyn ProcessDriver>,
    child: SpawnedChild,
    finished: bool,
}

impl Drop for KillOnDrop {
    fn drop(&mut self) {
        if !self.finished {
            let _ = self.driver.kill(&mut self.child);
        }
    }
}

/// Reads `stream` line-by-line and forwards each as a `TaskEvent::Line`
/// tagged `which`, until EOF or the receiver goes away. A private fn (rather
/// than a closure spawned twice) keeps the `Send` bound on the future
/// unambiguous for `tokio::spawn`.
async fn pump_stream(stream: Option<OutputStream>, which: Stream, tx: Sender<TaskEvent>) {
    let Some(s) = stream else { return };
    let mut lines = BufReader::new(s).lines();
    while let Ok(Some(text)) = lines.next_line().await {
        if tx
            .send(TaskEvent::Line {
                stream: which,
                text,
            })
            .await
            .is_err()
        {
            return; // receiver gone: stop reading, let the guard clean up
        }
    }
}

/// Runs `spec` to completion, sending every output line then a final
/// `Finished`. `Ok(None)` means the process was killed by a signal.
///
/// `ProcError` is only for failing to *run* the program (missing binary,
/// spawn refused). A program that runs and exits non-zero is `Ok(Some(code))`
/// — "brew said no" is an outcome to render, not a runner error.
pub async fn run_task(
    driver: Arc<dyn ProcessDriver>,
    spec: SpawnSpec,
    tx: Sender<TaskEvent>,
) -> Result<Option<i32>, ProcError> {
    let mut child = driver.spawn(&spec)?;

    let stdout = child.take_stdout();
    let stderr = child.take_stderr();

    let mut guard = KillOnDrop {
        driver,
        child,
        finished: false,
    };

    let out = tokio::spawn(pump_stream(stdout, Stream::Stdout, tx.clone()));
    let err = tokio::spawn(pump_stream(stderr, Stream::Stderr, tx.clone()));

    let status = guard.child.wait().await?;
    guard.finished = true;

    // Await the readers AFTER wait() returns, so lines still buffered in the
    // pipes are delivered before Finished is sent — otherwise a fast exit can
    // race the readers and drop trailing output on the floor.
    let _ = out.await;
    let _ = err.await;

    let code = status.code();
    let _ = tx.send(TaskEvent::Finished { code }).await;
    Ok(code)
}
