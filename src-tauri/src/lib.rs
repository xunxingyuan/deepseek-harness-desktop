use std::{
    collections::VecDeque,
    fs::{self, File},
    path::{Component, Path, PathBuf},
    sync::Arc,
};

use flate2::read::GzDecoder;
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_shell::{
    process::{CommandChild, CommandEvent},
    ShellExt,
};
use url::Url;

const HARNESS_VERSION: &str = "0.1.0-rc.8";
const HARNESS_DATA_SCHEMA: &str = "rc8";
const MAX_DIAGNOSTIC_LINES: usize = 12;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct BackendStatus {
    phase: String,
    message: String,
    url: Option<String>,
    harness_version: String,
}

impl BackendStatus {
    fn starting() -> Self {
        Self {
            phase: "starting".into(),
            message: "正在启动内置 DeepSeek Harness…".into(),
            url: None,
            harness_version: HARNESS_VERSION.into(),
        }
    }

    fn running(url: String) -> Self {
        Self {
            phase: "running".into(),
            message: "DeepSeek Harness 已就绪。".into(),
            url: Some(url),
            harness_version: HARNESS_VERSION.into(),
        }
    }

    fn failed(message: impl Into<String>) -> Self {
        Self {
            phase: "failed".into(),
            message: message.into(),
            url: None,
            harness_version: HARNESS_VERSION.into(),
        }
    }
}

struct BackendRuntime {
    child: Option<CommandChild>,
    generation: u64,
    status: BackendStatus,
    diagnostics: VecDeque<String>,
}

impl Default for BackendRuntime {
    fn default() -> Self {
        Self {
            child: None,
            generation: 0,
            status: BackendStatus::starting(),
            diagnostics: VecDeque::new(),
        }
    }
}

#[derive(Clone, Default)]
struct BackendManager {
    inner: Arc<tauri::async_runtime::Mutex<BackendRuntime>>,
}

impl BackendManager {
    async fn status(&self) -> BackendStatus {
        self.inner.lock().await.status.clone()
    }

    async fn stop(&self) {
        let mut runtime = self.inner.lock().await;
        runtime.generation = runtime.generation.wrapping_add(1);
        if let Some(child) = runtime.child.take() {
            if let Err(error) = child.kill() {
                log::warn!("failed to stop Harness sidecar: {error}");
            }
        }
    }

    async fn start(&self, app: AppHandle) -> Result<BackendStatus, String> {
        self.stop().await;

        let resource_dir = app
            .path()
            .resource_dir()
            .map_err(|error| format!("无法定位应用资源目录：{error}"))?;
        let data_dir = app
            .path()
            .app_data_dir()
            .map_err(|error| format!("无法定位应用数据目录：{error}"))?;
        fs::create_dir_all(&data_dir).map_err(|error| format!("无法创建应用数据目录：{error}"))?;
        let working_dir = app.path().home_dir().unwrap_or_else(|_| data_dir.clone());

        let generation = {
            let mut runtime = self.inner.lock().await;
            runtime.generation = runtime.generation.wrapping_add(1);
            runtime.status = BackendStatus::starting();
            runtime.diagnostics.clear();
            runtime.generation
        };

        emit_status(&app, BackendStatus::starting());
        let entry = ensure_harness_runtime(&resource_dir, &data_dir)?;

        // rc.8 introduced an incompatible SQLite storage layout. Keep the
        // previous Harness home intact so upgrades start safely and rollback
        // remains possible.
        let dsh_home = data_dir.join("harness").join(HARNESS_DATA_SCHEMA);
        let agents_home = data_dir.join("agents");
        fs::create_dir_all(&dsh_home)
            .and_then(|_| fs::create_dir_all(&agents_home))
            .map_err(|error| format!("无法准备 Harness 数据目录：{error}"))?;

        let command = app
            .shell()
            .sidecar("node")
            .map_err(|error| format!("无法定位内置 Node.js：{error}"))?
            .args([
                entry.to_string_lossy().into_owned(),
                "web".into(),
                "--no-open".into(),
                "--host".into(),
                "127.0.0.1".into(),
                "--port".into(),
                "0".into(),
            ])
            .env("DSH_HOME", dsh_home)
            .env("DSH_AGENTS_HOME", agents_home)
            .env("DSH_TELEMETRY_DISABLED", "1")
            .current_dir(working_dir);

        let (mut events, child) = command
            .spawn()
            .map_err(|error| format!("无法启动内置 Harness：{error}"))?;

        {
            let mut runtime = self.inner.lock().await;
            if runtime.generation != generation {
                let _ = child.kill();
                return Ok(runtime.status.clone());
            }
            runtime.child = Some(child);
        }

        let manager = self.clone();
        let app_for_events = app.clone();
        tauri::async_runtime::spawn(async move {
            while let Some(event) = events.recv().await {
                match event {
                    CommandEvent::Stdout(bytes) => {
                        let line = String::from_utf8_lossy(&bytes).trim().to_owned();
                        log::info!(target: "dsh", "{line}");
                        if let Some(url) = readiness_url(&line) {
                            let status = BackendStatus::running(url);
                            let mut runtime = manager.inner.lock().await;
                            if runtime.generation == generation {
                                runtime.status = status.clone();
                                drop(runtime);
                                emit_status(&app_for_events, status);
                            }
                        }
                    }
                    CommandEvent::Stderr(bytes) => {
                        let line = String::from_utf8_lossy(&bytes).trim().to_owned();
                        log::warn!(target: "dsh", "{line}");
                        let mut runtime = manager.inner.lock().await;
                        if runtime.generation == generation && !line.is_empty() {
                            if runtime.diagnostics.len() == MAX_DIAGNOSTIC_LINES {
                                runtime.diagnostics.pop_front();
                            }
                            runtime.diagnostics.push_back(line);
                        }
                    }
                    CommandEvent::Terminated(payload) => {
                        let mut runtime = manager.inner.lock().await;
                        if runtime.generation != generation {
                            break;
                        }
                        runtime.child = None;
                        let detail = runtime
                            .diagnostics
                            .iter()
                            .cloned()
                            .collect::<Vec<_>>()
                            .join(" · ");
                        let suffix = payload.code.map_or_else(
                            || "进程已退出".to_owned(),
                            |code| format!("进程退出码 {code}"),
                        );
                        let message = if detail.is_empty() {
                            format!("DeepSeek Harness 意外停止（{suffix}）。")
                        } else {
                            format!("DeepSeek Harness 意外停止（{suffix}）：{detail}")
                        };
                        let status = BackendStatus::failed(message);
                        runtime.status = status.clone();
                        drop(runtime);
                        emit_status(&app_for_events, status);
                        break;
                    }
                    _ => {}
                }
            }
        });

        Ok(self.status().await)
    }
}

fn ensure_harness_runtime(resource_dir: &Path, data_dir: &Path) -> Result<PathBuf, String> {
    let runtime_root = data_dir.join("runtime");
    let runtime_id = format!("{}-{}", HARNESS_VERSION, std::env::consts::ARCH);
    let destination = runtime_root.join(&runtime_id);
    let entry = destination
        .join("node_modules")
        .join("@deepseek-ai")
        .join("dsh")
        .join("lib")
        .join("bin.js");
    if entry.is_file() {
        return Ok(entry);
    }

    let archive_path = resource_dir.join("runtime").join("dsh-runtime.tar.gz");
    if !archive_path.is_file() {
        return Err(format!(
            "安装包缺少 Harness 运行时：{}",
            archive_path.display()
        ));
    }

    fs::create_dir_all(&runtime_root).map_err(|error| format!("无法创建运行时目录：{error}"))?;
    let staging = runtime_root.join(format!(".{runtime_id}.staging"));
    if staging.exists() {
        fs::remove_dir_all(&staging).map_err(|error| format!("无法清理未完成的运行时：{error}"))?;
    }
    fs::create_dir_all(&staging).map_err(|error| format!("无法创建运行时暂存目录：{error}"))?;

    let unpack_result = (|| -> Result<(), String> {
        let archive_file = File::open(&archive_path)
            .map_err(|error| format!("无法读取 Harness 运行时：{error}"))?;
        let decoder = GzDecoder::new(archive_file);
        let mut archive = tar::Archive::new(decoder);
        let entries = archive
            .entries()
            .map_err(|error| format!("无法读取 Harness 归档目录：{error}"))?;
        for entry_result in entries {
            let mut archive_entry =
                entry_result.map_err(|error| format!("无法读取 Harness 归档条目：{error}"))?;
            let entry_path = archive_entry
                .path()
                .map_err(|error| format!("Harness 归档包含无效路径：{error}"))?
                .into_owned();
            if !safe_archive_path(&entry_path) {
                return Err(format!(
                    "Harness 归档包含越界路径：{}",
                    entry_path.display()
                ));
            }
            let entry_type = archive_entry.header().entry_type();
            if entry_type.is_symlink() || entry_type.is_hard_link() {
                let link = archive_entry
                    .link_name()
                    .map_err(|error| format!("Harness 归档包含无效链接：{error}"))?
                    .ok_or_else(|| "Harness 归档包含空链接。".to_owned())?;
                if !archive_link_stays_inside(&entry_path, &link) {
                    return Err(format!("Harness 归档链接越界：{}", entry_path.display()));
                }
            }
            let unpacked = archive_entry
                .unpack_in(&staging)
                .map_err(|error| format!("无法解压 Harness 运行时：{error}"))?;
            if !unpacked {
                return Err(format!("Harness 归档条目越界：{}", entry_path.display()));
            }
        }
        Ok(())
    })();

    if let Err(error) = unpack_result {
        let _ = fs::remove_dir_all(&staging);
        return Err(error);
    }
    if !staging
        .join("node_modules")
        .join("@deepseek-ai")
        .join("dsh")
        .join("lib")
        .join("bin.js")
        .is_file()
    {
        let _ = fs::remove_dir_all(&staging);
        return Err("Harness 运行时归档不完整。".into());
    }
    if destination.exists() {
        fs::remove_dir_all(&destination).map_err(|error| format!("无法替换旧运行时：{error}"))?;
    }
    fs::rename(&staging, &destination)
        .map_err(|error| format!("无法启用 Harness 运行时：{error}"))?;
    Ok(entry)
}

fn safe_archive_path(path: &Path) -> bool {
    path.components()
        .all(|part| matches!(part, Component::Normal(_) | Component::CurDir))
}

fn archive_link_stays_inside(entry_path: &Path, link: &Path) -> bool {
    if link.is_absolute() || !safe_archive_path(entry_path) {
        return false;
    }
    let mut depth = entry_path.parent().map_or(0, |parent| {
        parent
            .components()
            .filter(|part| matches!(part, Component::Normal(_)))
            .count()
    });
    for part in link.components() {
        match part {
            Component::Normal(_) => depth += 1,
            Component::CurDir => {}
            Component::ParentDir if depth > 0 => depth -= 1,
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => return false,
        }
    }
    true
}

fn readiness_url(line: &str) -> Option<String> {
    let candidate = line.strip_prefix("dsh web: ")?.split_whitespace().next()?;
    let parsed = Url::parse(candidate).ok()?;
    if parsed.scheme() != "http"
        || parsed.host_str() != Some("127.0.0.1")
        || parsed.port().is_none()
        || parsed.username() != ""
        || parsed.password().is_some()
    {
        return None;
    }
    Some(parsed.to_string())
}

fn emit_status(app: &AppHandle, status: BackendStatus) {
    if let Err(error) = app.emit("backend-status", status) {
        log::warn!("failed to emit backend status: {error}");
    }
}

#[tauri::command]
async fn backend_status(manager: State<'_, BackendManager>) -> Result<BackendStatus, String> {
    Ok(manager.status().await)
}

#[tauri::command]
async fn restart_backend(
    app: AppHandle,
    manager: State<'_, BackendManager>,
) -> Result<BackendStatus, String> {
    match manager.start(app.clone()).await {
        Ok(status) => Ok(status),
        Err(error) => {
            let status = BackendStatus::failed(error.clone());
            {
                let mut runtime = manager.inner.lock().await;
                runtime.status = status.clone();
            }
            emit_status(&app, status);
            Err(error)
        }
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let manager = BackendManager::default();
    let manager_for_setup = manager.clone();

    let app = tauri::Builder::default()
        .plugin(tauri_plugin_log::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.show();
                let _ = window.set_focus();
            }
        }))
        .manage(manager.clone())
        .invoke_handler(tauri::generate_handler![backend_status, restart_backend])
        .setup(move |app| {
            #[cfg(target_os = "macos")]
            {
                use tauri::menu::{MenuBuilder, SubmenuBuilder};

                let app_menu = SubmenuBuilder::new(app, "DSH Desktop")
                    .about(None)
                    .separator()
                    .hide()
                    .hide_others()
                    .show_all()
                    .separator()
                    .quit()
                    .build()?;
                let edit_menu = SubmenuBuilder::new(app, "Edit")
                    .undo()
                    .redo()
                    .separator()
                    .cut()
                    .copy()
                    .paste()
                    .select_all()
                    .build()?;
                let menu = MenuBuilder::new(app)
                    .item(&app_menu)
                    .item(&edit_menu)
                    .build()?;
                app.set_menu(menu)?;
            }

            let handle = app.handle().clone();
            let backend = manager_for_setup.clone();
            tauri::async_runtime::spawn(async move {
                if let Err(error) = backend.start(handle.clone()).await {
                    let status = BackendStatus::failed(error);
                    {
                        let mut runtime = backend.inner.lock().await;
                        runtime.status = status.clone();
                    }
                    emit_status(&handle, status);
                }
            });
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("failed to build DSH Desktop");

    app.run(move |_handle, event| {
        if matches!(
            event,
            tauri::RunEvent::Exit | tauri::RunEvent::ExitRequested { .. }
        ) {
            tauri::async_runtime::block_on(manager.stop());
        }
    });
}

#[cfg(test)]
mod tests {
    use std::{fs, path::Path, process::Command};

    use super::{
        archive_link_stays_inside, ensure_harness_runtime, readiness_url, safe_archive_path,
    };

    #[test]
    fn accepts_loopback_readiness_line() {
        assert_eq!(
            readiness_url("dsh web: http://127.0.0.1:49152"),
            Some("http://127.0.0.1:49152/".into())
        );
    }

    #[test]
    fn rejects_non_loopback_readiness_line() {
        assert_eq!(readiness_url("dsh web: http://localhost:3080"), None);
        assert_eq!(readiness_url("dsh web: https://127.0.0.1:3080"), None);
        assert_eq!(readiness_url("dsh web: http://example.com:3080"), None);
    }

    #[test]
    fn rejects_archive_path_traversal() {
        assert!(safe_archive_path(Path::new("node_modules/pkg/index.js")));
        assert!(!safe_archive_path(Path::new("../outside")));
        assert!(archive_link_stays_inside(
            Path::new("node_modules/pkg/node_modules/dep"),
            Path::new("../../../dep/node_modules/dep")
        ));
        assert!(!archive_link_stays_inside(
            Path::new("node_modules/pkg/link"),
            Path::new("../../../outside")
        ));
    }

    #[test]
    fn extracts_prepared_runtime_archive_when_present() {
        let resource_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
        if !resource_dir.join("runtime/dsh-runtime.tar.gz").is_file() {
            return;
        }
        let data_dir =
            std::env::temp_dir().join(format!("dsh-desktop-runtime-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&data_dir);
        let entry = ensure_harness_runtime(resource_dir, &data_dir)
            .expect("prepared runtime archive should extract safely");
        assert!(entry.is_file());
        let bundled_node = fs::read_dir(resource_dir.join("runtime"))
            .expect("prepared runtime directory should be readable")
            .filter_map(Result::ok)
            .map(|item| item.path())
            .find(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with("node-") && !name.ends_with(".json"))
            })
            .expect("prepared Node.js sidecar should exist");
        let version = Command::new(bundled_node)
            .arg(&entry)
            .arg("--version")
            .output()
            .expect("extracted Harness should run with bundled Node.js");
        assert!(version.status.success());
        assert_eq!(
            String::from_utf8_lossy(&version.stdout).trim(),
            "0.1.0-rc.8"
        );
        fs::remove_dir_all(&data_dir).expect("temporary runtime should be removable");
    }
}
