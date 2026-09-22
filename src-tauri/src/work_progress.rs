//! A scoped heartbeat keeps silent subprocesses and filesystem cleanup visible.
//! Elapsed time is explicitly not presented as byte or percentage progress.
use std::{
    cell::RefCell,
    sync::{mpsc, Arc, Mutex},
    thread,
    time::{Duration, Instant},
};
use tauri::Emitter;

type Sink = Arc<dyn Fn(String, bool) + Send + Sync>;
#[derive(Clone)]
struct Activity {
    phase: String,
    detail: String,
    started: Instant,
    updated: Instant,
}
impl Activity {
    fn new(phase: &str) -> Self {
        Self {
            phase: phase.into(),
            detail: String::new(),
            started: Instant::now(),
            updated: Instant::now(),
        }
    }
    fn message(&self) -> String {
        let elapsed = self.started.elapsed().as_secs();
        let mut message = format!("{} · {}:{:02} min", self.phase, elapsed / 60, elapsed % 60);
        if !self.detail.is_empty() {
            message.push_str(&format!(" · {}", self.detail));
        }
        if self.updated.elapsed().as_secs() >= 5 {
            message.push_str(" · warte auf Abschluss dieses Arbeitsschritts");
        }
        message
    }
}
struct Reporter {
    activity: Mutex<Activity>,
    first_failure: Mutex<Option<String>>,
    sink: Sink,
}
thread_local! { static REPORTER: RefCell<Option<Arc<Reporter>>> = const { RefCell::new(None) }; }

pub(super) struct Session {
    stop: mpsc::Sender<()>,
    thread: Option<thread::JoinHandle<()>>,
}
impl Session {
    pub fn attach(window: tauri::Window) -> Self {
        Self::with_sink(Arc::new(move |message, log| {
            let _ = window.emit("backup-activity", &message);
            if log {
                let _ = window.emit("backup-log", &message);
            }
        }))
    }
    fn with_sink(sink: Sink) -> Self {
        let reporter = Arc::new(Reporter {
            activity: Mutex::new(Activity::new("Backup wird vorbereitet")),
            first_failure: Mutex::new(None),
            sink,
        });
        REPORTER.with(|r| {
            assert!(r.borrow().is_none(), "Nested progress sessions");
            *r.borrow_mut() = Some(reporter.clone());
        });
        let (stop, receiver) = mpsc::channel();
        let thread = thread::spawn(move || {
            let mut ticks = 0;
            while receiver.recv_timeout(Duration::from_secs(1))
                == Err(mpsc::RecvTimeoutError::Timeout)
            {
                ticks += 1;
                let activity = reporter.activity.lock().unwrap();
                (reporter.sink)(activity.message(), ticks % 10 == 0);
            }
        });
        Self {
            stop,
            thread: Some(thread),
        }
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.stop.send(());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        REPORTER.with(|r| *r.borrow_mut() = None);
    }
}

pub(super) struct Phase {
    previous: Option<Activity>,
}
impl Phase {
    pub fn enter(label: &str) -> Self {
        let previous = REPORTER.with(|r| {
            r.borrow().as_ref().map(|reporter| {
                let activity = Activity::new(label);
                let message = activity.message();
                let mut state = reporter.activity.lock().unwrap();
                let previous = std::mem::replace(&mut *state, activity);
                (reporter.sink)(message, true);
                previous
            })
        });
        Self { previous }
    }
}
impl Drop for Phase {
    fn drop(&mut self) {
        if let Some(previous) = self.previous.take() {
            REPORTER.with(|r| {
                if let Some(reporter) = r.borrow().as_ref() {
                    *reporter.activity.lock().unwrap() = previous;
                }
            });
        }
    }
}
pub(super) fn detail(message: String, log: bool) {
    REPORTER.with(|r| {
        if let Some(reporter) = r.borrow().as_ref() {
            let mut activity = reporter.activity.lock().unwrap();
            activity.detail = message;
            activity.updated = Instant::now();
            let rendered = activity.message();
            (reporter.sink)(rendered, log);
        }
    });
}

/// Report at the point of failure, before an owning temporary directory is
/// dropped. Returning the error to the UI alone can hide it behind long cleanup.
pub(super) fn report_result<T>(result: Result<T, String>) -> Result<T, String> {
    if let Err(error) = &result {
        REPORTER.with(|r| {
            if let Some(reporter) = r.borrow().as_ref() {
                let mut first = reporter.first_failure.lock().unwrap();
                if first.is_none() {
                    let phase = reporter.activity.lock().unwrap().phase.clone();
                    (reporter.sink)(
                        format!("Arbeitsschritt fehlgeschlagen – {phase}: {error}"),
                        true,
                    );
                    *first = Some(error.clone());
                }
            }
        });
    }
    result
}

pub(super) fn cleanup(path: &std::path::Path, remove: impl FnOnce() -> std::io::Result<()>) {
    let failed = REPORTER.with(|r| {
        r.borrow()
            .as_ref()
            .is_some_and(|reporter| reporter.first_failure.lock().unwrap().is_some())
    });
    let _phase = Phase::enter(if failed {
        "Temporäre Dateien nach Fehler aufräumen"
    } else {
        "Temporäre Dateien aufräumen"
    });
    detail(format!("Ordner: {}", path.display()), true);
    if let Err(error) = remove() {
        if error.kind() != std::io::ErrorKind::NotFound {
            detail(
                format!("Aufräumen fehlgeschlagen: {}: {error}", path.display()),
                true,
            );
        }
    }
}

/// Byte-based work emits at most twice a second, regardless of buffer size.
pub(super) struct Bytes {
    total: u64,
    last: Instant,
}
impl Bytes {
    pub fn new() -> Self {
        Self {
            total: 0,
            last: Instant::now(),
        }
    }
    pub fn add(&mut self, bytes: usize) {
        self.total += bytes as u64;
        if self.last.elapsed() >= Duration::from_millis(500) {
            detail(
                format!("{:.1} MiB gelesen", self.total as f64 / (1024.0 * 1024.0)),
                false,
            );
            self.last = Instant::now();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{run_with_timeout, BACKUP_CANCELLED, VERIFY_CANCELLED};
    use std::{process::Command, sync::atomic::Ordering};

    #[test]
    fn original_failure_is_logged_before_temporary_directory_cleanup() {
        let messages = Arc::new(Mutex::new(Vec::new()));
        let captured = messages.clone();
        let session = Session::with_sink(Arc::new(move |message, log| {
            if log {
                captured.lock().unwrap().push(message);
            }
        }));
        let path;
        {
            let directory = crate::restore::PrivateDir::temp().unwrap();
            path = directory.0.clone();
            std::fs::write(path.join("partial-archive"), b"incomplete").unwrap();
            let _phase = Phase::enter("Archiv erstellen: Preferences");
            let error = "tar: (null); zstd: Broken pipe".to_string();
            let result = report_result::<()>(Err(error.clone()));
            assert_eq!(result.unwrap_err(), error);
            // Propagating the same failure through another layer must not
            // replace the first failing phase or emit it a second time.
            assert!(report_result::<()>(Err(error)).is_err());
        }
        assert!(!path.exists());
        drop(session);
        let messages = messages.lock().unwrap();
        let error = messages
            .iter()
            .position(|m| m.contains("Arbeitsschritt fehlgeschlagen"))
            .unwrap();
        let cleanup = messages
            .iter()
            .position(|m| m.contains("Temporäre Dateien nach Fehler aufräumen"))
            .unwrap();
        assert!(error < cleanup);
        assert!(messages[error].contains("Archiv erstellen: Preferences"));
        assert_eq!(
            messages
                .iter()
                .filter(|m| m.contains("Arbeitsschritt fehlgeschlagen"))
                .count(),
            1
        );
        assert!(messages
            .iter()
            .any(|m| m.contains(&format!("Ordner: {}", path.display()))));
    }

    #[test]
    fn cleanup_failure_is_reported_separately() {
        let messages = Arc::new(Mutex::new(Vec::new()));
        let captured = messages.clone();
        let session = Session::with_sink(Arc::new(move |message, _| {
            captured.lock().unwrap().push(message);
        }));
        cleanup(std::path::Path::new("/test/private-staging"), || {
            Err(std::io::Error::from_raw_os_error(libc::EIO))
        });
        drop(session);
        let messages = messages.lock().unwrap();
        assert!(messages
            .iter()
            .any(|m| m.contains("Aufräumen fehlgeschlagen: /test/private-staging")));
        assert!(!messages.iter().any(|m| m.contains("nach Fehler aufräumen")));
    }

    #[test]
    fn silent_subprocess_emits_phase_heartbeat_and_stops_on_drop() {
        BACKUP_CANCELLED.store(false, Ordering::SeqCst);
        VERIFY_CANCELLED.store(false, Ordering::SeqCst);
        let output = Arc::new(Mutex::new(Vec::new()));
        let capture = output.clone();
        let session = Session::with_sink(Arc::new(move |message, _| {
            capture.lock().unwrap().push(message)
        }));
        {
            let _phase = Phase::enter("Archiv entpacken / macOS-Metadaten setzen");
            let mut cmd = Command::new("/bin/sleep");
            cmd.arg("1.2");
            run_with_timeout(cmd, Duration::from_secs(5)).unwrap();
        }
        drop(session);
        let messages = output.lock().unwrap();
        assert!(messages.len() >= 2);
        assert!(messages
            .iter()
            .any(|m| m.contains("Metadaten setzen · 0:01 min")));
        assert!(!messages.iter().any(|m| m.contains("Prüfe Dateien")));
        REPORTER.with(|r| assert!(r.borrow().is_none()));
    }
    #[test]
    fn nested_phases_reset_stale_details_and_restore_parent() {
        let session = Session::with_sink(Arc::new(|_, _| {}));
        let _parent = Phase::enter("Rückleseprüfung");
        detail("alte Datei".into(), false);
        {
            let _child = Phase::enter("Entpacken");
            REPORTER.with(|r| {
                let reporter = r.borrow();
                let state = reporter.as_ref().unwrap().activity.lock().unwrap();
                assert_eq!(state.phase, "Entpacken");
                assert!(state.detail.is_empty());
            });
        }
        REPORTER.with(|r| {
            assert_eq!(
                r.borrow().as_ref().unwrap().activity.lock().unwrap().phase,
                "Rückleseprüfung"
            )
        });
        drop(_parent);
        drop(session);
    }
    #[test]
    fn cancellation_during_silent_subprocess_remains_prompt() {
        BACKUP_CANCELLED.store(false, Ordering::SeqCst);
        VERIFY_CANCELLED.store(false, Ordering::SeqCst);
        let _session = Session::with_sink(Arc::new(|_, _| {}));
        let _phase = Phase::enter("Entpacken");
        let trigger = thread::spawn(|| {
            thread::sleep(Duration::from_millis(150));
            BACKUP_CANCELLED.store(true, Ordering::SeqCst);
        });
        let mut cmd = Command::new("/bin/sleep");
        cmd.arg("20");
        let start = Instant::now();
        let result = run_with_timeout(cmd, Duration::from_secs(30));
        trigger.join().unwrap();
        BACKUP_CANCELLED.store(false, Ordering::SeqCst);
        assert!(result.unwrap_err().contains("abgebrochen"));
        assert!(start.elapsed() < Duration::from_secs(3));
    }
}
