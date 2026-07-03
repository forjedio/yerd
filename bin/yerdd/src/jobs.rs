//! Long-running job registry.
//!
//! Some IPC operations (scaffolding a new site via `laravel new`) take far
//! longer than a single request/response round-trip and stream output as they
//! go. The IPC protocol is one-shot, so those operations run as a background
//! **job**: [`Request::CreateSite`] starts one and returns a [`JobId`]
//! immediately; the client polls [`Request::JobStatus`] for the streamed log +
//! phase until the job reaches a terminal [`JobState`].
//!
//! [`Request::CreateSite`]: yerd_ipc::Request::CreateSite
//! [`Request::JobStatus`]: yerd_ipc::Request::JobStatus

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

use tokio::sync::{watch, Mutex};

use yerd_ipc::{ErrorCode, JobId, JobState, Response};

/// Cap on retained log lines per job. Older lines are evicted (the client is
/// told via the cursor, so it never silently loses position).
const LOG_CAP: usize = 5_000;

/// Cap on retained *terminal* jobs, so a long-lived daemon doesn't accumulate
/// finished-job state without bound.
const TERMINAL_CAP: usize = 64;

/// One tracked job.
struct Job {
    state: JobState,
    phase: String,
    /// Retained log tail (ring-buffered to [`LOG_CAP`]).
    log: Vec<String>,
    /// Count of log lines evicted before `log[0]`, so a poll cursor maps to the
    /// right slice even after eviction.
    dropped: u64,
    error: Option<String>,
    /// Set to `true` to request cancellation; the running task selects on this.
    cancel: watch::Sender<bool>,
}

impl Job {
    fn is_terminal(&self) -> bool {
        !matches!(self.state, JobState::Running)
    }
}

/// Registry of background jobs, shared via [`crate::state::DaemonState`].
#[derive(Default)]
pub struct JobRegistry {
    inner: Mutex<HashMap<JobId, Job>>,
    next: AtomicU64,
}

impl JobRegistry {
    /// Allocate a fresh job (state `Running`) and return its id plus a receiver
    /// the running task selects on for cancellation.
    pub async fn create(&self) -> (JobId, watch::Receiver<bool>) {
        let id = format!("job-{}", self.next.fetch_add(1, Ordering::Relaxed));
        let (tx, rx) = watch::channel(false);
        let mut map = self.inner.lock().await;
        prune_terminal(&mut map);
        map.insert(
            id.clone(),
            Job {
                state: JobState::Running,
                phase: "Starting".to_owned(),
                log: Vec::new(),
                dropped: 0,
                error: None,
                cancel: tx,
            },
        );
        (id, rx)
    }

    /// Append a log line to a job (no-op if the job is gone or the line is
    /// blank after stripping). ANSI escapes are stripped first - the log is
    /// rendered in a plain-text panel, and some subprocesses (npm, git, the
    /// Laravel installer's spinner) emit them even with
    /// `NO_COLOR`/`TERM=dumb`/`--no-ansi` set on the child (see
    /// `crate::create_site::run_scaffold`). A line made up entirely of escapes
    /// (e.g. a bare cursor-hide/show or OSC title-set) strips to empty and is
    /// dropped rather than left as a blank entry.
    pub async fn push_log(&self, id: &str, line: String) {
        let line = crate::ansi::strip(&line);
        if line.is_empty() {
            return;
        }
        let mut map = self.inner.lock().await;
        if let Some(job) = map.get_mut(id) {
            job.log.push(line);
            if job.log.len() > LOG_CAP {
                let over = job.log.len() - LOG_CAP;
                job.log.drain(0..over);
                job.dropped += over as u64;
            }
        }
    }

    /// Update a job's current-phase label.
    pub async fn set_phase(&self, id: &str, phase: impl Into<String>) {
        let mut map = self.inner.lock().await;
        if let Some(job) = map.get_mut(id) {
            job.phase = phase.into();
        }
    }

    /// Move a job to a terminal state.
    pub async fn finish(&self, id: &str, state: JobState, error: Option<String>) {
        let mut map = self.inner.lock().await;
        if let Some(job) = map.get_mut(id) {
            job.state = state;
            job.error = error;
        }
    }

    /// Poll a job's progress for a client holding `cursor` log lines. Returns
    /// only newer lines plus the next cursor, or an error if the id is unknown.
    pub async fn poll(&self, id: &str, cursor: u64) -> Response {
        let map = self.inner.lock().await;
        let Some(job) = map.get(id) else {
            return not_found(id);
        };
        let total = job.dropped + job.log.len() as u64;
        let start = (cursor.saturating_sub(job.dropped) as usize).min(job.log.len());
        Response::JobProgress {
            state: job.state,
            phase: job.phase.clone(),
            log: job.log.get(start..).unwrap_or(&[]).to_vec(),
            next_cursor: total,
            error: job.error.clone(),
        }
    }

    /// Request cancellation of a running job. Idempotent; an unknown id errors.
    pub async fn cancel(&self, id: &str) -> Response {
        let map = self.inner.lock().await;
        let Some(job) = map.get(id) else {
            return not_found(id);
        };
        let _ = job.cancel.send(true);
        Response::Ok
    }
}

/// Drop the oldest terminal jobs once they exceed [`TERMINAL_CAP`]. Running jobs
/// are never pruned.
fn prune_terminal(map: &mut HashMap<JobId, Job>) {
    let terminal = map.values().filter(|j| j.is_terminal()).count();
    if terminal <= TERMINAL_CAP {
        return;
    }
    let mut victims: Vec<JobId> = map
        .iter()
        .filter(|(_, j)| j.is_terminal())
        .map(|(id, _)| id.clone())
        .collect();
    victims.sort_by_key(|id| id_suffix(id));
    for id in victims.into_iter().take(terminal - TERMINAL_CAP) {
        map.remove(&id);
    }
}

/// Numeric suffix of a `job-<n>` id (0 if malformed - never expected).
fn id_suffix(id: &str) -> u64 {
    id.rsplit('-')
        .next()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0)
}

fn not_found(id: &str) -> Response {
    Response::Error {
        code: ErrorCode::NotFound,
        message: format!("unknown job {id}"),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::*;

    fn progress(r: &Response) -> (JobState, &[String], u64, Option<&str>) {
        match r {
            Response::JobProgress {
                state,
                log,
                next_cursor,
                error,
                ..
            } => (*state, log.as_slice(), *next_cursor, error.as_deref()),
            other => panic!("expected JobProgress, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn push_log_strips_ansi_escapes() {
        let reg = JobRegistry::default();
        let (id, _cancel) = reg.create().await;

        reg.push_log(
            &id,
            "\x1b[36mCreating Laravel application...\x1b[39m".into(),
        )
        .await;

        let r = reg.poll(&id, 0).await;
        let (_, log, _, _) = progress(&r);
        assert_eq!(log, ["Creating Laravel application..."]);
    }

    #[tokio::test]
    async fn push_log_drops_lines_that_are_ansi_only() {
        let reg = JobRegistry::default();
        let (id, _cancel) = reg.create().await;

        reg.push_log(&id, "\x1b[?25l".into()).await;
        reg.push_log(&id, "visible".into()).await;

        let r = reg.poll(&id, 0).await;
        let (_, log, next, _) = progress(&r);
        assert_eq!(log, ["visible"]);
        assert_eq!(next, 1);
    }

    #[tokio::test]
    async fn cursor_slicing_returns_only_newer_lines() {
        let reg = JobRegistry::default();
        let (id, _cancel) = reg.create().await;

        reg.push_log(&id, "a".into()).await;
        reg.push_log(&id, "b".into()).await;

        let r = reg.poll(&id, 0).await;
        let (state, log, next, err) = progress(&r);
        assert_eq!(state, JobState::Running);
        assert_eq!(log, ["a", "b"]);
        assert_eq!(next, 2);
        assert_eq!(err, None);

        let r = reg.poll(&id, 2).await;
        let (_, log, next, _) = progress(&r);
        assert!(log.is_empty());
        assert_eq!(next, 2);

        reg.push_log(&id, "c".into()).await;
        let r = reg.poll(&id, 2).await;
        let (_, log, next, _) = progress(&r);
        assert_eq!(log, ["c"]);
        assert_eq!(next, 3);
    }

    #[tokio::test]
    async fn eviction_advances_dropped_and_cursor_stays_consistent() {
        let reg = JobRegistry::default();
        let (id, _cancel) = reg.create().await;
        for i in 0..(LOG_CAP + 10) {
            reg.push_log(&id, format!("line{i}")).await;
        }
        let r = reg.poll(&id, 0).await;
        let (_, log, next, _) = progress(&r);
        assert_eq!(next, (LOG_CAP + 10) as u64);
        assert_eq!(log.len(), LOG_CAP);
        assert_eq!(log[0], "line10");
    }

    #[tokio::test]
    async fn finish_sets_terminal_state_and_error() {
        let reg = JobRegistry::default();
        let (id, _cancel) = reg.create().await;
        reg.finish(&id, JobState::Failed, Some("boom".into())).await;
        let r = reg.poll(&id, 0).await;
        let (state, _, _, err) = progress(&r);
        assert_eq!(state, JobState::Failed);
        assert_eq!(err, Some("boom"));
    }

    #[tokio::test]
    async fn cancel_signals_receiver_then_errors_on_unknown() {
        let reg = JobRegistry::default();
        let (id, mut cancel) = reg.create().await;
        assert!(matches!(reg.cancel(&id).await, Response::Ok));
        assert!(cancel.has_changed().unwrap());
        assert!(*cancel.borrow_and_update());

        assert!(matches!(
            reg.poll("job-999", 0).await,
            Response::Error {
                code: ErrorCode::NotFound,
                ..
            }
        ));
    }
}
