use std::{
    collections::VecDeque,
    fs,
    io::{BufRead, BufReader, Read},
    net::TcpListener,
    path::{Path, PathBuf},
    process::{Child, Command, ExitStatus, Stdio},
    sync::Mutex,
    time::Duration,
};

use serde::{Deserialize, Serialize};
use tauri::{Emitter, Manager, WebviewUrl, WebviewWindowBuilder};
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_updater::UpdaterExt;

const SIDECAR_NAME: &str = "BililiveRecorder.Cli";
const MAX_LOG_LINES: usize = 300;

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

#[derive(Clone, Copy, Default, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum BackendStatus {
    #[default]
    Stopped,
    Starting,
    Running,
    Stopping,
    Failed,
}

#[derive(Default)]
struct BackendInner {
    child: Option<Child>,
    status: BackendStatus,
    workdir: Option<PathBuf>,
    url: Option<String>,
    last_error: Option<String>,
    logs: VecDeque<String>,
}

#[derive(Default)]
struct BackendState(Mutex<BackendInner>);

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct LauncherState {
    status: BackendStatus,
    workdir: Option<String>,
    url: Option<String>,
    last_error: Option<String>,
    logs: Vec<String>,
}

#[derive(Serialize)]
struct UpdateInfo {
    version: String,
    body: Option<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Preferences {
    workdir: PathBuf,
}

impl From<&BackendInner> for LauncherState {
    fn from(inner: &BackendInner) -> Self {
        Self {
            status: inner.status,
            workdir: inner
                .workdir
                .as_ref()
                .map(|path| path.display().to_string()),
            url: inner.url.clone(),
            last_error: inner.last_error.clone(),
            logs: inner.logs.iter().cloned().collect(),
        }
    }
}

fn snapshot(state: &BackendState) -> LauncherState {
    LauncherState::from(&*state.0.lock().expect("backend state lock poisoned"))
}

fn emit_state(app: &tauri::AppHandle, state: &BackendState) {
    let _ = app.emit("backend-state", snapshot(state));
}

fn append_log(inner: &mut BackendInner, line: String) {
    if line.is_empty() {
        return;
    }
    if inner.logs.len() >= MAX_LOG_LINES {
        inner.logs.pop_front();
    }
    inner.logs.push_back(line);
}

fn preferences_path(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_config_dir()
        .map(|directory| directory.join("launcher.json"))
        .map_err(|error| error.to_string())
}

fn load_preferences(app: &tauri::AppHandle) -> Option<Preferences> {
    let path = preferences_path(app).ok()?;
    serde_json::from_slice(&fs::read(path).ok()?).ok()
}

fn save_preferences(app: &tauri::AppHandle, workdir: &Path) -> Result<(), String> {
    let path = preferences_path(app)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let data = serde_json::to_vec_pretty(&Preferences {
        workdir: workdir.to_path_buf(),
    })
    .map_err(|error| error.to_string())?;
    fs::write(path, data).map_err(|error| error.to_string())
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

fn request_graceful_shutdown(pid: u32) -> Result<(), String> {
    #[cfg(unix)]
    {
        let result = unsafe { libc::kill(pid as i32, libc::SIGINT) };
        if result == 0 {
            Ok(())
        } else {
            Err(std::io::Error::last_os_error().to_string())
        }
    }

    #[cfg(windows)]
    {
        let _ = pid;
        Err("Windows 当前无法向无控制台后端发送 Ctrl+C，将使用终止进程兜底。".into())
    }
}

#[cfg(unix)]
fn exit_signal(status: &ExitStatus) -> Option<i32> {
    use std::os::unix::process::ExitStatusExt;
    status.signal()
}

#[cfg(windows)]
fn exit_signal(_status: &ExitStatus) -> Option<i32> {
    None
}

fn spawn_log_reader(app: tauri::AppHandle, reader: impl Read + Send + 'static, stderr: bool) {
    std::thread::spawn(move || {
        for line in BufReader::new(reader).lines().map_while(Result::ok) {
            let state = app.state::<BackendState>();
            {
                let mut inner = state.0.lock().expect("backend state lock poisoned");
                append_log(
                    &mut inner,
                    if stderr {
                        format!("[stderr] {line}")
                    } else {
                        line
                    },
                );
            }
            emit_state(&app, &state);
        }
    });
}

fn monitor_child(app: tauri::AppHandle) {
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_millis(150)).await;
            let state = app.state::<BackendState>();
            let mut should_exit = false;
            let mut should_emit = false;
            {
                let mut inner = state.0.lock().expect("backend state lock poisoned");
                let result = match inner.child.as_mut() {
                    Some(child) => child.try_wait(),
                    None => break,
                };
                match result {
                    Ok(Some(status)) => {
                        append_log(
                            &mut inner,
                            format!(
                                "后端已退出（code={:?}, signal={:?}）",
                                status.code(),
                                exit_signal(&status)
                            ),
                        );
                        inner.child = None;
                        inner.url = None;
                        if inner.status == BackendStatus::Stopping || status.success() {
                            inner.status = BackendStatus::Stopped;
                        } else {
                            inner.status = BackendStatus::Failed;
                            inner.last_error =
                                Some(format!("录播后端异常退出：code={:?}", status.code()));
                        }
                        should_exit = true;
                        should_emit = true;
                    }
                    Ok(None) => {}
                    Err(error) => {
                        inner.child = None;
                        inner.url = None;
                        inner.status = BackendStatus::Failed;
                        inner.last_error = Some(format!("无法读取后端状态：{error}"));
                        should_exit = true;
                        should_emit = true;
                    }
                }
            }
            if should_emit {
                emit_state(&app, &state);
            }
            if should_exit {
                break;
            }
        }
    });
}

fn fail_start(app: &tauri::AppHandle, state: &BackendState, error: String) -> String {
    {
        let mut inner = state.0.lock().expect("backend state lock poisoned");
        inner.status = BackendStatus::Failed;
        inner.last_error = Some(error.clone());
        inner.url = None;
    }
    emit_state(app, state);
    error
}

fn stop_backend_before_exit(app: &tauri::AppHandle) {
    let state = app.state::<BackendState>();
    let pid = {
        let mut inner = state.0.lock().expect("backend state lock poisoned");
        let Some(pid) = inner.child.as_ref().map(Child::id) else {
            return;
        };
        inner.status = BackendStatus::Stopping;
        pid
    };

    let _ = request_graceful_shutdown(pid);
    for _ in 0..30 {
        std::thread::sleep(Duration::from_millis(100));
        if state
            .0
            .lock()
            .expect("backend state lock poisoned")
            .child
            .is_none()
        {
            return;
        }
    }

    if let Some(mut child) = state
        .0
        .lock()
        .expect("backend state lock poisoned")
        .child
        .take()
    {
        let _ = child.kill();
        let _ = child.wait();
    }
}

#[tauri::command]
fn get_launcher_state(state: tauri::State<'_, BackendState>) -> LauncherState {
    snapshot(&state)
}

#[tauri::command]
fn choose_workdir(app: tauri::AppHandle) -> Result<Option<String>, String> {
    let selection = app.dialog().file().blocking_pick_folder();
    selection
        .map(|path| path.into_path().map_err(|error| error.to_string()))
        .transpose()
        .map(|path| path.map(|value| value.display().to_string()))
}

#[tauri::command]
async fn check_for_update(app: tauri::AppHandle) -> Result<Option<UpdateInfo>, String> {
    let update = app
        .updater()
        .map_err(|error| error.to_string())?
        .check()
        .await
        .map_err(|error| error.to_string())?;
    Ok(update.map(|update| UpdateInfo {
        version: update.version,
        body: update.body,
    }))
}

#[tauri::command]
async fn install_update(app: tauri::AppHandle) -> Result<(), String> {
    let update = app
        .updater()
        .map_err(|error| error.to_string())?
        .check()
        .await
        .map_err(|error| error.to_string())?
        .ok_or("当前已是最新版本。")?;
    update
        .download_and_install(|_, _| {}, || {})
        .await
        .map_err(|error| error.to_string())?;
    stop_backend_before_exit(&app);
    app.restart();
}

#[tauri::command]
async fn start_backend(
    app: tauri::AppHandle,
    state: tauri::State<'_, BackendState>,
    workdir: String,
) -> Result<LauncherState, String> {
    let workdir = PathBuf::from(workdir);
    fs::create_dir_all(&workdir).map_err(|error| format!("无法创建工作目录：{error}"))?;
    if !workdir.is_dir() {
        return Err("工作目录不是文件夹。".into());
    }

    {
        let mut inner = state.0.lock().map_err(|error| error.to_string())?;
        if inner.child.is_some() {
            return Err("录播后端已经在运行。".into());
        }
        inner.status = BackendStatus::Starting;
        inner.last_error = None;
        inner.logs.clear();
        inner.workdir = Some(workdir.clone());
    }
    emit_state(&app, &state);

    let port = reserve_local_port().map_err(|error| fail_start(&app, &state, error))?;
    let url = format!("http://127.0.0.1:{port}");
    let executable = sidecar_path(&app).map_err(|error| fail_start(&app, &state, error))?;
    if !executable.is_file() {
        return Err(fail_start(
            &app,
            &state,
            format!("找不到录播后端：{}", executable.display()),
        ));
    }

    let mut command = Command::new(&executable);
    command
        .current_dir(executable.parent().ok_or("录播后端目录不可用。")?)
        .arg("run")
        .arg("--bind")
        .arg(&url)
        .arg(&workdir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }

    let mut child = command
        .spawn()
        .map_err(|error| fail_start(&app, &state, format!("无法启动录播后端：{error}")))?;
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    {
        let mut inner = state.0.lock().map_err(|error| error.to_string())?;
        append_log(&mut inner, format!("启动 {SIDECAR_NAME}，监听 {url}"));
        inner.child = Some(child);
        inner.url = Some(url.clone());
    }
    if let Some(stdout) = stdout {
        spawn_log_reader(app.clone(), stdout, false);
    }
    if let Some(stderr) = stderr {
        spawn_log_reader(app.clone(), stderr, true);
    }
    monitor_child(app.clone());
    if let Err(error) = save_preferences(&app, &workdir) {
        let mut inner = state
            .0
            .lock()
            .map_err(|lock_error| lock_error.to_string())?;
        append_log(&mut inner, format!("无法保存启动器设置：{error}"));
    }
    emit_state(&app, &state);

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(2))
        .build()
        .map_err(|error| error.to_string())?;
    let version_url = format!("{url}/api/version");
    let mut ready = false;
    for _ in 0..80 {
        if state
            .0
            .lock()
            .map_err(|error| error.to_string())?
            .child
            .is_none()
        {
            break;
        }
        if client
            .get(&version_url)
            .send()
            .await
            .is_ok_and(|response| response.status().is_success())
        {
            ready = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }

    if !ready {
        let error = "录播后端未能在 20 秒内就绪，请检查后端日志。".to_string();
        let mut inner = state
            .0
            .lock()
            .map_err(|lock_error| lock_error.to_string())?;
        inner.status = BackendStatus::Failed;
        inner.last_error = Some(error.clone());
        if let Some(mut child) = inner.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        inner.url = None;
        drop(inner);
        emit_state(&app, &state);
        return Err(error);
    }

    {
        let mut inner = state.0.lock().map_err(|error| error.to_string())?;
        inner.status = BackendStatus::Running;
        append_log(&mut inner, "录播后端已就绪。".into());
    }
    emit_state(&app, &state);
    Ok(snapshot(&state))
}

#[tauri::command]
async fn stop_backend(
    app: tauri::AppHandle,
    state: tauri::State<'_, BackendState>,
) -> Result<LauncherState, String> {
    let pid = {
        let mut inner = state.0.lock().map_err(|error| error.to_string())?;
        let Some(pid) = inner.child.as_ref().map(Child::id) else {
            inner.status = BackendStatus::Stopped;
            inner.url = None;
            return Ok(LauncherState::from(&*inner));
        };
        inner.status = BackendStatus::Stopping;
        pid
    };
    emit_state(&app, &state);

    if let Err(error) = request_graceful_shutdown(pid) {
        let mut inner = state
            .0
            .lock()
            .map_err(|lock_error| lock_error.to_string())?;
        append_log(&mut inner, format!("优雅停止不可用：{error}"));
    }

    for _ in 0..40 {
        tokio::time::sleep(Duration::from_millis(125)).await;
        if state
            .0
            .lock()
            .map_err(|error| error.to_string())?
            .child
            .is_none()
        {
            emit_state(&app, &state);
            return Ok(snapshot(&state));
        }
    }

    let mut inner = state.0.lock().map_err(|error| error.to_string())?;
    if let Some(mut child) = inner.child.take() {
        append_log(&mut inner, "后端未及时退出，已强制终止。".into());
        let _ = child.kill();
        let _ = child.wait();
    }
    inner.status = BackendStatus::Stopped;
    inner.url = None;
    drop(inner);
    emit_state(&app, &state);
    Ok(snapshot(&state))
}

#[tauri::command]
fn open_recorder_window(
    app: tauri::AppHandle,
    state: tauri::State<'_, BackendState>,
) -> Result<(), String> {
    if let Some(window) = app.get_webview_window("recorder") {
        window.show().map_err(|error| error.to_string())?;
        window.set_focus().map_err(|error| error.to_string())?;
        return Ok(());
    }

    let url = {
        let inner = state.0.lock().map_err(|error| error.to_string())?;
        if inner.status != BackendStatus::Running {
            return Err("录播后端尚未运行。".into());
        }
        inner.url.clone().ok_or("录播后端地址不可用。")?
    };

    let url = url::Url::parse(&url).map_err(|error| error.to_string())?;
    WebviewWindowBuilder::new(&app, "recorder", WebviewUrl::External(url))
        .title("BililiveRecorder")
        .inner_size(1180.0, 760.0)
        .min_inner_size(800.0, 560.0)
        .build()
        .map_err(|error| error.to_string())?;
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .manage(BackendState::default())
        .setup(|app| {
            if let Some(preferences) = load_preferences(app.handle()) {
                app.state::<BackendState>()
                    .0
                    .lock()
                    .expect("backend state lock poisoned")
                    .workdir = Some(preferences.workdir);
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_launcher_state,
            choose_workdir,
            check_for_update,
            install_update,
            start_backend,
            stop_backend,
            open_recorder_window,
        ])
        .build(tauri::generate_context!())
        .expect("error while building BililiveRecorder GUI");

    app.run(|app, event| {
        if matches!(event, tauri::RunEvent::ExitRequested { .. }) {
            stop_backend_before_exit(app);
        }
    });
}
