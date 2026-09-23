//! Opt-in, observation-only lifecycle trace for disposable-runner tests.
//! The parent creates a fresh file and passes its path; ordinary launches do
//! not open any file. A marker records control flow, NOT recording integrity.
use std::{fs::OpenOptions, io::Write, path::Path};

pub fn record(event: &str) {
    if std::env::var("BILILIVE_ARTIFACT_OK").as_deref() != Ok("1") {
        return;
    }
    if let Some(path) = std::env::var_os("BILILIVE_ARTIFACT_TRACE") {
        // Do not create a caller-specified file in a normal app launch. The
        // artifact parent must have created this run's trace before spawn.
        if let Err(error) = append(Path::new(&path), std::process::id(), event) {
            eprintln!("artifact trace failed: {error}");
        }
    }
}

fn append(path: &Path, pid: u32, event: &str) -> std::io::Result<()> {
    let mut file = OpenOptions::new().append(true).open(path)?;
    file.write_all(format!("{pid} {event}\n").as_bytes())
}

pub fn contains(trace: &str, pid: u32, event: &str) -> bool {
    let marker = format!("{pid} {event}\n");
    trace.split_inclusive('\n').any(|line| line == marker)
}

pub fn close_progress(trace: &str, pid: u32) -> &'static str {
    for (event, state) in [
        ("run-exit", "EXIT_EVENT_OBSERVED"),
        ("exit-invoked", "EXIT_INVOKED_WAITING_FOR_PROCESS"),
        ("stop-backend-returned", "SHUTDOWN_RETURNED"),
        ("stop-backend-start", "SHUTDOWN_ENTERED_NOT_RETURNED"),
        ("close-requested", "CLOSE_DELIVERED"),
        ("main-window-ready", "GUI_READY_NO_CLOSE_EVENT"),
    ] {
        if contains(trace, pid, event) {
            return state;
        }
    }
    "GUI_NOT_READY"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markers_require_own_pid_exact_event_and_complete_line() {
        assert!(contains("7 main-window-ready\n", 7, "main-window-ready"));
        assert!(!contains("8 main-window-ready\n", 7, "main-window-ready"));
        assert!(!contains("7 main-window-ready", 7, "main-window-ready"));
        assert!(!contains(
            "7 not-main-window-ready\n",
            7,
            "main-window-ready"
        ));
    }

    #[test]
    fn traces_distinguish_no_delivery_from_shutdown_and_exit_stalls() {
        assert_eq!(close_progress("7 backend-ready\n", 7), "GUI_NOT_READY");
        let trace = "7 main-window-ready\n7 close-requested\n7 stop-backend-start\n";
        assert_eq!(close_progress(trace, 7), "SHUTDOWN_ENTERED_NOT_RETURNED");
        assert_eq!(
            close_progress("7 main-window-ready\n", 7),
            "GUI_READY_NO_CLOSE_EVENT"
        );
        assert_eq!(
            close_progress("7 exit-invoked\n", 7),
            "EXIT_INVOKED_WAITING_FOR_PROCESS"
        );
    }
}
