//! Artifact-level check that the packaged app never leaves its backend behind.
//!
//! This replaces `scripts/verify-sidecar-lifecycle.sh`, which is deleted. That
//! script captured the app's pid with `&`/`$!` and signalled it on the grounds
//! that a shell's background child stays unreaped, so its pid cannot be
//! recycled. **That is false for a shell.** Measurement:
//!
//! ```text
//! $ bash -c 'sleep 0.2 & pid=$!; sleep 1.5; ps -p "$pid" -o stat='
//! (no output — the process is already gone)
//! ```
//!
//! bash reaps background children itself, without an explicit `wait`, within
//! about a tenth of a second of the child exiting. The pid is released at that
//! moment and the kernel may hand it to anything, so `$!` carries no ownership
//! guarantee at all — it is a bare recorded pid. (`std::process::Child` is
//! different: the kernel reserves the pid until the parent calls `wait`, which
//! is what `an_unreaped_child_keeps_its_pid_reserved` in `sidecar_lifecycle.rs`
//! pins. That argument applies to `Child`, and it does not transfer to `$!`.)
//!
//! So this tool holds the app as a real [`Child`] and signals **only** that
//! value, and **only** while it is unreaped. Every other question is answered
//! from outside the process table entirely: the app is told which port to use
//! and the verdict is whether that port still answers.
//!
//! # What the verdict does and does not prove
//!
//! "The app exited" and "the backend stopped" are different claims, and neither
//! is "the recording was flushed to disk". This tool keeps them apart instead
//! of collapsing them into one pass:
//!
//! | Signal | How it is observed | What it proves |
//! | --- | --- | --- |
//! | backend stopped serving | the loopback port stops answering | the backend is no longer accepting requests — **not** that the process is gone, and not that it flushed |
//! | recording flushed | the bundled CLI's own log ends in a clean shutdown | the CLI reached its shutdown path rather than being cut off mid-write |
//! | the app exited | `Child::wait` on the process this tool started | the app itself is gone, and with what exit status |
//!
//! **The backend process exiting is deliberately not observed here.** The only
//! ways to see it are enumerating the process table or reading the guardian's
//! event pipe, and this tool does neither: enumeration is what made the deleted
//! shell script unsafe, and the app consumes the event pipe itself. So a pass
//! here means "the backend stopped serving and the CLI reported a clean
//! shutdown", which is the property users care about, but it is not a claim
//! about the process table.
//!
//! The flush signal is only checked when `BILILIVE_ARTIFACT_CLI_LOG` points at
//! the CLI's log file, and it is read **from the length the file had before
//! this run started**, so a shutdown recorded by an earlier session cannot be
//! mistaken for this one's. Without the log path the tool says so rather than
//! implying it proved something it did not.
//!
//! **It is not evidence about recording files.** A shutdown line in the log
//! means the CLI entered its shutdown path; it does not mean any particular
//! `.flv` is intact, complete, or was flushed before the process ended. Proving
//! that needs the files themselves — readable, expected duration, expected
//! checksum — checked separately, on the recording the run was actually
//! writing. Nothing in this tool does that, and a pass here must not be read as
//! if it did.
//!
//! # Running it
//!
//! ```text
//! BILILIVE_ARTIFACT_APP="/path/to/BililiveRecorder GUI.app/Contents/MacOS/bililive-recorder-gui" \
//! BILILIVE_ARTIFACT_OK=1 \
//!   cargo test --test artifact_lifecycle -- --ignored --test-threads 1
//! ```
//!
//! Both tests are `#[ignore]`d and refuse to start without
//! `BILILIVE_ARTIFACT_OK=1`, because they launch and terminate a real app.
//!
//! **The acknowledgement is not what makes this safe.** Ownership is what makes
//! it pid-safe: the tool can only ever signal the one process it started. What
//! the acknowledgement buys is data safety — these tests stop a live recorder
//! mid-write, so they belong on a disposable machine, not on one where someone
//! is recording. Nothing else on the machine is touched either way.

#![cfg(any(unix, windows))]

use bililive_recorder_gui_lib::artifact_probe;
use std::{
    fs,
    net::{Ipv4Addr, SocketAddr, TcpStream},
    path::PathBuf,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

/// How long the backend gets to come up before the run is called a failure.
const STARTUP: Duration = Duration::from_secs(60);
/// How long the backend gets to be gone after the app has gone.
const SETTLE: Duration = Duration::from_secs(20);

fn app_path() -> PathBuf {
    let raw = std::env::var_os("BILILIVE_ARTIFACT_APP").unwrap_or_else(|| {
        panic!(
            "set BILILIVE_ARTIFACT_APP to the app executable \
             (…/BililiveRecorder GUI.app/Contents/MacOS/bililive-recorder-gui)"
        )
    });
    let path = PathBuf::from(raw);
    assert!(path.is_file(), "not a file: {}", path.display());
    path
}

/// Refuses to run unless a human has said this machine may lose a recording.
fn acknowledged() {
    assert_eq!(
        std::env::var("BILILIVE_ARTIFACT_OK").as_deref(),
        Ok("1"),
        "these tests stop a real recorder mid-write. Run them on a disposable \
         machine and set BILILIVE_ARTIFACT_OK=1 to say so."
    );
}

/// The app under test, owned by this process for as long as it lives.
struct App {
    child: Child,
    port: u16,
    trace: PathBuf,
    /// How long the CLI's log was before this run, so the flush signal is
    /// bound to this run's own output.
    log_length_before: u64,
}

impl App {
    fn launch(port: u16) -> Self {
        // The app hands this port to the backend verbatim, so the test can
        // watch the backend without ever enumerating a process.
        let evidence = std::env::var_os("BILILIVE_ARTIFACT_EVIDENCE_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        let run = evidence.join(format!(
            "gui-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&run).expect("create this run's evidence directory");
        let trace = run.join("lifecycle.log");
        fs::File::create(&trace).expect("create fresh trace before spawning app");
        let output = fs::File::create(run.join("app-output.log")).expect("create app log");
        let mut command = Command::new(app_path());
        command
            .env("BILILIVE_GUI_BIND_PORT", port.to_string())
            .env("BILILIVE_ARTIFACT_TRACE", &trace)
            .env("BILILIVE_RECORDER_GUI_WORKDIR", run.join("recordings"))
            .stdin(if cfg!(windows) {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::from(output.try_clone().expect("clone app log")))
            .stderr(Stdio::from(output));
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x0800_0000);
        }

        let log_length_before = cli_log_length_before();
        let child = command.spawn().expect("the app should start");
        let app = Self {
            child,
            port,
            trace,
            log_length_before,
        };
        app.await_backend();
        app
    }

    fn backend_answers(&self) -> bool {
        TcpStream::connect_timeout(
            &SocketAddr::from((Ipv4Addr::LOCALHOST, self.port)),
            Duration::from_secs(1),
        )
        .is_ok()
    }

    fn await_backend(&self) {
        let deadline = Instant::now() + STARTUP;
        while !self.backend_answers() {
            assert!(
                Instant::now() < deadline,
                "the backend never started listening on port {}",
                self.port
            );
            std::thread::sleep(Duration::from_millis(250));
        }
    }

    /// The app's pid. Safe to signal only while `self.child` is unreaped, which
    /// is exactly what the callers below guarantee by construction.
    ///
    /// Unix signals use this. Windows normal quit uses the owned stdin pipe,
    /// and trace matching uses the PID only to identify the writer.
    #[allow(dead_code)]
    fn pid(&self) -> u32 {
        self.child.id()
    }

    /// Asks the app to quit the way a user would.
    ///
    /// This is the path that was broken: on macOS a normal quit delivers
    /// `RunEvent::Exit` and never `CloseRequested`.
    fn request_quit(&mut self) {
        #[cfg(target_os = "macos")]
        {
            // Apple Event `quit`, by executable name. Not yet observed to work
            // against an app started from its bundle executable rather than by
            // `open`; if it silently does nothing the timeout below says so.
            let status = Command::new("osascript")
                .args(["-e", "tell application \"BililiveRecorder GUI\" to quit"])
                .status()
                .expect("osascript should run");
            assert!(status.success(), "osascript could not ask the app to quit");
        }

        #[cfg(windows)]
        {
            // CI proved taskkill can report success after GUI readiness yet
            // never reach CloseRequested. Use the app's opt-in owned pipe to
            // call Tauri main.close(), which still traverses the real callback.
            let pid = self.pid();
            let child = &mut self.child;
            request_quit_when_ready(&self.trace, pid, STARTUP, SETTLE, || {
                let input = child.stdin.as_mut().ok_or("app control pipe missing")?;
                artifact_probe::write_close_request(input).map_err(|error| error.to_string())
            })
            .unwrap_or_else(|error| panic!("{error}"));
        }

        #[cfg(all(unix, not(target_os = "macos")))]
        {
            // No window server to ask; SIGTERM is the graceful request.
            self.signal(libc::SIGTERM);
        }
    }

    /// Only macOS skips this: it has a window server to ask instead.
    #[cfg(unix)]
    #[allow(dead_code)]
    fn signal(&self, signal: libc::c_int) {
        // Exact: `self.child` is unreaped, so this pid still belongs to it.
        unsafe { libc::kill(self.pid() as libc::c_int, signal) };
    }

    /// Ends the app outright, the way a crash or `SIGKILL` would.
    fn kill(&mut self) {
        let _ = self.child.kill();
    }

    /// Waits for the app to exit, then returns whether the backend went with it.
    fn observe_backend_shutdown(&self) -> bool {
        let deadline = Instant::now() + SETTLE;
        while Instant::now() < deadline {
            if !self.backend_answers() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(250));
        }
        false
    }

    /// Waits for the app to exit, up to a deadline. See [`wait_for_exit`].
    fn reap_within(&mut self, within: Duration) -> Option<i32> {
        wait_for_exit(&mut self.child, within)
    }
}

impl Drop for App {
    fn drop(&mut self) {
        eprintln!(
            "GUI lifecycle trace ({}):\n{}",
            self.trace.display(),
            fs::read_to_string(&self.trace).unwrap_or_default()
        );
        // Cleanup is by ownership only. If the app is somehow still there, it
        // is still our unreaped child, so this stays exact.
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Where the bundled CLI writes its log, if the caller said.
///
/// The app passes `BILILIVERECORDER_LOG_FILE_PATH` to the CLI; whoever runs
/// this tool knows the app's log directory, so it is supplied rather than
/// guessed.
fn cli_log_path() -> Option<PathBuf> {
    std::env::var_os("BILILIVE_ARTIFACT_CLI_LOG").map(PathBuf::from)
}

/// How long the CLI's log already was, before this run touched anything.
///
/// Read before the app is launched so that only what this run appends is
/// searched later.
fn cli_log_length_before() -> u64 {
    cli_log_path()
        .and_then(|path| std::fs::metadata(path).ok())
        .map(|meta| meta.len())
        .unwrap_or(0)
}

/// Whether the CLI's log shows it reached its shutdown path **during this run**.
///
/// `None` means the question was not asked — no log path was supplied, or the
/// file is not there — and the caller must report that as unproven rather than
/// as a pass.
fn recording_was_flushed(since: u64) -> Option<bool> {
    let path = cli_log_path()?;
    let bytes = std::fs::read(path).ok()?;
    // If the log was rotated or truncated the offset is meaningless; fall back
    // to the whole file rather than silently reading nothing.
    let tail = if bytes.len() as u64 >= since {
        &bytes[since as usize..]
    } else {
        &bytes[..]
    };
    let text = String::from_utf8_lossy(tail);
    // The upstream CLI logs these as it disposes of its recording pipeline.
    Some(text.contains("Shutdown in progress") || text.contains("Dispose called"))
}

/// Waits for `child` to exit, up to a deadline, returning its exit code.
///
/// Deliberately **not** `Child::wait`: this tool asks the app to quit through
/// drivers whose effect is not yet established on every platform (see
/// `request_quit`), and if one of them silently does nothing an unbounded wait
/// hangs the run instead of failing it. A test that hangs proves nothing and
/// reports nothing — on CI it burns the job to its timeout and leaves no
/// diagnostic — so the wait is bounded and the absence of an exit becomes the
/// assertion instead.
fn wait_for_exit(child: &mut Child, within: Duration) -> Option<i32> {
    let deadline = Instant::now() + within;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Some(status.code().unwrap_or(-1)),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(100)),
            // Still running when the time ran out, or unwaitable.
            _ => return None,
        }
    }
}

fn free_port() -> u16 {
    std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .expect("a free port")
        .local_addr()
        .expect("a local address")
        .port()
}

/// A user quitting the app must take the backend down, and the CLI must get
/// the chance to flush.
#[test]
#[ignore = "launches and terminates a real app; needs a disposable machine"]
fn quitting_the_app_stops_the_backend() {
    acknowledged();
    let mut app = App::launch(free_port());
    app.request_quit();

    let stopped_serving = app.observe_backend_shutdown();
    let flushed = recording_was_flushed(app.log_length_before);
    let exit = app.reap_within(SETTLE);

    // The app exiting is checked before the backend is, so a quit driver that
    // silently did nothing is reported as exactly that rather than as a
    // backend that kept serving.
    assert!(
        exit.is_some(),
        "the app never exited after being asked to quit; observed phase: {}",
        artifact_probe::close_progress(
            &fs::read_to_string(&app.trace).unwrap_or_default(),
            app.pid()
        )
    );
    assert!(
        stopped_serving,
        "the app exited but the backend was still serving"
    );
    if flushed == Some(false) {
        panic!(
            "the backend stopped serving but the CLI log shows no clean \
             shutdown, so the recording may have been cut off mid-write"
        );
    }
    if flushed.is_none() {
        eprintln!(
            "note: BILILIVE_ARTIFACT_CLI_LOG was not supplied or unreadable, so \
             the flush signal is UNPROVEN for this run"
        );
    }
}

/// An app killed outright must still take the backend down. This is the path
/// the guardian exists for.
#[test]
#[ignore = "launches and terminates a real app; needs a disposable machine"]
fn killing_the_app_stops_the_backend() {
    acknowledged();
    let mut app = App::launch(free_port());
    app.kill();

    let stopped_serving = app.observe_backend_shutdown();
    let flushed = recording_was_flushed(app.log_length_before);
    let exit = app.reap_within(SETTLE);

    assert!(
        exit.is_some(),
        "the app did not exit after being killed outright"
    );
    assert!(
        stopped_serving,
        "the backend outlived an app that was killed outright"
    );
    // A forced kill cannot promise a flush; the guardian's graceful stage is
    // what gives the CLI its chance. Report it, do not assert it.
    match flushed {
        Some(value) => eprintln!("note: CLI reported a clean shutdown: {value}"),
        None => eprintln!("note: flush signal UNPROVEN (no CLI log supplied)"),
    }
}

// ---------------------------------------------------------------------------
// The wait itself, which needs no app and therefore runs everywhere
// ---------------------------------------------------------------------------

#[test]
fn a_child_that_does_not_exit_is_reported_rather_than_waited_on_forever() {
    // The regression this pins: the first version of this tool called
    // `Child::wait`, so a quit driver that silently did nothing hung the run
    // instead of failing it. Nothing here launches the app, so unlike the two
    // artifact cases above this one runs normally on every platform.
    let mut stubborn = Command::new(if cfg!(windows) { "cmd" } else { "/bin/sh" })
        .args(if cfg!(windows) {
            ["/C", "ping -n 30 127.0.0.1 >NUL"]
        } else {
            ["-c", "exec sleep 30"]
        })
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("the stand-in should start");

    let started = Instant::now();
    let outcome = wait_for_exit(&mut stubborn, Duration::from_millis(400));
    let elapsed = started.elapsed();

    assert_eq!(
        outcome, None,
        "a running child must be reported as not exited"
    );
    assert!(
        elapsed < Duration::from_secs(10),
        "the wait must be bounded, took {elapsed:?}"
    );

    // Still this test's own process, still unreaped: killing it is exact.
    let _ = stubborn.kill();
    let _ = stubborn.wait();
}

#[test]
fn a_child_that_exits_reports_its_code() {
    let mut quick = Command::new(if cfg!(windows) { "cmd" } else { "/bin/sh" })
        .args(if cfg!(windows) {
            ["/C", "exit 3"]
        } else {
            ["-c", "exit 3"]
        })
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("the stand-in should start");

    assert_eq!(
        wait_for_exit(&mut quick, Duration::from_secs(10)),
        Some(3),
        "an exited child must report its code"
    );
}

// Kept outside cfg(windows) so the exact Windows driver's handshake can be
// tested without starting, signalling, or terminating any OS process.
#[cfg(any(windows, test))]
fn request_quit_when_ready(
    trace: &std::path::Path,
    pid: u32,
    ready_timeout: Duration,
    ack_timeout: Duration,
    request: impl FnOnce() -> Result<(), String>,
) -> Result<(), String> {
    wait_for_gui_event(trace, pid, "main-window-ready", ready_timeout)?;
    request()?;
    wait_for_gui_event(trace, pid, "close-requested", ack_timeout)
}

#[cfg(any(windows, test))]
fn wait_for_gui_event(
    trace: &std::path::Path,
    pid: u32,
    event: &str,
    timeout: Duration,
) -> Result<(), String> {
    let deadline = Instant::now() + timeout;
    loop {
        let contents =
            fs::read_to_string(trace).map_err(|e| format!("cannot read GUI trace: {e}"))?;
        if artifact_probe::contains(&contents, pid, event) {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "GUI event {event} not observed: {}; trace: {contents}",
                artifact_probe::close_progress(&contents, pid)
            ));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[cfg(test)]
mod quit_handshake_tests {
    use super::*;
    fn fixture(run: impl FnOnce(&std::path::Path)) {
        let path = std::env::temp_dir().join(format!(
            "quit-trace-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::write(&path, "7 backend-ready\n").unwrap();
        run(&path);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn backend_listening_does_not_license_a_gui_quit_request() {
        fixture(|trace| {
            let mut calls = 0;
            let result = request_quit_when_ready(trace, 7, Duration::ZERO, Duration::ZERO, || {
                calls += 1;
                Ok(())
            });
            assert_eq!(calls, 0, "must not send quit before main-window-ready");
            assert!(result.unwrap_err().contains("GUI_NOT_READY"));
        });
    }

    #[test]
    fn successful_driver_without_close_event_is_not_delivery() {
        fixture(|trace| {
            fs::write(trace, "7 main-window-ready\n").unwrap();
            let mut calls = 0;
            let result = request_quit_when_ready(trace, 7, Duration::ZERO, Duration::ZERO, || {
                calls += 1;
                Ok(())
            });
            assert_eq!(calls, 1);
            assert!(result.unwrap_err().contains("GUI_READY_NO_CLOSE_EVENT"));
        });
    }

    #[test]
    fn delivered_close_is_acknowledged_but_not_mistaken_for_process_exit() {
        fixture(|trace| {
            fs::write(trace, "7 main-window-ready\n").unwrap();
            request_quit_when_ready(trace, 7, Duration::ZERO, Duration::ZERO, || {
                fs::write(
                    trace,
                    "7 main-window-ready\n7 close-requested\n7 stop-backend-start\n",
                )
                .unwrap();
                Ok(())
            })
            .unwrap();
            assert_eq!(
                artifact_probe::close_progress(&fs::read_to_string(trace).unwrap(), 7),
                "SHUTDOWN_ENTERED_NOT_RETURNED"
            );
        });
    }
}
