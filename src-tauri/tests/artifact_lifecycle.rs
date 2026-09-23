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

use std::{
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
    /// How long the CLI's log was before this run, so the flush signal is
    /// bound to this run's own output.
    log_length_before: u64,
}

impl App {
    fn launch(port: u16) -> Self {
        // The app hands this port to the backend verbatim, so the test can
        // watch the backend without ever enumerating a process.
        let mut command = Command::new(app_path());
        command
            .env("BILILIVE_GUI_BIND_PORT", port.to_string())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
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
    /// macOS asks the app to quit by Apple Event instead, so it does not use
    /// this; the pid is still what `taskkill` and the signals need.
    #[allow(dead_code)]
    fn pid(&self) -> u32 {
        self.child.id()
    }

    /// Asks the app to quit the way a user would.
    ///
    /// This is the path that was broken: on macOS a normal quit delivers
    /// `RunEvent::Exit` and never `CloseRequested`.
    fn request_quit(&self) {
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
            // WM_CLOSE to the app's windows: the close-requested path.
            let status = Command::new("taskkill")
                .args(["/PID", &self.pid().to_string()])
                .status()
                .expect("taskkill should run");
            assert!(status.success(), "taskkill could not ask the app to close");
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

    /// Reaps the app, returning its exit code. Nothing may signal it
    /// afterwards.
    fn reap(&mut self) -> Option<i32> {
        self.child
            .wait()
            .ok()
            .map(|status| status.code().unwrap_or(-1))
    }
}

impl Drop for App {
    fn drop(&mut self) {
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
    let exit = app.reap();

    assert!(
        stopped_serving,
        "the backend was still serving after the app was asked to quit"
    );
    assert!(
        exit.is_some(),
        "the app was never reaped after being asked to quit"
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
    let _ = app.reap();

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
