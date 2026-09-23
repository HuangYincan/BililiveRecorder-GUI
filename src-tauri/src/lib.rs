use std::{
    fs,
    net::TcpListener,
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::Mutex,
    time::{Duration, Instant},
};

use tauri::{Manager, WebviewUrl, WebviewWindowBuilder};
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};
use tauri_plugin_updater::UpdaterExt;

const SIDECAR_NAME: &str = "BililiveRecorder.Cli";

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

struct OwnedBackend {
    child: Option<Child>,
}

impl OwnedBackend {
    fn new(child: Child) -> Self {
        Self { child: Some(child) }
    }

    fn shutdown(&mut self) {
        if let Some(child) = self.child.as_mut() {
            shutdown_child(child, Duration::from_secs(5));
        }
        self.child = None;
    }
}

impl Drop for OwnedBackend {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

#[derive(Default)]
struct BackendInner {
    backend: Option<OwnedBackend>,
}

#[derive(Default)]
struct BackendState(Mutex<BackendInner>);

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
        Err("Windows 无法向无控制台后端发送 Ctrl+C。".into())
    }
}

fn shutdown_child(child: &mut Child, timeout: Duration) {
    let _ = request_graceful_shutdown(child.id());
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        match child.try_wait() {
            Ok(Some(_)) => return,
            Ok(None) => {}
            Err(_) => break,
        }
        std::thread::sleep(Duration::from_millis(100));
    }

    let _ = child.kill();
    let _ = child.wait();
}

fn stop_backend(app: &tauri::AppHandle) {
    let mut backend = app
        .state::<BackendState>()
        .0
        .lock()
        .expect("backend state lock poisoned")
        .backend
        .take();
    if let Some(backend) = backend.as_mut() {
        backend.shutdown();
    }
}

fn monitor_backend(app: tauri::AppHandle) {
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_millis(250)).await;
            let state = app.state::<BackendState>();
            let exited_unexpectedly = {
                let mut inner = state.0.lock().expect("backend state lock poisoned");
                let Some(backend) = inner.backend.as_mut() else {
                    break;
                };
                let Some(child) = backend.child.as_mut() else {
                    inner.backend = None;
                    break;
                };
                match child.try_wait() {
                    Ok(Some(_)) => {
                        backend.child = None;
                        inner.backend = None;
                        true
                    }
                    Ok(None) => false,
                    Err(_) => {
                        backend.child = None;
                        inner.backend = None;
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
    {
        let state = app.state::<BackendState>();
        let mut inner = state.0.lock().map_err(|error| error.to_string())?;
        inner.backend = Some(OwnedBackend::new(child));
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
        Ok(()) => app.restart(),
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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .manage(BackendState::default())
        .setup(|app| {
            let handle = app.handle().clone();
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
                stop_backend(window.app_handle());
                window.app_handle().exit(0);
            }
        })
        .build(tauri::generate_context!())
        .expect("error while building BililiveRecorder GUI");

    app.run(|app, event| {
        if matches!(event, tauri::RunEvent::ExitRequested { .. }) {
            stop_backend(app);
        }
    });
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn shutdown_only_stops_the_owned_child() {
        let mut owned = OwnedBackend::new(
            Command::new("sleep")
                .arg("30")
                .spawn()
                .expect("owned test child should start"),
        );
        let mut independent = OwnedBackend::new(
            Command::new("sleep")
                .arg("30")
                .spawn()
                .expect("independent test child should start"),
        );

        owned.shutdown();

        assert!(owned.child.is_none());
        assert!(
            independent
                .child
                .as_mut()
                .expect("independent child should still be tracked")
                .try_wait()
                .expect("independent child status should be readable")
                .is_none()
        );
    }

    #[test]
    fn dropping_owned_backend_reaps_its_child() {
        let backend = OwnedBackend::new(
            Command::new("sleep")
                .arg("30")
                .spawn()
                .expect("test child should start"),
        );
        let pid = backend
            .child
            .as_ref()
            .expect("child should be tracked")
            .id();

        drop(backend);

        assert_eq!(unsafe { libc::kill(pid as i32, 0) }, -1);
        assert_eq!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::ESRCH)
        );
    }
}
