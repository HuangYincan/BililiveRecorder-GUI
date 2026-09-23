//! BililiveRecorder GUI — a Tauri desktop shell around the official
//! BililiveRecorder CLI, whose embedded WebUI is the only user interface.
//!
//! # Backend ownership
//!
//! The desktop app **never handles the backend's process id**. It starts one
//! guardian process and hands it the command line to run; the guardian creates
//! the backend as *its own* child and is the only thing that ever signals or
//! reaps it. Every signal therefore goes through a `std::process::Child` the
//! guardian itself spawned:
//!
//! * The kernel does not recycle a pid while an unreaped child still holds it,
//!   so a recycled-pid mix-up cannot happen — there is no pid to mismatch, and
//!   no window between "check who this pid is" and "signal it".
//! * No code path inspects `/proc`, `proc_pidpath`, process names or process
//!   groups. An instance the user started themselves is unreachable by
//!   construction, at any path, including a second copy of the same binary.
//!
//! # Exit path guarantees
//!
//! | Exit path | Guarantee |
//! | --- | --- |
//! | Main window closed (`CloseRequested`) | the app asks the guardian to stop and waits for it, so the backend is down before the app exits |
//! | App quit — Cmd+Q, Dock ▸ Quit, `osascript quit` | same; on macOS a normal quit only ever delivers `RunEvent::Exit`, never `CloseRequested` |
//! | `SIGTERM` / `SIGHUP` / `SIGINT` to the app | handled explicitly, same shutdown, app exits with `128 + signal` |
//! | App `SIGKILL`ed, crashed, panicked or aborted | the guardian notices the app disappear through a closed pipe and shuts the backend down on its own |
//!
//! Shutdown is two-stage everywhere: `SIGINT` first so the CLI can flush
//! recordings and exit cleanly, escalating to `SIGKILL` only after
//! [`GUARDIAN_GRACEFUL_TIMEOUT`].
//!
//! # Platform support boundary
//!
//! **Only macOS has been measured. No cross-platform guarantee is claimed.**
//!
//! The guardian uses nothing platform-specific — one `Child` plus two pipes —
//! so it builds and ships for Windows and Linux too, and the ownership argument
//! carries over unchanged: Windows likewise will not hand out a pid while a
//! handle to that process object is open, and `Child` holds one until `wait`.
//! But that is reasoning, not measurement. For Windows specifically, none of
//! the following has ever been observed:
//!
//! * whether the app's death reliably closes the write end of the pipe the
//!   guardian blocks on, so that it actually sees end of file — the entire
//!   crash and `SIGKILL` path depends on this;
//! * whether anything else ends up holding a copy of that write end, which
//!   would keep the guardian from ever waking;
//! * how the guardian starts and exits when launched without a console.
//!
//! Windows also cannot be asked politely: a console-less child cannot be sent
//! `Ctrl+C`, so shutdown there is a forced termination that gives the CLI no
//! chance to flush.
//!
//! What *has* been checked is narrower than running it. `platform.rs` — the
//! job object, `PR_SET_PDEATHSIG`, and the two ways of asking the kernel for a
//! process's children — type-checks for all three targets, by way of a minimal
//! crate that avoids tauri's C dependencies (`cargo check` needs no linker, but
//! `aws-lc-sys` wants `windows.h` and `gdk-sys` wants GTK, so the crate as a
//! whole cannot be cross-checked). Type-checking is not running, and none of it
//! has run on Windows or Linux.
//!
//! # When the guardian itself dies or hangs
//!
//! **This program signals exactly one kind of thing: a `Child` it spawned
//! itself and has not reaped**, so the kernel still reserves that pid. It never
//! signals a pid read from anywhere else — not the kernel's child list, not a
//! log, not a report.
//!
//! An earlier revision broke that rule. To converge a hung guardian it read the
//! guardian's children from the kernel and signalled them, arguing that the
//! read and the signal happened in one loop iteration so the list could not be
//! stale. That argument does not hold: those children are not held by this
//! process, so one can exit and be reaped between the read and the signal, and
//! the kernel may hand its pid to an unrelated process — a recorder the user
//! started, which is the exact harm this design exists to prevent. Reading a
//! live parent's child list is not ownership of its children, and re-reading
//! does not make it atomic. That path was deleted outright, not narrowed.
//!
//! What convergence is left turns on one platform fact,
//! [`platform::DESCENDANTS_DIE_WITH_THE_PROCESS`]:
//!
//! | Case | Linux | Windows | macOS |
//! | --- | --- | --- | --- |
//! | app exits normally, or is killed | guardian sees the keepalive pipe close and shuts the backend down itself | same | same |
//! | guardian **killed** | kernel takes the backend with it (`PR_SET_PDEATHSIG`) | kernel takes the backend with it (job object) | **not contained** |
//! | guardian **hung** | the app ends the guardian — a `Child` it owns — and the kernel reclaims the backend | same: ending the guardian closes its job | the app **does nothing** and reports; see below |
//!
//! ## The macOS trade-off, stated as a decision
//!
//! macOS has no `PR_SET_PDEATHSIG` and no job object, so nothing ties the
//! backend's lifetime to the guardian's. If the guardian is killed outright the
//! backend is reparented to `launchd` and the app can neither prove it is that
//! backend nor safely signal it, so it says the backend may still be running
//! and leaves it at that.
//!
//! A *hung* guardian is the harder case, and this is the trade-off being made
//! rather than a gap left by accident. The app can see that the guardian has
//! stopped making progress. It has exactly two options, and only one of them is
//! safe:
//!
//! * **End the guardian** — reclaims nothing on macOS, and *guarantees* the
//!   backend is orphaned. Strictly worse than doing nothing.
//! * **Leave it alone** (what this implements) — the guardian still owns the
//!   backend and will shut it down if it ever resumes; and because the app's
//!   exit closes the keepalive pipe, a guardian that resumes at any point
//!   afterwards shuts the backend down on its own.
//!
//! So on macOS a hung guardian means the backend may keep running until the
//! guardian resumes, and the app says so instead of pretending otherwise. It
//! does **not** reach for the backend's pid to force the issue: a forced
//! recovery that can hit an unrelated process is not worth a prompt shutdown.
//! Closing this properly needs a kernel-side reaper that adopts orphans (a
//! launchd agent or a system extension) — out of proportion to this bug, and a
//! signing and notarisation burden — so it is left as a decision for the
//! maintainers rather than assumed.
//!
//! ## What the user is told, and what they are not
//!
//! The maintainers accepted this limitation on the terms that it is described
//! accurately and that nobody is pointed at a process to kill. Both are
//! structural here rather than a matter of wording discipline:
//!
//! * [`UNRECLAIMED_BACKEND_MESSAGE`] takes **no arguments**, so it cannot name
//!   a process id even by accident. The pid the guardian reported goes to
//!   stderr, where it is evidence for a log rather than an instruction.
//! * The message says the backend *may* still be running and that the app
//!   cannot confirm otherwise. It never claims the abnormal paths all clean up
//!   after themselves — on macOS this one demonstrably does not.
//! * It tells the user **not** to terminate by process id, because a pid read
//!   from a pipe at startup may by now name an unrelated process. That is the
//!   same mistake this design exists to prevent; it would be perverse to
//!   automate the avoidance and then recommend it in a dialog.
//!
//! ## Windows containment is established before the backend exists
//!
//! The job object is created at guardian startup and the **guardian itself** is
//! assigned to it, before any spawn. A Windows job is inherited by child
//! processes, so the backend — and anything the backend starts — joins it at
//! creation. There is therefore no window in which the backend runs
//! uncontained, and no process is adopted into the job after it has had a
//! chance to execute or spawn. If the job cannot be established the guardian
//! exits non-zero **without spawning anything**: nothing has been created, so
//! there is no created process to converge, and the handles involved are
//! closed on the failure path.
//!

use std::{
    ffi::OsString,
    fs,
    io::{self, BufRead, BufReader, Read, Write},
    net::TcpListener,
    path::PathBuf,
    process::{Child, ChildStdout, Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicI32, AtomicU32, Ordering},
    },
    time::{Duration, Instant},
};

use tauri::{Manager, WebviewUrl, WebviewWindowBuilder};
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};
use tauri_plugin_updater::UpdaterExt;

mod platform;

const SIDECAR_NAME: &str = "BililiveRecorder.Cli";

/// Hidden argv flag that re-runs this same executable as the backend guardian.
pub const GUARDIAN_FLAG: &str = "--bililive-sidecar-guardian";

/// How long the backend may take to shut down gracefully before it is killed.
pub const GUARDIAN_GRACEFUL_TIMEOUT: Duration = Duration::from_secs(8);
/// Extra time the app gives the guardian to finish after that.
const GUARDIAN_EXIT_MARGIN: Duration = Duration::from_secs(4);
/// How often the guardian and the app re-check the state they are watching.
const POLL_INTERVAL: Duration = Duration::from_millis(50);
/// Timeout applied when the guardian is asked for its version by a human.
const DEFAULT_GRACEFUL_TIMEOUT: Duration = GUARDIAN_GRACEFUL_TIMEOUT;

/// One line the guardian writes to the app when it is done.
pub const EVENT_STOPPED: &str = "stopped";
/// The backend exited by itself, without anyone asking it to.
pub const EVENT_EXITED: &str = "exited";
/// The guardian could not do its job; the rest of the line says why.
pub const EVENT_ERROR_PREFIX: &str = "error: ";
/// Followed by the backend's pid, reported once at startup.
///
/// **Diagnostics only.** The app never signals a pid it read from a pipe; the
/// guardian remains the only thing that touches the backend. This exists so a
/// human can be told which process to look at when something has already gone
/// wrong, and so a support log says what the app was actually running.
pub const EVENT_STARTED_PREFIX: &str = "started ";

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
const TARGET_TRIPLE: &str = "aarch64-apple-darwin";
#[cfg(all(target_os = "macos", target_arch = "x86_64"))]
const TARGET_TRIPLE: &str = "x86_64-apple-darwin";
#[cfg(all(target_os = "windows", target_arch = "x86_64"))]
const TARGET_TRIPLE: &str = "x86_64-pc-windows-msvc";
#[cfg(all(target_os = "linux", target_arch = "x86_64", target_env = "gnu"))]
const TARGET_TRIPLE: &str = "x86_64-unknown-linux-gnu";
#[cfg(all(target_os = "linux", target_arch = "aarch64", target_env = "gnu"))]
const TARGET_TRIPLE: &str = "aarch64-unknown-linux-gnu";
#[cfg(all(target_os = "linux", target_arch = "x86_64", target_env = "musl"))]
const TARGET_TRIPLE: &str = "x86_64-unknown-linux-musl";
#[cfg(all(target_os = "linux", target_arch = "aarch64", target_env = "musl"))]
const TARGET_TRIPLE: &str = "aarch64-unknown-linux-musl";

// ---------------------------------------------------------------------------
// What the guardian is asked to run
// ---------------------------------------------------------------------------

/// The backend command line the guardian will own.
///
/// Deliberately contains no process id: the guardian learns the backend's pid
/// only from the `Child` it creates itself, which is what makes every signal it
/// sends exact.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BackendCommand {
    pub program: PathBuf,
    pub args: Vec<OsString>,
    pub envs: Vec<(OsString, OsString)>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GuardianConfig {
    pub backend: BackendCommand,
    pub graceful_timeout: Duration,
}

impl GuardianConfig {
    /// Parses `<exe> GUARDIAN_FLAG --program P [--backend-arg A]…
    /// [--backend-env K V]… [--graceful-timeout-ms N]`, or `None` when this is
    /// an ordinary launch of the app.
    pub fn from_args<I, S>(args: I) -> Option<Self>
    where
        I: IntoIterator<Item = S>,
        S: Into<OsString>,
    {
        let mut args = args.into_iter().map(Into::into);
        let _executable = args.next()?;
        if args.next()? != GUARDIAN_FLAG {
            return None;
        }

        let mut program: Option<PathBuf> = None;
        let mut backend_args = Vec::new();
        let mut envs = Vec::new();
        let mut graceful_timeout = DEFAULT_GRACEFUL_TIMEOUT;

        while let Some(flag) = args.next() {
            match flag.to_str()? {
                "--program" => program = Some(PathBuf::from(args.next()?)),
                "--backend-arg" => backend_args.push(args.next()?),
                "--backend-env" => {
                    let key = args.next()?;
                    let value = args.next()?;
                    envs.push((key, value));
                }
                "--graceful-timeout-ms" => {
                    let millis: u64 = args.next()?.to_str()?.parse().ok()?;
                    graceful_timeout = Duration::from_millis(millis);
                }
                _ => return None,
            }
        }

        Some(Self {
            backend: BackendCommand {
                program: program?,
                args: backend_args,
                envs,
            },
            graceful_timeout,
        })
    }

    /// The argv the app passes to its own binary to start this guardian.
    pub fn to_args(&self) -> Vec<OsString> {
        let mut args = vec![
            OsString::from(GUARDIAN_FLAG),
            OsString::from("--program"),
            self.backend.program.clone().into_os_string(),
        ];
        for arg in &self.backend.args {
            args.push(OsString::from("--backend-arg"));
            args.push(arg.clone());
        }
        for (key, value) in &self.backend.envs {
            args.push(OsString::from("--backend-env"));
            args.push(key.clone());
            args.push(value.clone());
        }
        args.push(OsString::from("--graceful-timeout-ms"));
        args.push(OsString::from(
            self.graceful_timeout.as_millis().to_string(),
        ));
        args
    }
}

// ---------------------------------------------------------------------------
// Guardian
// ---------------------------------------------------------------------------

/// Asks a backend to exit cleanly.
///
/// `pid` always comes from a `Child` this process created and has not reaped,
/// so the kernel still reserves it for that child and cannot have handed it to
/// anything else.
fn send_interrupt(pid: u32) -> bool {
    #[cfg(unix)]
    {
        unsafe { libc::kill(pid as libc::c_int, libc::SIGINT) == 0 }
    }

    #[cfg(not(unix))]
    {
        // A console-less Windows child cannot be sent Ctrl+C at all.
        let _ = pid;
        false
    }
}

/// What a bounded wait on a child actually settled.
///
/// Three outcomes, not two. A wait that *failed* is not a wait that *timed
/// out*: a timeout says the child is still running and still unreaped, so its
/// pid is provably this process's to signal; a failure says nothing at all
/// about the child. Collapsing the two is how a pid stops being owned — an
/// error from [`Child::try_wait`] is exactly the case where the child may
/// already have been reaped out from under us and its pid handed to an
/// unrelated process, so it must never be signalled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaitOutcome {
    /// Reaped, and this is the code it exited with.
    Exited(i32),
    /// Still running when the deadline passed: unreaped, therefore still ours.
    TimedOut,
    /// The wait itself failed. The child's state is unknown, so ownership is
    /// not established and no signal may be sent to its pid.
    Unwaitable,
}

impl WaitOutcome {
    /// Whether this outcome leaves the child's pid provably this process's own.
    ///
    /// Only a timeout does. An exit has already released the pid, and a failed
    /// wait establishes nothing about it.
    pub fn pid_is_still_ours(self) -> bool {
        matches!(self, WaitOutcome::TimedOut)
    }
}

/// Polls `child` until it exits or `within` elapses, keeping the three outcomes
/// distinct.
///
/// The `Err` arm is deliberately matched *before* the deadline check: it is the
/// one result that must never be read as "still running", and a timeout that
/// had already been reached is not a reason to reinterpret a failed wait.
pub fn wait_for_exit(child: &mut Child, within: Duration) -> WaitOutcome {
    let deadline = Instant::now() + within;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return WaitOutcome::Exited(status.code().unwrap_or(-1)),
            Err(_) => return WaitOutcome::Unwaitable,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(POLL_INTERVAL),
            Ok(None) => return WaitOutcome::TimedOut,
        }
    }
}

/// Ends the guardian, if this platform can do so safely.
///
/// **Every signal this program sends goes to a `Child` it spawned itself and
/// has not reaped**, so the kernel still reserves that pid for it. Nothing here
/// signals a pid that was read from anywhere — not the kernel's child list, not
/// a log, not a report. An earlier revision did signal the guardian's children
/// after reading them from the kernel, on the grounds that the read and the
/// signal happened in the same loop iteration. That argument is wrong: the
/// guardian's *children* are not held by this process, so one of them can exit
/// and be reaped between the read and the signal, and the kernel may then hand
/// its pid to an unrelated process. Reading a live parent's child list is not
/// ownership of those children, and no amount of re-reading makes it atomic.
/// That path is gone; nothing replaced it.
///
/// What remains is the guardian itself, which this app does own. Whether it can
/// be ended turns on one platform fact, [`platform::DESCENDANTS_DIE_WITH_THE_PROCESS`]:
///
/// * **Linux and Windows** — the kernel ties the backend to the guardian, so
///   ending the guardian reclaims the backend too. Signalling the guardian is
///   both sufficient and exact.
/// * **macOS** — nothing ties them together, so ending a hung guardian would
///   orphan the backend with no way back. Leaving both alone is strictly
///   better: the guardian still owns the backend and will shut it down if it
///   ever resumes, whereas killing it guarantees the backend is stranded. So on
///   macOS this reports that it could not converge and deliberately does
///   nothing.
///
/// Returns the guardian's exit code when it could be reaped, `None` when it
/// could not be ended safely.
pub fn shutdown_guardian(guardian: &mut Child, patience: Duration) -> Option<i32> {
    // The ordinary case: the guardian shut the backend down and finished.
    match wait_for_exit(guardian, patience) {
        WaitOutcome::Exited(code) => return Some(code),
        // The wait failed, so we cannot say the guardian is still running — and
        // a pid we cannot claim is the one thing this module refuses to signal.
        // There is nothing to escalate to either: `guardian.kill()` would reach
        // the same unverified pid by a different route.
        WaitOutcome::Unwaitable => return None,
        WaitOutcome::TimedOut => {}
    }

    // Hung, not slow. On macOS this is where convergence stops: killing the
    // guardian here would strand the backend rather than reclaim it, and the
    // app will not trade a stranded recording for the appearance of a clean
    // shutdown. See the module docs for what that costs.
    if !platform::DESCENDANTS_DIE_WITH_THE_PROCESS {
        return None;
    }

    // Elsewhere the kernel will take the backend down with the guardian, and
    // the wait above confirmed the guardian is alive and unreaped, so this pid
    // is exact rather than remembered.
    #[cfg(unix)]
    unsafe {
        libc::kill(guardian.id() as libc::c_int, libc::SIGTERM);
    }

    match wait_for_exit(guardian, GUARDIAN_EXIT_MARGIN) {
        WaitOutcome::Exited(code) => return Some(code),
        // Same rule for the escalation: a failed wait leaves nothing to claim.
        WaitOutcome::Unwaitable => return None,
        WaitOutcome::TimedOut => {}
    }

    // Last resort. The wait just before this proved the guardian is still
    // unreaped, so `kill` reaches the same pid this `Child` still holds.
    let _ = guardian.kill();
    guardian
        .wait()
        .ok()
        .map(|status| status.code().unwrap_or(-1))
}

/// Walks the guardian's own child through the two-stage shutdown.
fn shutdown_child(child: &mut Child, graceful_timeout: Duration) {
    // No wait has happened yet, so this child is still unreaped and its pid is
    // unambiguously ours — including on the platforms where `send_interrupt`
    // declines and we fall straight through to `kill`.
    if send_interrupt(child.id()) {
        match wait_for_exit(child, graceful_timeout) {
            // Reaped: the pid is free, and nothing below may signal it again.
            WaitOutcome::Exited(_) => return,
            // The wait failed, so ownership of this pid is no longer
            // established. Escalating to `child.kill()` would signal it anyway
            // on the strength of having spawned it earlier — the same stale-pid
            // hazard through a different doorway. Where the kernel ties the
            // backend to this process it is the kernel that reclaims this one;
            // on macOS nothing does, which is the documented limit.
            WaitOutcome::Unwaitable => return,
            // Confirmed still running and unreaped, so escalation is exact.
            WaitOutcome::TimedOut => {}
        }
    }
    let _ = child.kill();
    let _ = child.wait();
}

fn write_event<W: Write>(events: &mut W, line: &str) {
    let _ = writeln!(events, "{line}");
    let _ = events.flush();
}

/// Owns `config.backend` for as long as this process lives.
///
/// Returns after the backend has been reaped, having written exactly one line
/// to `events`: [`EVENT_STOPPED`], [`EVENT_EXITED`] or an [`EVENT_ERROR_PREFIX`]
/// message. `keepalive` reaching end of file means the app is gone.
pub fn supervise<R, W>(config: &GuardianConfig, keepalive: R, events: W) -> i32
where
    R: Read + Send + 'static,
    W: Write,
{
    supervise_with(config, keepalive, events, platform::contain_guardian)
}

/// [`supervise`], with the platform containment step injected.
///
/// The seam exists so the failure branch can be exercised on any platform, not
/// only the one that has a job object. It runs **before** the backend is
/// spawned, so a failure means nothing was created.
pub fn supervise_with<R, W, C>(
    config: &GuardianConfig,
    mut keepalive: R,
    mut events: W,
    contain: C,
) -> i32
where
    R: Read + Send + 'static,
    W: Write,
    C: FnOnce() -> Result<(), String>,
{
    // Before anything is spawned. A platform whose containment cannot be
    // established must not run a backend it cannot promise to reclaim, and at
    // this point there is nothing to clean up.
    if let Err(error) = contain() {
        write_event(
            &mut events,
            &format!("{EVENT_ERROR_PREFIX}无法为录播后端建立进程约束，已拒绝启动：{error}"),
        );
        return 1;
    }

    let mut command = Command::new(&config.backend.program);
    command
        .args(&config.backend.args)
        .envs(config.backend.envs.iter().cloned())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if let Some(directory) = config.backend.program.parent() {
        command.current_dir(directory);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }

    // Linux: the kernel kills the backend when the guardian dies, even if the
    // guardian is SIGKILLed and never runs another line of its own code.
    platform::arm_parent_death(&mut command);

    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            write_event(
                &mut events,
                &format!("{EVENT_ERROR_PREFIX}无法启动录播后端：{error}"),
            );
            return 1;
        }
    };

    // Diagnostics only. The app must never signal a pid it read from a pipe.
    write_event(
        &mut events,
        &format!("{EVENT_STARTED_PREFIX}{}", child.id()),
    );

    // End of file on the keepalive pipe is the only signal that also arrives
    // when the app was killed outright, so it is watched from its own thread.
    let app_gone = Arc::new(AtomicBool::new(false));
    {
        let app_gone = Arc::clone(&app_gone);
        std::thread::spawn(move || {
            let mut scratch = [0u8; 64];
            loop {
                match keepalive.read(&mut scratch) {
                    Ok(0) => break,
                    Ok(_) => continue,
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                    Err(_) => break,
                }
            }
            app_gone.store(true, Ordering::SeqCst);
        });
    }

    loop {
        if app_gone.load(Ordering::SeqCst) {
            shutdown_child(&mut child, config.graceful_timeout);
            write_event(&mut events, EVENT_STOPPED);
            return 0;
        }
        match child.try_wait() {
            Ok(Some(_)) => {
                write_event(&mut events, EVENT_EXITED);
                return 0;
            }
            Ok(None) => {}
            Err(error) => {
                write_event(
                    &mut events,
                    &format!("{EVENT_ERROR_PREFIX}无法监视录播后端：{error}"),
                );
                let _ = child.kill();
                let _ = child.wait();
                return 1;
            }
        }
        std::thread::sleep(POLL_INTERVAL);
    }
}

/// Parses the guardian configuration out of this process's real arguments.
pub fn guardian_config() -> Option<GuardianConfig> {
    GuardianConfig::from_args(std::env::args_os())
}

/// Entry point used when the binary is re-run with [`GUARDIAN_FLAG`].
pub fn run_guardian(config: &GuardianConfig) -> i32 {
    supervise(config, io::stdin(), io::stdout())
}

// ---------------------------------------------------------------------------
// App side
// ---------------------------------------------------------------------------

#[derive(Default)]
struct BackendState {
    /// The guardian process. Holding it keeps the keepalive pipe open; closing
    /// that pipe is the only way the app asks for a shutdown.
    guardian: Mutex<Option<Child>>,
    stopping: AtomicBool,
    /// The backend pid the guardian reported. **Never signalled** — kept only
    /// so that a failure can tell a human which process to look at.
    backend_pid: AtomicU32,
}

fn reserve_local_port() -> Result<u16, String> {
    // Test seam. The artifact lifecycle test needs to know where to look
    // without enumerating processes — which is exactly the thing that must not
    // be done on a shared machine. An unusable value is ignored, not fatal.
    if let Some(port) = std::env::var_os("BILILIVE_GUI_BIND_PORT")
        .and_then(|raw| raw.to_str().and_then(|raw| raw.parse::<u16>().ok()))
        && port != 0
        && TcpListener::bind(("127.0.0.1", port)).is_ok()
    {
        return Ok(port);
    }

    TcpListener::bind(("127.0.0.1", 0))
        .and_then(|listener| listener.local_addr())
        .map(|address| address.port())
        .map_err(|error| format!("无法分配本地端口：{error}"))
}

fn sidecar_path(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    let executable = if cfg!(windows) {
        format!("{SIDECAR_NAME}.exe")
    } else {
        SIDECAR_NAME.to_string()
    };
    app.path()
        .resource_dir()
        .map(|directory| {
            directory
                .join("sidecar")
                .join(TARGET_TRIPLE)
                .join(executable)
        })
        .map_err(|error| error.to_string())
}

fn work_directory(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    if let Some(path) = std::env::var_os("BILILIVE_RECORDER_GUI_WORKDIR") {
        return Ok(PathBuf::from(path));
    }

    app.path()
        .video_dir()
        .or_else(|_| app.path().app_data_dir())
        .map(|directory| directory.join("BililiveRecorder"))
        .map_err(|error| error.to_string())
}

fn spawn_guardian(config: &GuardianConfig) -> Result<Child, String> {
    let executable = std::env::current_exe().map_err(|error| error.to_string())?;
    let mut command = Command::new(executable);
    command
        .args(config.to_args())
        // Piped and never written to: the app holds the write end so that the
        // pipe closes exactly when this process dies. The guardian reports back
        // on its stdout.
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    command.spawn().map_err(|error| error.to_string())
}

/// Shown when the backend may still be running and could not be reclaimed.
///
/// Deliberately takes no arguments, so it **cannot** name a process id. The pid
/// the guardian reported is a snapshot from startup; by the time this is shown
/// it may have been recycled onto an unrelated process, and telling a user to
/// stop "process 12345" would be inviting the very mistake this design exists
/// to prevent. The pid goes to stderr for diagnosis instead, where it is
/// evidence rather than an instruction.
///
/// The wording is also careful not to promise that every abnormal path cleans
/// itself up: it says the backend may still be running, because on this
/// platform nothing here can prove otherwise.
pub const UNRECLAIMED_BACKEND_MESSAGE: &str = "录播后端可能仍在运行：本应用无法安全确认它已停止，也不会向无法确认归属的进程发送信号。\
     请勿按进程号手动终止任何进程，那样可能误伤其他程序。可以退出并重新启动本应用，\
     或在应用日志中查看详情。";

fn report_fatal(app: &tauri::AppHandle, message: &str) {
    app.dialog()
        .message(message)
        .title("BililiveRecorder")
        .kind(MessageDialogKind::Error)
        .blocking_show();
}

/// Reports a problem without holding the caller.
///
/// Used on the exit path. A modal dialog there would block the main thread
/// while the run loop is already ending, which can stop the app from exiting at
/// all — trading "the backend may outlive the app" for "the app never quits".
/// That is a worse outcome than the one being reported, so the message goes to
/// stderr as well, where it survives even if no dialog is ever shown.
fn report_warning(app: &tauri::AppHandle, message: &str) {
    eprintln!("bililive-recorder-gui: {message}");
    app.dialog()
        .message(message)
        .title("BililiveRecorder")
        .kind(MessageDialogKind::Error)
        .show(|_| {});
}

/// Watches the guardian's event pipe.
///
/// The guardian writes exactly one line before exiting, so end of file without
/// a line means the guardian itself died — the one case where nothing is left
/// holding the backend.
fn watch_guardian(app: tauri::AppHandle, events: ChildStdout) {
    std::thread::spawn(move || {
        for line in BufReader::new(events).lines().map_while(Result::ok) {
            let line = line.trim().to_owned();
            if line == EVENT_STOPPED {
                return;
            }
            if app.state::<BackendState>().stopping.load(Ordering::SeqCst) {
                return;
            }
            if let Some(pid) = line.strip_prefix(EVENT_STARTED_PREFIX) {
                if let Ok(pid) = pid.trim().parse() {
                    app.state::<BackendState>()
                        .backend_pid
                        .store(pid, Ordering::SeqCst);
                }
                continue;
            }
            if line == EVENT_EXITED {
                report_fatal(&app, "录播后端意外退出，请查看应用日志。");
            } else if let Some(detail) = line.strip_prefix(EVENT_ERROR_PREFIX) {
                report_fatal(&app, detail);
            } else {
                report_fatal(&app, "录播后端守护进程返回了无法识别的状态。");
            }
            app.exit(1);
            return;
        }

        if !app.state::<BackendState>().stopping.load(Ordering::SeqCst) {
            // The guardian is gone without having shut the backend down, and on
            // macOS the kernel cannot take the backend with it. The pid the
            // guardian reported goes to the log for diagnosis; it is
            // deliberately kept out of what the user is told, because a pid
            // read from a pipe is a snapshot — by now it may name a different
            // process, and pointing someone at it invites exactly the mistake
            // this whole design exists to prevent.
            let pid = app
                .state::<BackendState>()
                .backend_pid
                .load(Ordering::SeqCst);
            if pid != 0 {
                eprintln!("bililive-recorder-gui: backend pid reported by the guardian was {pid}");
            }
            report_fatal(&app, UNRECLAIMED_BACKEND_MESSAGE);
            app.exit(1);
        }
    });
}

/// Asks the guardian to shut the backend down and waits for it to finish.
///
/// The guardian owns the backend, so this only closes the keepalive pipe. The
/// guardian is deliberately *not* killed if it overruns: killing it would leave
/// the backend with no owner at all. It finishes on its own instead.
fn stop_backend(app: &tauri::AppHandle) {
    let state = app.state::<BackendState>();
    state.stopping.store(true, Ordering::SeqCst);
    let guardian = state
        .guardian
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .take();

    let Some(mut guardian) = guardian else {
        return;
    };
    drop(guardian.stdin.take());

    // The guardian now shuts the backend down and reaps it. If it does not
    // finish in time it is hung rather than slow; what can be done about that
    // depends on the platform, and `shutdown_guardian` says so.
    let exit = shutdown_guardian(
        &mut guardian,
        GUARDIAN_GRACEFUL_TIMEOUT + GUARDIAN_EXIT_MARGIN,
    );

    // Nothing to converge, and the guardian is still there: on this platform
    // ending it would orphan the backend rather than reclaim it. Say so — the
    // user has to know the backend may outlive the app, because nothing here is
    // going to stop it. `try_wait` on a live child is read-only.
    if exit.is_none()
        && !platform::DESCENDANTS_DIE_WITH_THE_PROCESS
        && matches!(guardian.try_wait(), Ok(None))
    {
        let pid = app
            .state::<BackendState>()
            .backend_pid
            .load(Ordering::SeqCst);
        if pid != 0 {
            eprintln!("bililive-recorder-gui: backend pid reported by the guardian was {pid}");
        }
        report_warning(app, UNRECLAIMED_BACKEND_MESSAGE);
    }
}

async fn start_backend(app: &tauri::AppHandle) -> Result<String, String> {
    let executable = sidecar_path(app)?;
    if !executable.is_file() {
        return Err(format!("找不到录播后端：{}", executable.display()));
    }

    let workdir = work_directory(app)?;
    fs::create_dir_all(&workdir).map_err(|error| format!("无法创建录制目录：{error}"))?;

    let log_directory = app
        .path()
        .app_log_dir()
        .map_err(|error| error.to_string())?;
    fs::create_dir_all(&log_directory).map_err(|error| error.to_string())?;
    let port = reserve_local_port()?;
    let url = format!("http://127.0.0.1:{port}");

    let config = GuardianConfig {
        backend: BackendCommand {
            program: executable,
            args: vec![
                OsString::from("run"),
                OsString::from("--bind"),
                OsString::from(&url),
                workdir.into_os_string(),
            ],
            envs: vec![(
                OsString::from("BILILIVERECORDER_LOG_FILE_PATH"),
                log_directory.join("bililive-recorder.txt").into_os_string(),
            )],
        },
        graceful_timeout: GUARDIAN_GRACEFUL_TIMEOUT,
    };

    let mut guardian = spawn_guardian(&config)?;
    let events = guardian
        .stdout
        .take()
        .ok_or("无法读取录播后端守护进程的状态。")?;
    watch_guardian(app.clone(), events);

    {
        let state = app.state::<BackendState>();
        state.stopping.store(false, Ordering::SeqCst);
        *state
            .guardian
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(guardian);
    }

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(2))
        .build()
        .map_err(|error| error.to_string())?;
    for _ in 0..120 {
        if app.state::<BackendState>().stopping.load(Ordering::SeqCst) {
            // The guardian already reported a failure and the app is exiting.
            return Err("录播后端未能启动。".into());
        }
        if client
            .get(format!("{url}/api/version"))
            .send()
            .await
            .is_ok_and(|response| response.status().is_success())
        {
            return Ok(url);
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }

    stop_backend(app);
    Err("录播后端未能在 30 秒内就绪。".into())
}

async fn check_for_updates(app: tauri::AppHandle) {
    tokio::time::sleep(Duration::from_secs(2)).await;
    let Ok(updater) = app.updater() else {
        return;
    };
    let Ok(Some(update)) = updater.check().await else {
        return;
    };

    let accepted = app
        .dialog()
        .message(format!(
            "发现 BililiveRecorder GUI {}。更新包包含对应版本的官方 CLI 与内嵌 WebUI。",
            update.version
        ))
        .title("发现更新")
        .buttons(MessageDialogButtons::OkCancelCustom(
            "安装更新".into(),
            "稍后".into(),
        ))
        .blocking_show();
    if !accepted {
        return;
    }

    match update.download_and_install(|_, _| {}, || {}).await {
        Ok(()) => {
            // Hand the backend over before restarting so two CLI instances
            // never overlap.
            stop_backend(&app);
            app.restart();
        }
        Err(error) => {
            report_fatal(&app, &format!("更新安装失败：{error}"));
        }
    }
}

async fn launch_main_window(app: tauri::AppHandle) -> Result<(), String> {
    let base_url = start_backend(&app).await?;
    let webui_url =
        url::Url::parse(&format!("{base_url}/ui/")).map_err(|error| error.to_string())?;
    let window = WebviewWindowBuilder::new(&app, "main", WebviewUrl::External(webui_url.clone()))
        .title("BililiveRecorder")
        .inner_size(1280.0, 820.0)
        .min_inner_size(800.0, 560.0)
        .center()
        .visible(false)
        .build()
        .map_err(|error| error.to_string())?;

    tokio::time::sleep(Duration::from_millis(400)).await;
    window
        .navigate(webui_url)
        .map_err(|error| error.to_string())?;
    tokio::time::sleep(Duration::from_millis(800)).await;
    window.show().map_err(|error| error.to_string())?;
    window.set_focus().map_err(|error| error.to_string())?;

    tauri::async_runtime::spawn(check_for_updates(app));
    Ok(())
}

// ---------------------------------------------------------------------------
// Termination signals
//
// A signal handler may only touch async-signal-safe state, so it records the
// number and a watcher thread performs the actual shutdown.
// ---------------------------------------------------------------------------

static PENDING_SIGNAL: AtomicI32 = AtomicI32::new(0);

#[cfg(unix)]
extern "C" fn record_signal(signal: libc::c_int) {
    PENDING_SIGNAL.store(signal, Ordering::SeqCst);
}

#[cfg(unix)]
fn install_signal_handlers() {
    for signal in [libc::SIGTERM, libc::SIGHUP, libc::SIGINT] {
        unsafe {
            libc::signal(signal, record_signal as *const () as libc::sighandler_t);
        }
    }
}

#[cfg(not(unix))]
fn install_signal_handlers() {}

#[cfg(unix)]
fn watch_for_signals(app: tauri::AppHandle) {
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(POLL_INTERVAL);
            let signal = PENDING_SIGNAL.load(Ordering::SeqCst);
            if signal != 0 {
                stop_backend(&app);
                std::process::exit(128 + signal);
            }
        }
    });
}

#[cfg(not(unix))]
fn watch_for_signals(_app: tauri::AppHandle) {}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    install_signal_handlers();

    let app = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .manage(BackendState::default())
        .setup(|app| {
            let handle = app.handle().clone();
            watch_for_signals(handle.clone());
            tauri::async_runtime::spawn(async move {
                if let Err(error) = launch_main_window(handle.clone()).await {
                    report_fatal(&handle, &error);
                    handle.exit(1);
                }
            });
            Ok(())
        })
        .on_window_event(|window, event| {
            if window.label() == "main"
                && let tauri::WindowEvent::CloseRequested { api, .. } = event
            {
                api.prevent_close();
                // Reaping here, before the window goes away, keeps the backend
                // from outliving its UI. `RunEvent::Exit` runs the same
                // idempotent shutdown again and finds nothing left to do.
                stop_backend(window.app_handle());
                let _ = window.hide();
                window.app_handle().exit(0);
            }
        })
        .build(tauri::generate_context!())
        .expect("error while building BililiveRecorder GUI");

    app.run(|app, event| {
        // `Exit` is the event a normal macOS quit delivers (`applicationWillTerminate`
        // -> `LoopDestroyed`); `ExitRequested` covers closes and `exit()` calls.
        // Both are handled because either can be the last one to run.
        if matches!(
            event,
            tauri::RunEvent::ExitRequested { .. } | tauri::RunEvent::Exit
        ) {
            stop_backend(app);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Option<GuardianConfig> {
        GuardianConfig::from_args(args.iter().copied())
    }

    #[test]
    fn only_a_timeout_leaves_a_pid_we_may_still_signal() {
        // The distinction the shutdown paths turn on: an exit has released the
        // pid, and a failed wait establishes nothing about it, so the only
        // outcome that licenses a signal is a confirmed-still-running child.
        assert!(WaitOutcome::TimedOut.pid_is_still_ours());
        assert!(!WaitOutcome::Exited(0).pid_is_still_ours());
        assert!(!WaitOutcome::Exited(-1).pid_is_still_ours());
        assert!(!WaitOutcome::Unwaitable.pid_is_still_ours());
    }

    /// Starts a stand-in backend that ignores the interrupt, and returns only
    /// once it is *known* to be ignoring it.
    ///
    /// The marker matters: a `trap` installed by the child races any signal the
    /// parent sends, and a child killed by the interrupt before it reaches the
    /// `trap` would die looking exactly like one ended by the escalation. The
    /// test below has to tell those apart, so it waits for the child to say it
    /// is ready.
    fn stubborn_child(ready: &std::path::Path) -> Child {
        // The path is quoted: it is interpolated into a shell command, and an
        // unquoted metacharacter in it would be a syntax error rather than a
        // failure the test could explain.
        let script = format!("trap '' INT; : > '{}'; exec sleep 300", ready.display());
        let child = Command::new("/bin/sh")
            .args(["-c", &script])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("stand-in backend should start");

        let deadline = Instant::now() + Duration::from_secs(5);
        while !ready.exists() {
            assert!(
                Instant::now() < deadline,
                "the stand-in never installed its interrupt handler"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        child
    }

    #[test]
    fn a_real_timeout_still_escalates_the_child() {
        use std::os::unix::process::ExitStatusExt;

        // Keeping timeout distinct from failure must not soften the timeout
        // path: a child that ignored the interrupt and outlived the grace
        // period is still confirmed ours, and is still ended — by the
        // escalation, not by the interrupt it already refused.
        let ready = std::env::temp_dir().join(format!(
            "nvc-stubborn-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("the clock is after the epoch")
                .as_nanos()
        ));
        let _ = std::fs::remove_file(&ready);
        let mut child = stubborn_child(&ready);

        shutdown_child(&mut child, Duration::from_millis(400));

        let status = child
            .try_wait()
            .expect("the child can still be waited on")
            .expect("the child must not survive the escalation");
        assert_eq!(
            status.signal(),
            Some(libc::SIGKILL),
            "the child should have been escalated to, not stopped by the interrupt"
        );
        let _ = std::fs::remove_file(&ready);
    }

    #[test]
    fn parses_a_guardian_invocation() {
        let config = parse(&[
            "/Applications/BililiveRecorder GUI.app/Contents/MacOS/bililive-recorder-gui",
            GUARDIAN_FLAG,
            "--program",
            "/Applications/BililiveRecorder GUI.app/Contents/Resources/sidecar/BililiveRecorder.Cli",
            "--backend-arg",
            "run",
            "--backend-arg",
            "--bind",
            "--backend-arg",
            "http://127.0.0.1:1234",
            "--backend-env",
            "BILILIVERECORDER_LOG_FILE_PATH",
            "/tmp/log.txt",
            "--graceful-timeout-ms",
            "8000",
        ])
        .expect("a guardian invocation should parse");

        assert_eq!(config.backend.args.len(), 3);
        assert_eq!(config.backend.args[0], OsString::from("run"));
        assert_eq!(
            config.backend.envs,
            vec![(
                OsString::from("BILILIVERECORDER_LOG_FILE_PATH"),
                OsString::from("/tmp/log.txt")
            )]
        );
        assert_eq!(config.graceful_timeout, Duration::from_secs(8));
    }

    #[test]
    fn ignores_ordinary_launches() {
        assert!(parse(&["bililive-recorder-gui"]).is_none());
        assert!(parse(&["app", "--some-other-flag"]).is_none());
        // A guardian launch with no program is not usable.
        assert!(parse(&["app", GUARDIAN_FLAG]).is_none());
        assert!(parse(&["app", GUARDIAN_FLAG, "--program"]).is_none());
        // Unknown flags are rejected rather than silently ignored.
        assert!(parse(&["app", GUARDIAN_FLAG, "--program", "/bin/true", "--nope"]).is_none());
    }

    #[test]
    fn arguments_round_trip() {
        let config = GuardianConfig {
            backend: BackendCommand {
                program: PathBuf::from("/tmp/odd name/Cli"),
                args: vec![OsString::from("run"), OsString::from("--bind")],
                envs: vec![(OsString::from("K"), OsString::from("v with spaces"))],
            },
            graceful_timeout: Duration::from_millis(2500),
        };
        let argv: Vec<OsString> = std::iter::once(OsString::from("app"))
            .chain(config.to_args())
            .collect();

        assert_eq!(GuardianConfig::from_args(argv), Some(config));
    }

    #[test]
    fn the_guardian_is_never_told_a_process_id() {
        // The only thing that identifies the backend is the command to run, so
        // there is no pid for the guardian to be wrong about.
        let config = parse(&["app", GUARDIAN_FLAG, "--program", "/bin/true"]).unwrap();
        let rendered = config
            .to_args()
            .iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join(" ");
        assert!(!rendered.contains("--pid"));
        assert!(!rendered.to_lowercase().contains("pid"));
    }
}
