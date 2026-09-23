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
//! chance to flush, and no Job Object is used as a backstop. Linux is
//! unmeasured as well.
//!
//! # Known residual
//!
//! If the *guardian* is killed outright while the app is still running, nothing
//! is left holding the backend. The app notices (the guardian's event pipe
//! reaches end of file) and reports it, but it cannot adopt the backend, so
//! that backend would have to be stopped by hand.

use std::{
    ffi::OsString,
    fs,
    io::{self, BufRead, BufReader, Read, Write},
    net::TcpListener,
    path::PathBuf,
    process::{Child, ChildStdout, Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicI32, Ordering},
    },
    time::{Duration, Instant},
};

use tauri::{Manager, WebviewUrl, WebviewWindowBuilder};
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};
use tauri_plugin_updater::UpdaterExt;

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
        if args.next()? != OsString::from(GUARDIAN_FLAG) {
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

/// Walks the guardian's own child through the two-stage shutdown.
fn shutdown_child(child: &mut Child, graceful_timeout: Duration) {
    if send_interrupt(child.id()) {
        let deadline = Instant::now() + graceful_timeout;
        loop {
            match child.try_wait() {
                // Reaped: from here on the pid is free, and nothing below may
                // signal it again.
                Ok(Some(_)) => return,
                Ok(None) if Instant::now() < deadline => std::thread::sleep(POLL_INTERVAL),
                Ok(None) => break,
                Err(_) => break,
            }
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
pub fn supervise<R, W>(config: &GuardianConfig, mut keepalive: R, mut events: W) -> i32
where
    R: Read + Send + 'static,
    W: Write,
{
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

fn report_fatal(app: &tauri::AppHandle, message: &str) {
    app.dialog()
        .message(message)
        .title("BililiveRecorder")
        .kind(MessageDialogKind::Error)
        .blocking_show();
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
            report_fatal(&app, "录播后端守护进程异常退出，后端可能仍在运行。");
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

    let deadline = Instant::now() + GUARDIAN_GRACEFUL_TIMEOUT + GUARDIAN_EXIT_MARGIN;
    loop {
        match guardian.try_wait() {
            Ok(Some(_)) => return,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(POLL_INTERVAL),
            Ok(None) => return,
            Err(_) => return,
        }
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
