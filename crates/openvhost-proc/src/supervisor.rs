// SPDX-License-Identifier: GPL-3.0-or-later
//! Registry + control surface + single broadcast event stream (spec §3).
//! Locks are short and never held across an await.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use tokio::sync::{broadcast, mpsc};

use crate::error::ProcError;
use crate::events::{LogLine, ServiceState, ServiceStatus, StreamSource, SupervisorEvent};
use crate::log::{RING_CAPACITY, RingBuffer, STDERR_TAIL, classify_level};
use crate::platform::{ProcessDriver, SpawnSpec};

#[derive(Debug, Clone)]
pub struct ServiceSpec {
    pub id: String,
    pub display_name: String,
    pub endpoint: Option<String>,
    pub spawn: SpawnSpec,
}

pub(crate) struct Entry {
    pub(crate) spec: ServiceSpec,
    pub(crate) state: ServiceState,
    pub(crate) pid: Option<u32>,
    pub(crate) logs: RingBuffer,
    pub(crate) stderr_tail: VecDeque<String>,
    pub(crate) stop_requested: Arc<AtomicBool>,
    pub(crate) control: Option<mpsc::Sender<()>>,
}

pub(crate) struct Inner {
    pub(crate) driver: Arc<dyn ProcessDriver>,
    pub(crate) entries: Mutex<HashMap<String, Entry>>,
    pub(crate) tx: broadcast::Sender<SupervisorEvent>,
}

#[derive(Clone)]
pub struct Supervisor {
    pub(crate) inner: Arc<Inner>,
}

pub(crate) fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

impl Supervisor {
    pub fn new(driver: Arc<dyn ProcessDriver>) -> Self {
        let (tx, _) = broadcast::channel(256);
        Self {
            inner: Arc::new(Inner {
                driver,
                entries: Mutex::new(HashMap::new()),
                tx,
            }),
        }
    }

    pub fn register(&self, spec: ServiceSpec) {
        let id = spec.id.clone();
        let entry = Entry {
            spec,
            state: ServiceState::Stopped,
            pid: None,
            logs: RingBuffer::new(RING_CAPACITY),
            stderr_tail: VecDeque::new(),
            stop_requested: Arc::new(AtomicBool::new(false)),
            control: None,
        };
        let mut entries = self.inner.entries.lock().unwrap_or_else(|e| e.into_inner());
        entries.insert(id, entry);
    }

    pub fn subscribe(&self) -> broadcast::Receiver<SupervisorEvent> {
        self.inner.tx.subscribe()
    }

    pub fn snapshot(&self) -> Vec<ServiceStatus> {
        let entries = self.inner.entries.lock().unwrap_or_else(|e| e.into_inner());
        let mut out: Vec<ServiceStatus> = entries
            .values()
            .map(|e| ServiceStatus {
                id: e.spec.id.clone(),
                display_name: e.spec.display_name.clone(),
                endpoint: e.spec.endpoint.clone(),
                pid: e.pid,
                state: e.state.clone(),
            })
            .collect();
        out.sort_by(|a, b| a.id.cmp(&b.id));
        out
    }

    pub fn log_tail(&self, id: &str, n: usize) -> Result<Vec<LogLine>, ProcError> {
        let entries = self.inner.entries.lock().unwrap_or_else(|e| e.into_inner());
        let e = entries
            .get(id)
            .ok_or_else(|| ProcError::NotFound(id.to_string()))?;
        Ok(e.logs.tail(n))
    }

    pub fn start(&self, id: &str) -> Result<(), ProcError> {
        let (spawn, stop_flag, control_rx) = {
            let mut entries = self.inner.entries.lock().unwrap_or_else(|e| e.into_inner());
            let e = entries
                .get_mut(id)
                .ok_or_else(|| ProcError::NotFound(id.to_string()))?;
            if matches!(e.state, ServiceState::Starting | ServiceState::Running) {
                return Ok(());
            }
            e.stop_requested.store(false, Ordering::SeqCst);
            let (ctl_tx, ctl_rx) = mpsc::channel(1);
            e.control = Some(ctl_tx);
            (e.spec.spawn.clone(), Arc::clone(&e.stop_requested), ctl_rx)
        };
        Inner::set_state(
            &self.inner,
            id,
            ServiceState::Starting,
            Some("requested by user".into()),
        );
        Inner::push_supervisor_log(
            &self.inner,
            id,
            "state Stopped → Starting (requested by user)".to_string(),
        );
        let inner = Arc::clone(&self.inner);
        let id_owned = id.to_string();
        tokio::spawn(crate::service_task::run(
            inner, id_owned, spawn, stop_flag, control_rx,
        ));
        Ok(())
    }

    pub fn stop(&self, id: &str) -> Result<(), ProcError> {
        let control = {
            let mut entries = self.inner.entries.lock().unwrap_or_else(|e| e.into_inner());
            let e = entries
                .get_mut(id)
                .ok_or_else(|| ProcError::NotFound(id.to_string()))?;
            if matches!(e.state, ServiceState::Stopped | ServiceState::Failed { .. }) {
                return Ok(());
            }
            // Flag FIRST — classification consults it (spec §4).
            e.stop_requested.store(true, Ordering::SeqCst);
            e.control.clone()
        };
        Inner::push_supervisor_log(&self.inner, id, "stop requested by user".to_string());
        if let Some(ctl) = control {
            let _ = ctl.try_send(());
        }
        Ok(())
    }
}

impl Inner {
    pub(crate) fn set_state(
        inner: &Arc<Inner>,
        id: &str,
        state: ServiceState,
        detail: Option<String>,
    ) {
        {
            let mut entries = inner.entries.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(e) = entries.get_mut(id) {
                e.state = state.clone();
                if matches!(state, ServiceState::Stopped | ServiceState::Failed { .. }) {
                    e.pid = None;
                    e.control = None;
                }
            }
        }
        let _ = inner.tx.send(SupervisorEvent::StateChanged {
            id: id.to_string(),
            state,
            detail,
        });
    }

    pub(crate) fn set_pid(inner: &Arc<Inner>, id: &str, pid: Option<u32>) {
        let mut entries = inner.entries.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(e) = entries.get_mut(id) {
            e.pid = pid;
        }
    }

    pub(crate) fn stderr_tail_snapshot(inner: &Arc<Inner>, id: &str) -> Vec<String> {
        let entries = inner.entries.lock().unwrap_or_else(|e| e.into_inner());
        entries
            .get(id)
            .map(|e| e.stderr_tail.iter().cloned().collect())
            .unwrap_or_default()
    }

    pub(crate) fn push_log(inner: &Arc<Inner>, id: &str, source: StreamSource, line: String) {
        let level = classify_level(source, &line);
        let ts_ms = now_ms();
        {
            let mut entries = inner.entries.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(e) = entries.get_mut(id) {
                e.logs.push(LogLine {
                    ts_ms,
                    level,
                    line: line.clone(),
                });
                if source == StreamSource::Stderr {
                    if e.stderr_tail.len() == STDERR_TAIL {
                        e.stderr_tail.pop_front();
                    }
                    e.stderr_tail.push_back(line.clone());
                }
            }
        }
        let _ = inner.tx.send(SupervisorEvent::Log {
            id: id.to_string(),
            ts_ms,
            level,
            line,
        });
    }

    pub(crate) fn push_supervisor_log(inner: &Arc<Inner>, id: &str, line: String) {
        Self::push_log(
            inner,
            id,
            StreamSource::Stdout,
            format!("supervisor: {line}"),
        );
    }
}
