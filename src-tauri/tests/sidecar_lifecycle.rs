//! Regression tests for the sidecar lifecycle: the app must always take its own
//! backend down, and must never signal a process it did not start.
//!
//! These exercise the real `guard` entry point the guardian process runs, using
//! ordinary system processes as stand-ins for the bundled CLI.

#![cfg(unix)]

use std::{
    io,
    path::PathBuf,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

use bililive_recorder_gui_lib::{ReapTarget, guard, guard_with_timeout};

const SLEEP: &str = "/bin/sleep";
const OTHER: &str = "/bin/cat";

fn canonical(path: &str) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| PathBuf::from(path))
}

/// Spawns `sh -c <script>` detached from the test's stdio.
///
/// Every script `exec`s its payload so the child PID is the payload itself,
/// exactly like the bundled CLI, rather than whatever shell started it.
fn spawn(script: &str) -> Child {
    Command::new("/bin/sh")
        .arg("-c")
        .arg(script)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("failed to spawn test process")
}

/// Waits for `child` to actually be reaped, which is what makes its PID free.
fn wait_gone(child: &mut Child, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return true,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(25)),
            Ok(None) => return false,
            Err(_) => return false,
        }
    }
}

fn kill_and_reap(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

#[test]
fn guard_stops_the_target_once_its_input_reaches_eof() {
    let mut target = spawn(&format!("exec {SLEEP} 300"));
    let reap = ReapTarget {
        pid: target.id(),
        executable: canonical(SLEEP),
    };

    // `io::empty()` reaches end of file immediately, exactly like the pipe on
    // the GUI's death.
    assert_eq!(guard(&mut io::empty(), &reap), 0);
    assert!(
        wait_gone(&mut target, Duration::from_secs(5)),
        "guard must terminate the backend process"
    );
}

#[test]
fn guard_escalates_when_the_target_ignores_sigint() {
    // `exec` keeps the ignored SIGINT across the exec, so this is a single
    // process that survives the graceful stage.
    let mut target = spawn("trap '' INT; exec /bin/sleep 300");
    let reap = ReapTarget {
        pid: target.id(),
        executable: canonical(SLEEP),
    };

    let started = Instant::now();
    assert_eq!(
        guard_with_timeout(&mut io::empty(), &reap, Duration::from_millis(400)),
        0
    );
    assert!(
        wait_gone(&mut target, Duration::from_secs(5)),
        "guard must escalate to SIGKILL when the backend ignores SIGINT"
    );
    assert!(
        started.elapsed() >= Duration::from_millis(400),
        "the graceful window must actually be granted before escalating"
    );
}

#[test]
fn guard_never_signals_a_pid_it_cannot_identify() {
    let mut decoy = spawn(&format!("exec {SLEEP} 300"));
    let reap = ReapTarget {
        pid: decoy.id(),
        // Same PID, different program: this stands in for a recycled PID.
        executable: canonical(OTHER),
    };

    assert_eq!(
        guard(&mut io::empty(), &reap),
        2,
        "an unverified PID must be reported and left alone"
    );
    std::thread::sleep(Duration::from_millis(300));
    assert!(
        !wait_gone(&mut decoy, Duration::from_millis(50)),
        "guard must not terminate a process it cannot identify"
    );

    kill_and_reap(&mut decoy);
}

#[test]
fn guard_leaves_unrelated_recorders_running() {
    let mut mine = spawn(&format!("exec {SLEEP} 300"));
    let mut theirs = spawn(&format!("exec {SLEEP} 300"));
    let reap = ReapTarget {
        pid: mine.id(),
        executable: canonical(SLEEP),
    };

    assert_eq!(guard(&mut io::empty(), &reap), 0);
    assert!(wait_gone(&mut mine, Duration::from_secs(5)));
    assert!(
        !wait_gone(&mut theirs, Duration::from_millis(50)),
        "a backend the app did not start must never be touched"
    );

    kill_and_reap(&mut theirs);
}

#[test]
fn guard_is_a_no_op_when_the_target_is_already_gone() {
    let mut target = spawn(&format!("exec {SLEEP} 300"));
    let reap = ReapTarget {
        pid: target.id(),
        executable: canonical(SLEEP),
    };
    kill_and_reap(&mut target);

    assert_eq!(guard(&mut io::empty(), &reap), 0);
}

#[test]
fn guard_ignores_rubbish_before_eof() {
    let mut target = spawn(&format!("exec {SLEEP} 300"));
    let reap = ReapTarget {
        pid: target.id(),
        executable: canonical(SLEEP),
    };

    // Data on the pipe must not be mistaken for end of file.
    let mut input: &[u8] = b"still alive";
    assert_eq!(guard(&mut input, &reap), 0);
    assert!(wait_gone(&mut target, Duration::from_secs(5)));
}
