//! BililiveRecorder GUI — a Tauri desktop shell around the official
//! BililiveRecorder CLI, whose embedded WebUI is the only user interface.
//!
//! # Sidecar lifecycle guarantees
//!
//! The app supervises exactly one backend process: the official
//! `BililiveRecorder.Cli` shipped inside the bundle. It is always addressed by
//! the PID returned from `Command::spawn`. The app never enumerates processes
//! and never signals anything by name or by process group, so a recorder the
//! user started independently is never touched.
//!
//! | Exit path | Guarantee |
//! | --- | --- |
//! | Main window closed (`CloseRequested`) | backend shut down before the app exits |
//! | App quit — Cmd+Q, Dock ▸ Quit, `osascript quit` | backend shut down; on macOS a normal quit only ever delivers `RunEvent::Exit`, never `CloseRequested` |
//! | `SIGTERM` / `SIGHUP` / `SIGINT` to the app | handled explicitly; backend shut down, app exits with `128 + signal` |
//! | App `SIGKILL`ed, crashed, panicked or aborted | a guardian process spawned next to the backend sees the app disappear through a closed pipe and shuts the backend down |
//!
//! Every path ends in the same two-stage shutdown: `SIGINT` first, which lets
//! the CLI flush recordings and exit cleanly, escalating to `SIGKILL` only
//! after [`GRACEFUL_SHUTDOWN_TIMEOUT`].

use std::{
    ffi::OsString,
    fs,
    io::{self, Read},
    net::TcpListener,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        Mutex,
        atomic::{AtomicI32, Ordering},
    },
    time::{Duration, Instant},
};

use tauri::{Manager, WebviewUrl, WebviewWindowBuilder};
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};
use tauri_plugin_updater::UpdaterExt;

#[cfg(unix)]
use std::os::unix::ffi::OsStringExt;

const SIDECAR_NAME: &str = "BililiveRecorder.Cli";

/// Hidden argv flag that re-runs this same executable as the sidecar guardian.
const GUARDIAN_FLAG: &str = "--bililive-sidecar-guardian";

/// How long the backend may take to shut down gracefully before it is killed.
const GRACEFUL_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(8);
const SHUTDOWN_POLL_INTERVAL: Duration = Duration::from_millis(100);
/// How long the guardian keeps re-reading a target's identity before deciding.
/// Covers the instant between `fork` and `exec`, when the child still reports
/// its parent's executable.
const IDENTITY_SETTLE_TIMEOUT: Duration = Duration::from_millis(1000);

/// Exit codes reported by the guardian process (not used by the GUI itself).
const GUARDIAN_REAPED: i32 = 0;
const GUARDIAN_TARGET_ALREADY_GONE: i32 = 0;
const GUARDIAN_TARGET_UNVERIFIED: i32 = 2;

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

#[derive(Default)]
struct BackendInner {
    child: Option<Child>,
    /// Kept alive so its stdin pipe stays open: the pipe closing is how the
    /// guardian learns that this process is gone. Dropping it early would make
    /// the guardian reap the backend while the app is still running.
    guardian: Option<Child>,
    stopping: bool,
}

#[derive(Default)]
struct BackendState(Mutex<BackendInner>);

impl BackendState {
    fn lock(&self) -> std::sync::MutexGuard<'_, BackendInner> {
        // A panic in another thread must not turn a shutdown into a crash.
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

fn reserve_local_port() -> Result<u16, String> {
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

// ---------------------------------------------------------------------------
// Process identity — used to confirm that a PID is still the process we
// spawned before anything is signalled. The app's primary safety property is
// that it only ever addresses the PID returned by `Command::spawn`; this check
// is the second line of defence against that PID having been recycled since.
// ---------------------------------------------------------------------------

/// Resolves the executable behind `pid`, or `None` when it cannot be read.
#[cfg(unix)]
fn process_executable(pid: u32) -> Option<PathBuf> {
    #[cfg(target_os = "linux")]
    {
        let link = fs::read_link(format!("/proc/{pid}/exe")).ok()?;
        let text = link.to_string_lossy().into_owned();
        // The kernel marks the link when the backing file was replaced.
        let text = text.strip_suffix(" (deleted)").unwrap_or(&text).to_string();
        Some(PathBuf::from(text))
    }

    #[cfg(target_os = "macos")]
    {
        let mut buffer = vec![0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
        let length = unsafe {
            libc::proc_pidpath(
                pid as libc::c_int,
                buffer.as_mut_ptr().cast(),
                buffer.len() as u32,
            )
        };
        if length <= 0 {
            return None;
        }
        buffer.truncate(length as usize);
        if buffer.last() == Some(&0) {
            buffer.pop();
        }
        Some(PathBuf::from(OsString::from_vec(buffer)))
    }
}

/// True when the resolved executables refer to the same file.
#[cfg(unix)]
fn same_executable(left: &Path, right: &Path) -> bool {
    match (fs::canonicalize(left), fs::canonicalize(right)) {
        (Ok(left), Ok(right)) => left == right,
        // The backend may already be gone (uninstalled app): fall back to a
        // literal comparison, which then reports a mismatch and stops us from
        // signalling a PID we can no longer identify.
        _ => left == right,
    }
}

/// True while `pid` exists and can be signalled.
#[cfg(unix)]
fn process_alive(pid: u32) -> bool {
    if unsafe { libc::kill(pid as libc::c_int, 0) } == 0 {
        return true;
    }
    // EPERM means the process exists but belongs to another user.
    io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

/// Asks the backend to shut down cleanly. Returns whether the request was sent.
fn request_graceful_shutdown(pid: u32) -> bool {
    #[cfg(unix)]
    {
        unsafe { libc::kill(pid as libc::c_int, libc::SIGINT) == 0 }
    }

    #[cfg(windows)]
    {
        // A console-less child cannot be sent Ctrl+C; the caller escalates
        // straight to a forced termination instead.
        let _ = pid;
        false
    }
}

/// Terminates the backend without giving it a chance to clean up.
#[cfg(unix)]
fn force_terminate(pid: u32) {
    unsafe { libc::kill(pid as libc::c_int, libc::SIGKILL) };
}

/// Walks `pid` through the two-stage shutdown and returns once it is gone.
#[cfg(unix)]
fn shutdown_process(pid: u32, timeout: Duration) {
    if !process_alive(pid) {
        return;
    }

    if request_graceful_shutdown(pid) {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if !process_alive(pid) {
                return;
            }
            std::thread::sleep(SHUTDOWN_POLL_INTERVAL);
        }
    }

    force_terminate(pid);
}

// ---------------------------------------------------------------------------
// Guardian: survives this process and reaps the backend if we vanish without
// running any shutdown code at all (SIGKILL, crash, panic, abort).
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReapTarget {
    pub pid: u32,
    pub executable: PathBuf,
}

impl ReapTarget {
    /// Parses `["<exe>", GUARDIAN_FLAG, "<pid>", "<sidecar path>"]`.
    pub fn from_args<I, S>(args: I) -> Option<Self>
    where
        I: IntoIterator<Item = S>,
        S: Into<OsString>,
    {
        let mut args = args.into_iter().map(Into::into);
        let _executable = args.next()?;
        if args.next()? != OsString::from(GUARDIAN_FLAG) {
            return None;
        }
        let pid = args.next()?.to_string_lossy().parse().ok()?;
        let executable = PathBuf::from(args.next()?);
        Some(Self { pid, executable })
    }
}

/// Parses the guardian target from the real process arguments.
pub fn guardian_target() -> Option<ReapTarget> {
    ReapTarget::from_args(std::env::args_os())
}

/// Reaps the target once `input` reaches end of file.
///
/// The GUI holds the write end of that pipe for its whole lifetime, so end of
/// file means the GUI is gone — the only signal that also fires when it was
/// killed outright.
#[cfg(unix)]
pub fn guard<R: Read>(input: &mut R, target: &ReapTarget) -> i32 {
    guard_with_timeout(input, target, GRACEFUL_SHUTDOWN_TIMEOUT)
}

/// [`guard`] with an explicit graceful-shutdown budget.
#[cfg(unix)]
pub fn guard_with_timeout<R: Read>(input: &mut R, target: &ReapTarget, timeout: Duration) -> i32 {
    let mut scratch = [0u8; 64];
    loop {
        match input.read(&mut scratch) {
            Ok(0) => break,
            Ok(_) => continue,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            // A read error is not proof that the app is gone, but the pipe is
            // the only supervision channel we have; treat it as a disconnect.
            Err(_) => break,
        }
    }

    if !process_alive(target.pid) {
        return GUARDIAN_TARGET_ALREADY_GONE;
    }

    // A positive mismatch is proof that the PID now belongs to a different
    // program, so nothing is signalled. An identity that cannot be read at all
    // is not proof of anything: refusing there would resurrect the orphaned
    // backend this guardian exists to prevent, and the target PID was alive and
    // ours only moments ago.
    let deadline = Instant::now() + IDENTITY_SETTLE_TIMEOUT;
    loop {
        match process_executable(target.pid) {
            Some(actual) if same_executable(&actual, &target.executable) => break,
            Some(_) => {
                if Instant::now() >= deadline {
                    return GUARDIAN_TARGET_UNVERIFIED;
                }
            }
            None => break,
        }
        std::thread::sleep(Duration::from_millis(25));
    }

    shutdown_process(target.pid, timeout);
    GUARDIAN_REAPED
}

/// Entry point used when the binary is re-run with [`GUARDIAN_FLAG`].
///
/// Only Unix builds ship a guardian: a *correct* Windows equivalent has to be a
/// Job Object with `KILL_ON_JOB_CLOSE`, which needs Win32 bindings this project
/// does not carry yet. On Windows the guardian is simply absent and the in-app
/// exit paths do the work.
#[cfg(unix)]
pub fn run_guardian(target: &ReapTarget) -> i32 {
    guard(&mut io::stdin(), target)
}

#[cfg(unix)]
fn spawn_guardian(sidecar_pid: u32, sidecar: &Path) -> Result<Child, String> {
    let executable = std::env::current_exe().map_err(|error| error.to_string())?;
    let mut command = Command::new(executable);
    command
        .arg(GUARDIAN_FLAG)
        .arg(sidecar_pid.to_string())
        .arg(sidecar)
        // Piped and never written to: the GUI holds the write end so that the
        // pipe closes exactly when this process dies.
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    command.spawn().map_err(|error| error.to_string())
}

#[cfg(not(unix))]
fn spawn_guardian(_sidecar_pid: u32, _sidecar: &Path) -> Result<Child, String> {
    Err("当前平台未提供 sidecar 守护进程。".into())
}

// ---------------------------------------------------------------------------
// Backend supervision
// ---------------------------------------------------------------------------

fn stop_backend(app: &tauri::AppHandle) {
    let state = app.state::<BackendState>();

    let child = {
        let mut inner = state.lock();
        // Marked before the child is taken so `monitor_backend` never reports
        // this as an unexpected exit.
        inner.stopping = true;
        inner.child.take()
    };

    if let Some(mut child) = child {
        let pid = child.id();
        if request_graceful_shutdown(pid) {
            let deadline = Instant::now() + GRACEFUL_SHUTDOWN_TIMEOUT;
            loop {
                match child.try_wait() {
                    Ok(Some(_)) => break,
                    Ok(None) => {
                        if Instant::now() >= deadline {
                            break;
                        }
                        std::thread::sleep(SHUTDOWN_POLL_INTERVAL);
                    }
                    Err(_) => break,
                }
            }
        }
        // Also reached when the backend ignored SIGINT, or when it could not be
        // asked politely in the first place (Windows).
        let _ = child.kill();
        let _ = child.wait();
    }

    // The backend is confirmed down, so the guardian has nothing left to do.
    // Closing its stdin lets it exit instead of lingering until this process
    // disappears; it re-checks the backend before acting either way.
    let guardian = state.lock().guardian.take();
    if let Some(mut guardian) = guardian {
        drop(guardian.stdin.take());
        std::thread::spawn(move || {
            let _ = guardian.wait();
        });
    }
}

fn monitor_backend(app: tauri::AppHandle) {
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_millis(250)).await;
            let state = app.state::<BackendState>();
            let exited_unexpectedly = {
                let mut inner = state.lock();
                let Some(child) = inner.child.as_mut() else {
                    break;
                };
                match child.try_wait() {
                    Ok(Some(_)) => {
                        let unexpected = !inner.stopping;
                        inner.child = None;
                        unexpected
                    }
                    Ok(None) => false,
                    Err(_) => {
                        inner.child = None;
                        true
                    }
                }
            };

            if exited_unexpectedly {
                app.dialog()
                    .message("录播后端意外退出，请查看应用日志。")
                    .title("BililiveRecorder")
                    .kind(MessageDialogKind::Error)
                    .blocking_show();
                app.exit(1);
                break;
            }
        }
    });
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

    let mut command = Command::new(&executable);
    command
        .current_dir(executable.parent().ok_or("录播后端目录不可用。")?)
        .arg("run")
        .arg("--bind")
        .arg(&url)
        .arg(&workdir)
        .env(
            "BILILIVERECORDER_LOG_FILE_PATH",
            log_directory.join("bililive-recorder.txt"),
        )
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }

    let child = command
        .spawn()
        .map_err(|error| format!("无法启动录播后端：{error}"))?;
    let sidecar_pid = child.id();

    // Started before the guardian so that nothing else can inherit the pipe.
    let guardian = match spawn_guardian(sidecar_pid, &executable) {
        Ok(guardian) => Some(guardian),
        Err(error) => {
            // Losing the guardian only costs the crash/SIGKILL path; every
            // other exit path still shuts the backend down.
            eprintln!("无法启动 sidecar 守护进程：{error}");
            None
        }
    };

    {
        let state = app.state::<BackendState>();
        let mut inner = state.lock();
        inner.child = Some(child);
        inner.stopping = false;
        inner.guardian = guardian;
    }
    monitor_backend(app.clone());

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(2))
        .build()
        .map_err(|error| error.to_string())?;
    for _ in 0..120 {
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
            // Shut the backend down before handing over to the new build so two
            // CLI instances never overlap.
            stop_backend(&app);
            app.restart();
        }
        Err(error) => {
            app.dialog()
                .message(format!("更新安装失败：{error}"))
                .title("BililiveRecorder")
                .kind(MessageDialogKind::Error)
                .blocking_show();
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
            std::thread::sleep(SHUTDOWN_POLL_INTERVAL);
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
                    handle
                        .dialog()
                        .message(error)
                        .title("BililiveRecorder")
                        .kind(MessageDialogKind::Error)
                        .blocking_show();
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

    #[test]
    fn parses_guardian_arguments() {
        let target = ReapTarget::from_args([
            "/Applications/BililiveRecorder GUI.app/Contents/MacOS/bililive-recorder-gui",
            GUARDIAN_FLAG,
            "4242",
            "/Applications/BililiveRecorder GUI.app/Contents/Resources/sidecar/BililiveRecorder.Cli",
        ])
        .expect("guardian arguments should parse");
        assert_eq!(target.pid, 4242);
        assert_eq!(
            target.executable,
            PathBuf::from(
                "/Applications/BililiveRecorder GUI.app/Contents/Resources/sidecar/BililiveRecorder.Cli"
            )
        );
    }

    #[test]
    fn ignores_ordinary_invocations() {
        assert!(ReapTarget::from_args(["bililive-recorder-gui"]).is_none());
        assert!(ReapTarget::from_args(["app", "--some-other-flag", "1"]).is_none());
        // Missing the executable argument.
        assert!(ReapTarget::from_args(["app", GUARDIAN_FLAG, "12"]).is_none());
        // Non-numeric PID.
        assert!(ReapTarget::from_args(["app", GUARDIAN_FLAG, "abc", "/tmp/x"]).is_none());
    }
}
