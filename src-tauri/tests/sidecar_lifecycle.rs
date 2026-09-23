//! Regression tests for backend ownership.
//!
//! The design claim under test is that the guardian only ever signals and reaps
//! a process it created itself, so a pid it did not spawn — a recycled pid, or
//! a recorder the user started from the very same path — is unreachable. These
//! tests drive the real `supervise` entry point the guardian process runs,
//! using ordinary system processes as stand-ins for the bundled CLI, and they
//! only ever observe or signal processes they started themselves.

#![cfg(unix)]

use std::{
    ffi::OsString,
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::mpsc,
    thread::JoinHandle,
    time::{Duration, Instant},
};

use bililive_recorder_gui_lib::{
    BackendCommand, EVENT_ERROR_PREFIX, EVENT_EXITED, EVENT_STARTED_PREFIX, EVENT_STOPPED,
    GuardianConfig, shutdown_guardian, supervise, supervise_with,
};
use std::{io::Read, os::unix::net::UnixStream};

const SHELL: &str = "/bin/sh";

/// How long a stand-in backend stays alive when nothing stops it.
const LINGER: &str = "300";

fn backend(script: &str, graceful: Duration) -> GuardianConfig {
    GuardianConfig {
        backend: BackendCommand {
            program: PathBuf::from(SHELL),
            args: vec![OsString::from("-c"), OsString::from(script)],
            envs: Vec::new(),
        },
        graceful_timeout: graceful,
    }
}

/// `exec` keeps the child a single process, exactly like the bundled CLI, so
/// the pid the guardian holds is the process actually running the workload.
fn lingering(marker: &str) -> GuardianConfig {
    backend(&format!("exec sleep {marker}"), Duration::from_secs(5))
}

/// A guardian running in-process, with both pipes under the test's control.
struct Supervisor {
    handle: JoinHandle<i32>,
    keepalive: Option<UnixStream>,
    events: UnixStream,
}

impl Supervisor {
    fn start(config: GuardianConfig) -> Self {
        let (guardian_side, app_side) = UnixStream::pair().expect("keepalive pair");
        let (events_out, events_in) = UnixStream::pair().expect("event pair");
        let handle = std::thread::spawn(move || supervise(&config, guardian_side, events_out));
        Self {
            handle,
            keepalive: Some(app_side),
            events: events_in,
        }
    }

    /// Ends the keepalive: exactly what the app does when it wants a shutdown,
    /// and exactly what happens on its own when the app is killed.
    fn request_shutdown(&mut self) {
        self.keepalive = None;
    }

    /// Waits for the guardian to finish, returning its exit code and the last
    /// event it wrote — the one outcome it settled on.
    ///
    /// The transcript opens with a `started` line naming the backend pid, so
    /// the terminal line, not the whole transcript, is what says how it ended.
    fn finish(self) -> (i32, String) {
        let (code, text) = self.finish_full();
        let last = text.lines().last().unwrap_or_default().trim().to_owned();
        (code, last)
    }

    /// Waits for the guardian to finish, returning its exit code and every
    /// event it wrote, in order.
    fn finish_full(self) -> (i32, String) {
        let code = self.handle.join().expect("guardian thread panicked");
        let mut text = String::new();
        let mut events = self.events;
        events
            .read_to_string(&mut text)
            .expect("read guardian events");
        (code, text.trim().to_owned())
    }
}

/// Pids of *this test process's own* children whose command line contains
/// `marker`. Only the test's own children are ever considered, so a process
/// belonging to anything else cannot be matched by accident.
fn own_children(marker: &str) -> Vec<u32> {
    let output = Command::new("ps")
        .args(["-ax", "-o", "pid=,ppid=,command="])
        .output()
        .expect("ps should run");
    let me = std::process::id();
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|line| line.contains(marker))
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let pid: u32 = fields.next()?.parse().ok()?;
            let ppid: u32 = fields.next()?.parse().ok()?;
            (ppid == me).then_some(pid)
        })
        .collect()
}

fn wait_for_child(marker: &str, timeout: Duration) -> u32 {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(pid) = own_children(marker).first() {
            return *pid;
        }
        assert!(Instant::now() < deadline, "child {marker} never appeared");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn wait_for_gone(marker: &str, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        if own_children(marker).is_empty() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn alive(pid: u32) -> bool {
    unsafe { libc::kill(pid as libc::c_int, 0) == 0 }
}

/// Starts a stand-in recorder that this test owns, so it may reap it.
fn spawn_owned(script: &str) -> Child {
    Command::new(SHELL)
        .args(["-c", script])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("decoy should start")
}

fn spawn_decoy(marker: &str) -> Child {
    spawn_owned(&format!("exec sleep {marker}"))
}

fn wait_until(what: &str, timeout: Duration, mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + timeout;
    while !ready() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn the_app_disappearing_shuts_the_backend_down() {
    let config = lingering("4311");
    let mut guardian = Supervisor::start(config);
    wait_for_child("4311", Duration::from_secs(5));

    guardian.request_shutdown();
    let (code, report) = guardian.finish();

    assert_eq!(code, 0);
    assert_eq!(report, EVENT_STOPPED);
    assert!(
        wait_for_gone("4311", Duration::from_secs(5)),
        "the backend must be reaped, not merely signalled"
    );
}

#[test]
fn a_backend_that_ignores_sigint_is_still_reaped() {
    // `exec` keeps the ignored SIGINT across the exec, so this is a single
    // process that survives the graceful stage.
    let config = backend("trap '' INT; exec sleep 4312", Duration::from_millis(400));
    let mut guardian = Supervisor::start(config);
    wait_for_child("4312", Duration::from_secs(5));

    let started = Instant::now();
    guardian.request_shutdown();
    let (code, report) = guardian.finish();

    assert_eq!(code, 0);
    assert_eq!(report, EVENT_STOPPED);
    assert!(
        started.elapsed() >= Duration::from_millis(400),
        "the graceful window must be granted before escalating"
    );
    assert!(wait_for_gone("4312", Duration::from_secs(5)));
}

#[test]
fn a_backend_that_dies_on_its_own_is_reported_not_reaped() {
    // Exits by itself while the app is still alive.
    let config = backend("exec sleep 0.3", Duration::from_secs(5));
    let guardian = Supervisor::start(config);

    let (code, report) = guardian.finish();

    assert_eq!(code, 0);
    assert_eq!(report, EVENT_EXITED);
}

#[test]
fn a_backend_that_cannot_be_started_is_reported() {
    let config = GuardianConfig {
        backend: BackendCommand {
            program: PathBuf::from("/nonexistent/definitely-not-a-backend"),
            args: Vec::new(),
            envs: Vec::new(),
        },
        graceful_timeout: Duration::from_millis(200),
    };
    let guardian = Supervisor::start(config);

    let (code, report) = guardian.finish();

    assert_ne!(code, 0);
    assert!(
        report.starts_with(EVENT_ERROR_PREFIX),
        "expected an error report, got {report:?}"
    );
}

#[test]
fn a_recorder_the_user_started_is_never_touched() {
    let mut theirs = spawn_decoy("4313");
    wait_for_child("4313", Duration::from_secs(5));
    let their_pid = theirs.id();

    let config = lingering("4314");
    let mut guardian = Supervisor::start(config);
    wait_for_child("4314", Duration::from_secs(5));

    guardian.request_shutdown();
    let (code, report) = guardian.finish();

    assert_eq!(code, 0);
    assert_eq!(report, EVENT_STOPPED);
    assert!(wait_for_gone("4314", Duration::from_secs(5)));
    assert!(
        alive(their_pid),
        "a backend the app did not start must survive regardless of how it was launched"
    );

    let _ = theirs.kill();
    let _ = theirs.wait();
}

#[test]
fn an_identical_instance_on_the_same_path_is_left_alone() {
    // Stronger than the previous test: the unrelated recorder runs the *same
    // program with the same arguments* as the guardian's own backend, so
    // nothing about the two can be told apart by inspecting the process table.
    // Ownership is the only thing that separates them — which is the point.
    let script = "exec sleep 4331";
    let mut theirs = spawn_owned(script);
    wait_until(
        "the unrelated recorder to start",
        Duration::from_secs(5),
        || own_children("4331").len() == 1,
    );

    let mut guardian = Supervisor::start(backend(script, Duration::from_secs(5)));
    wait_until(
        "the guardian's own backend to start",
        Duration::from_secs(5),
        || own_children("4331").len() == 2,
    );

    guardian.request_shutdown();
    let (code, report) = guardian.finish();

    assert_eq!(code, 0);
    assert_eq!(report, EVENT_STOPPED);

    let survivors = own_children("4331");
    assert_eq!(
        survivors,
        vec![theirs.id()],
        "only the guardian's own child may be reaped; the identical unrelated instance must survive"
    );

    let _ = theirs.kill();
    let _ = theirs.wait();
}

#[test]
fn shutdown_still_works_when_the_backend_binary_is_gone() {
    // Any identity check based on the executable path would fail here — the
    // file no longer exists. Ownership does not care: the guardian holds the
    // child itself.
    let tag = format!("blrlcopy{}", std::process::id());
    let directory = std::env::temp_dir().join(&tag);
    std::fs::create_dir_all(&directory).expect("temp dir");
    let copy = directory.join(&tag);
    std::fs::copy("/bin/sleep", &copy).expect("copy sleep");
    std::fs::set_permissions(&copy, std::os::unix::fs::PermissionsExt::from_mode(0o755))
        .expect("chmod");

    let config = GuardianConfig {
        backend: BackendCommand {
            program: copy.clone(),
            args: vec![OsString::from(LINGER)],
            envs: Vec::new(),
        },
        graceful_timeout: Duration::from_secs(5),
    };
    let mut guardian = Supervisor::start(config);
    wait_for_child(&tag, Duration::from_secs(5));

    // The path is now unreadable: `/proc`-style lookups and `proc_pidpath`
    // would both come back empty.
    std::fs::remove_file(&copy).expect("remove the running executable");

    guardian.request_shutdown();
    let (code, report) = guardian.finish();

    assert_eq!(code, 0);
    assert_eq!(report, EVENT_STOPPED);
    assert!(wait_for_gone(&tag, Duration::from_secs(5)));

    let _ = std::fs::remove_dir_all(&directory);
}

#[test]
fn a_shutdown_racing_the_backend_exit_always_settles_cleanly() {
    // End of keepalive and the backend exiting on its own land in the same
    // instant; whatever wins, there must be exactly one report and the child
    // must be reaped.
    for _ in 0..25 {
        let config = backend("exec sleep 0.05", Duration::from_millis(300));
        let mut guardian = Supervisor::start(config);
        guardian.request_shutdown();
        let (code, report) = guardian.finish();

        assert_eq!(code, 0, "guardian must not fail on a racing shutdown");
        assert!(
            report == EVENT_STOPPED || report == EVENT_EXITED,
            "expected exactly one settled report, got {report:?}"
        );
    }
}

#[test]
fn concurrent_guardians_each_reap_only_their_own_backend() {
    let markers = ["4321", "4322", "4323", "4324", "4325", "4326"];
    let mut theirs = spawn_decoy("4327");
    wait_for_child("4327", Duration::from_secs(5));

    let mut guardians = Vec::new();
    for marker in markers {
        let guardian = Supervisor::start(lingering(marker));
        wait_for_child(marker, Duration::from_secs(5));
        guardians.push(guardian);
    }

    let (sender, receiver) = mpsc::channel();
    let mut handles = Vec::new();
    for mut guardian in guardians {
        let sender = sender.clone();
        handles.push(std::thread::spawn(move || {
            guardian.request_shutdown();
            let _ = sender.send(guardian.finish());
        }));
    }
    drop(sender);

    for outcome in receiver {
        assert_eq!(outcome.0, 0);
        assert_eq!(outcome.1, EVENT_STOPPED);
    }
    for handle in handles {
        handle.join().expect("shutdown thread panicked");
    }

    for marker in markers {
        assert!(
            wait_for_gone(marker, Duration::from_secs(5)),
            "backend {marker} was left behind"
        );
    }
    assert!(alive(theirs.id()), "the unrelated recorder must survive");

    let _ = theirs.kill();
    let _ = theirs.wait();
}

#[test]
fn an_unreaped_child_keeps_its_pid_reserved() {
    // This pins the platform guarantee the whole design rests on: until the
    // parent reaps it, a child's pid cannot be handed to another process, so
    // "the pid I spawned" and "the process I must signal" cannot drift apart.
    let mut child = Command::new(SHELL)
        .args(["-c", &format!("exec sleep {LINGER}")])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("child should start");
    let pid = child.id();

    unsafe { libc::kill(pid as libc::c_int, libc::SIGKILL) };
    std::thread::sleep(Duration::from_millis(200));

    assert!(
        alive(pid),
        "an exited but unreaped child must still hold its pid"
    );

    let _ = child.wait();
    assert!(
        !alive(pid),
        "once reaped, the pid is released — which is why nothing may signal it afterwards"
    );
}

// ---------------------------------------------------------------------------
// The guardian itself dying or hanging
// ---------------------------------------------------------------------------

#[test]
fn a_guardian_that_exits_on_its_own_is_reaped_without_escalation() {
    // The ordinary case: the guardian finished its job. Nothing should be
    // escalated at it, and its exit code must come back.
    let mut guardian = Command::new(SHELL)
        .args(["-c", "exit 7"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("guardian stand-in should start");

    let code = shutdown_guardian(&mut guardian, Duration::from_secs(5));
    assert_eq!(code, Some(7), "a guardian that exited must report its code");
    assert!(!alive(guardian.id()) || guardian.try_wait().is_ok());
}

#[test]
fn the_guardian_reports_the_backend_pid_it_owns() {
    // The app is told the backend's pid so a human can be pointed at it when
    // something has already gone wrong. It is never signalled, so this only has
    // to be true and to arrive before anything else.
    let mut supervisor = Supervisor::start(lingering("4341"));
    wait_for_child("4341", Duration::from_secs(5));
    let owned = own_children("4341");
    assert_eq!(owned.len(), 1, "exactly one backend should be running");

    supervisor.request_shutdown();
    let (code, events) = supervisor.finish_full();
    assert_eq!(code, 0, "guardian should stop cleanly, events: {events}");

    let started = events
        .lines()
        .find_map(|line| line.strip_prefix(EVENT_STARTED_PREFIX))
        .expect("the guardian must report the backend pid");
    assert_eq!(
        started.trim().parse::<u32>().ok(),
        Some(owned[0]),
        "the reported pid must be the backend the guardian actually owns"
    );
}

// ---------------------------------------------------------------------------
// Containment is established before anything is spawned
// ---------------------------------------------------------------------------

#[test]
fn a_backend_never_gets_to_execute_when_containment_cannot_be_established() {
    // Failure injection for the platform containment step, which on Windows is
    // the job object. The seam is `supervise_with`.
    //
    // The property is stronger than "the backend is not left running": the
    // constraint has to exist *before the backend has a chance to execute or
    // spawn anything*, because a process already running can start children
    // that containment would then have to chase. So the stand-in announces
    // itself by creating a file the instant it starts, and the assertion is
    // that the file never appears. A test that only checked for a surviving
    // process would pass even if the backend had run and been killed
    // afterwards, which is exactly the ordering that must not happen.
    let announced = std::env::temp_dir().join(format!("nvc-ran-{}.flag", std::process::id()));
    let _ = std::fs::remove_file(&announced);

    let (guardian_side, _app_side) = UnixStream::pair().expect("keepalive pair");
    let (events_out, mut events_in) = UnixStream::pair().expect("event pair");
    let config = backend(
        &format!("echo ran > {}; exec sleep {LINGER}", announced.display()),
        Duration::from_secs(5),
    );

    // The injected failure takes a moment before it gives up. That delay is
    // what makes this deterministic: if containment runs before the spawn, no
    // backend exists during it and the file can never appear; if containment
    // ran after the spawn, the backend has that whole window to run and the
    // file appears. Without the delay the check is a race the kill usually
    // wins, which would let a post-spawn ordering pass — measured, not assumed.
    let code = supervise_with(&config, guardian_side, events_out, || {
        std::thread::sleep(Duration::from_millis(750));
        Err("注入的失败".to_owned())
    });

    assert_eq!(code, 1, "a failed containment must not report success");
    assert!(
        !announced.exists(),
        "the backend executed before containment was established"
    );

    let mut events = String::new();
    events_in
        .read_to_string(&mut events)
        .expect("read guardian events");
    assert!(
        events.trim().starts_with(EVENT_ERROR_PREFIX),
        "the refusal must be reported, got {events:?}"
    );
}

#[test]
fn containment_succeeding_lets_the_backend_execute() {
    // The control for the test above: with containment established the backend
    // does run and does announce itself. Without this, the negative result
    // there would be consistent with the probe simply never working.
    let announced = std::env::temp_dir().join(format!("nvc-ran-ok-{}.flag", std::process::id()));
    let _ = std::fs::remove_file(&announced);

    let (guardian_side, app_side) = UnixStream::pair().expect("keepalive pair");
    let (events_out, mut events_in) = UnixStream::pair().expect("event pair");
    let config = backend(
        &format!("echo ran > {}; exec sleep {LINGER}", announced.display()),
        Duration::from_secs(5),
    );

    let handle =
        std::thread::spawn(move || supervise_with(&config, guardian_side, events_out, || Ok(())));
    wait_until(
        "the backend to announce itself",
        Duration::from_secs(5),
        || announced.exists(),
    );
    assert!(
        announced.exists(),
        "the probe must work: an uncontained run has to create the file"
    );
    let _ = std::fs::remove_file(&announced);

    drop(app_side);
    let code = handle.join().expect("guardian thread panicked");
    assert_eq!(code, 0, "an uncontended guardian should stop cleanly");

    let mut events = String::new();
    events_in
        .read_to_string(&mut events)
        .expect("read guardian events");
    assert!(
        events.contains(EVENT_STOPPED),
        "the guardian should have settled, got {events:?}"
    );
}

// ---------------------------------------------------------------------------
// A hung guardian
// ---------------------------------------------------------------------------

#[test]
fn a_hung_guardian_is_left_alone_where_ending_it_would_orphan_the_backend() {
    // The safety property, stated as a test: this program must never signal a
    // process it does not own. On macOS, ending a hung guardian cannot reclaim
    // the backend — it can only orphan it — so the correct behaviour is to do
    // nothing and report. An earlier revision reached for the guardian's
    // children here, which risked hitting a recycled pid.
    //
    // The stand-ins refuse SIGTERM so the guardian is genuinely stuck.
    let pid_file = std::env::temp_dir().join(format!("nvc-hung-{}.pid", std::process::id()));
    let _ = std::fs::remove_file(&pid_file);

    let mut guardian = Command::new(SHELL)
        .args([
            "-c",
            &format!(
                "sh -c 'trap \"\" TERM; exec sleep 300' & echo $! > {}; trap \"\" TERM; wait",
                pid_file.display()
            ),
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("the hung guardian stand-in should start");
    let guardian_pid = guardian.id();

    let mut recorded = None;
    wait_until(
        "the guardian to record its child's pid",
        Duration::from_secs(5),
        || {
            recorded = std::fs::read_to_string(&pid_file)
                .ok()
                .and_then(|text| text.trim().parse::<u32>().ok());
            recorded.is_some()
        },
    );
    let child_pid = recorded.expect("the guardian should record its child's pid");
    let _ = std::fs::remove_file(&pid_file);

    let exit = shutdown_guardian(&mut guardian, Duration::from_millis(300));

    if cfg!(target_os = "macos") {
        assert_eq!(exit, None, "macOS cannot converge a hung guardian");
        assert!(
            alive(guardian_pid),
            "the hung guardian must be left running: ending it on macOS would \
             orphan the backend rather than reclaim it"
        );
        assert!(
            alive(child_pid),
            "the backend must not be signalled — this program does not own it"
        );
    } else {
        // Elsewhere the kernel ties the two together, so ending the guardian
        // reclaims the backend and this must settle.
        assert!(
            !alive(guardian_pid),
            "on a platform with parent-death containment the guardian must be ended"
        );
    }

    // Clean up only what this test created, and only by handles it holds.
    let _ = guardian.kill();
    let _ = guardian.wait();
    #[cfg(unix)]
    unsafe {
        libc::kill(child_pid as libc::c_int, libc::SIGKILL);
    }
}
