use std::{
    collections::{BTreeMap, HashMap, HashSet, VecDeque},
    fs::{self, File},
    io::Write,
    path::{Component, Path, PathBuf},
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use flate2::read::GzDecoder;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_shell::{
    process::{CommandChild, CommandEvent},
    ShellExt,
};
use url::Url;

const HARNESS_VERSION: &str = "0.1.1-rc.1";
const PROJECT_HOMEPAGE: &str = "https://github.com/xunxingyuan/deepseek-harness-desktop";
const PROJECT_HOMEPAGE_MENU_ID: &str = "open-project-homepage";
// rc.8 and 0.1.1-rc.1 use the same workspace v2 and projection-cache v3 schemas.
// Keep this directory stable so upgrading does not hide existing projects.
const HARNESS_DATA_SCHEMA: &str = "rc8";
const HARNESS_MIGRATION_VERSION: &str = "workspace-v1";
const MAX_DIAGNOSTIC_LINES: usize = 12;

#[derive(Clone, Debug, Deserialize, Serialize)]
struct StorageUnit {
    name: String,
    version: u64,
    #[serde(flatten)]
    extra: Map<String, Value>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkspaceGlobal {
    initialized: bool,
    workspace_ids: Vec<String>,
    #[serde(default)]
    archived_session_ids: Vec<String>,
    #[serde(flatten)]
    extra: Map<String, Value>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkspaceRecord {
    path: String,
    title: String,
    session_ids: Vec<String>,
    created_at: String,
    updated_at: String,
    #[serde(flatten)]
    extra: Map<String, Value>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct WorkspaceTables {
    workspaces: BTreeMap<String, WorkspaceRecord>,
    #[serde(flatten)]
    extra: Map<String, Value>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct WorkspaceStorage {
    unit: StorageUnit,
    global: WorkspaceGlobal,
    tables: WorkspaceTables,
    #[serde(flatten)]
    extra: Map<String, Value>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct MigrationMarker {
    version: &'static str,
    migrated_at_unix_seconds: u64,
    imported_workspaces: usize,
    imported_sessions: usize,
}

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

        // rc.8 uses an isolated home because its storage layout differs from
        // earlier releases. Import durable user data before the backend opens
        // the new home, while keeping the legacy home intact for rollback.
        let legacy_dsh_home = data_dir.join("harness");
        let dsh_home = legacy_dsh_home.join(HARNESS_DATA_SCHEMA);
        let agents_home = data_dir.join("agents");
        fs::create_dir_all(&dsh_home)
            .and_then(|_| fs::create_dir_all(&agents_home))
            .map_err(|error| format!("无法准备 Harness 数据目录：{error}"))?;
        migrate_legacy_harness_data(&legacy_dsh_home, &dsh_home)?;

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

fn migrate_legacy_harness_data(legacy_home: &Path, target_home: &Path) -> Result<(), String> {
    let marker_path = target_home.join(format!(
        ".dsh-desktop-migration-{HARNESS_MIGRATION_VERSION}.json"
    ));
    if marker_path.is_file() {
        return Ok(());
    }

    let legacy_workspace_path = legacy_home.join("storages").join("workspace.json");
    let target_workspace_path = target_home.join("storages").join("workspace.json");
    recover_interrupted_atomic_write(&target_workspace_path)?;

    let mut imported_workspaces = 0;
    let mut imported_sessions = 0;
    if legacy_workspace_path.is_file() {
        let legacy_workspace = read_workspace_storage(&legacy_workspace_path)?;
        validate_workspace_storage(&legacy_workspace, &legacy_workspace_path)?;

        let mut target_workspace = if target_workspace_path.is_file() {
            let target_workspace = read_workspace_storage(&target_workspace_path)?;
            validate_workspace_storage(&target_workspace, &target_workspace_path)?;
            target_workspace
        } else {
            WorkspaceStorage {
                unit: legacy_workspace.unit.clone(),
                global: WorkspaceGlobal {
                    initialized: false,
                    workspace_ids: Vec::new(),
                    archived_session_ids: Vec::new(),
                    extra: Map::new(),
                },
                tables: WorkspaceTables {
                    workspaces: BTreeMap::new(),
                    extra: Map::new(),
                },
                extra: Map::new(),
            }
        };

        imported_workspaces = merge_workspace_storage(&mut target_workspace, &legacy_workspace)?;

        if target_workspace_path.is_file() {
            let backup_dir = legacy_home
                .join("migration-backups")
                .join(HARNESS_MIGRATION_VERSION);
            fs::create_dir_all(&backup_dir)
                .map_err(|error| format!("无法创建迁移备份目录：{error}"))?;
            let backup_path = backup_dir.join("workspace.before-migration.json");
            if !backup_path.exists() {
                fs::copy(&target_workspace_path, &backup_path)
                    .map_err(|error| format!("无法备份当前工作区数据：{error}"))?;
            }
        }

        write_json_atomically(&target_workspace_path, &target_workspace)?;
        imported_sessions =
            copy_missing_tree(&legacy_home.join("sessions"), &target_home.join("sessions"))?;
    }

    let marker = MigrationMarker {
        version: HARNESS_MIGRATION_VERSION,
        migrated_at_unix_seconds: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
        imported_workspaces,
        imported_sessions,
    };
    write_json_atomically(&marker_path, &marker)?;
    log::info!(
        "Harness data migration completed: {imported_workspaces} workspaces, {imported_sessions} session files"
    );
    Ok(())
}

fn read_workspace_storage(path: &Path) -> Result<WorkspaceStorage, String> {
    let data = fs::read(path)
        .map_err(|error| format!("无法读取工作区数据 {}：{error}", path.display()))?;
    serde_json::from_slice(&data)
        .map_err(|error| format!("无法解析工作区数据 {}：{error}", path.display()))
}

fn validate_workspace_storage(storage: &WorkspaceStorage, path: &Path) -> Result<(), String> {
    if storage.unit.name != "workspace" || storage.unit.version != 2 {
        return Err(format!(
            "工作区数据格式不受支持 {}（{} v{}）",
            path.display(),
            storage.unit.name,
            storage.unit.version
        ));
    }
    Ok(())
}

fn merge_workspace_storage(
    target: &mut WorkspaceStorage,
    legacy: &WorkspaceStorage,
) -> Result<usize, String> {
    let mut imported = 0;
    let mut path_index = target
        .tables
        .workspaces
        .iter()
        .map(|(id, workspace)| (workspace.path.clone(), id.clone()))
        .collect::<HashMap<_, _>>();
    let mut target_order = target
        .global
        .workspace_ids
        .iter()
        .cloned()
        .collect::<HashSet<_>>();

    for legacy_id in &legacy.global.workspace_ids {
        let legacy_workspace = legacy
            .tables
            .workspaces
            .get(legacy_id)
            .ok_or_else(|| format!("旧工作区索引引用了不存在的记录：{legacy_id}"))?;

        let target_id = if let Some(existing) = target.tables.workspaces.get(legacy_id) {
            if existing.path != legacy_workspace.path {
                return Err(format!(
                    "工作区标识冲突：{legacy_id} 同时指向 {} 和 {}",
                    existing.path, legacy_workspace.path
                ));
            }
            legacy_id.clone()
        } else if let Some(existing_id) = path_index.get(&legacy_workspace.path) {
            existing_id.clone()
        } else {
            target
                .tables
                .workspaces
                .insert(legacy_id.clone(), legacy_workspace.clone());
            path_index.insert(legacy_workspace.path.clone(), legacy_id.clone());
            imported += 1;
            legacy_id.clone()
        };

        let target_workspace = target
            .tables
            .workspaces
            .get_mut(&target_id)
            .ok_or_else(|| format!("无法定位迁移后的工作区：{target_id}"))?;
        let mut known_sessions = target_workspace
            .session_ids
            .iter()
            .cloned()
            .collect::<HashSet<_>>();
        for session_id in &legacy_workspace.session_ids {
            if known_sessions.insert(session_id.clone()) {
                target_workspace.session_ids.push(session_id.clone());
            }
        }

        if target_order.insert(target_id.clone()) {
            target.global.workspace_ids.push(target_id);
        }
    }

    let mut archived = target
        .global
        .archived_session_ids
        .iter()
        .cloned()
        .collect::<HashSet<_>>();
    for session_id in &legacy.global.archived_session_ids {
        if archived.insert(session_id.clone()) {
            target.global.archived_session_ids.push(session_id.clone());
        }
    }
    target.global.initialized |= legacy.global.initialized;
    Ok(imported)
}

fn copy_missing_tree(source: &Path, destination: &Path) -> Result<usize, String> {
    if !source.exists() {
        return Ok(0);
    }
    fs::create_dir_all(destination)
        .map_err(|error| format!("无法创建会话迁移目录 {}：{error}", destination.display()))?;

    let mut copied = 0;
    for entry in fs::read_dir(source)
        .map_err(|error| format!("无法读取旧会话目录 {}：{error}", source.display()))?
    {
        let entry = entry.map_err(|error| format!("无法读取旧会话条目：{error}"))?;
        let source_path = entry.path();
        let destination_path = destination.join(entry.file_name());
        let file_type = entry
            .file_type()
            .map_err(|error| format!("无法识别旧会话条目 {}：{error}", source_path.display()))?;
        if file_type.is_dir() {
            copied += copy_missing_tree(&source_path, &destination_path)?;
        } else if file_type.is_file() {
            if !destination_path.exists() {
                fs::copy(&source_path, &destination_path).map_err(|error| {
                    format!(
                        "无法迁移会话文件 {} 到 {}：{error}",
                        source_path.display(),
                        destination_path.display()
                    )
                })?;
                copied += 1;
            }
        } else {
            return Err(format!(
                "旧会话目录包含不支持的链接或特殊文件：{}",
                source_path.display()
            ));
        }
    }
    Ok(copied)
}

fn atomic_sidecar_path(path: &Path, suffix: &str) -> Result<PathBuf, String> {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| format!("无效的数据文件路径：{}", path.display()))?;
    Ok(path.with_file_name(format!(".{name}.{suffix}")))
}

fn recover_interrupted_atomic_write(path: &Path) -> Result<(), String> {
    let previous = atomic_sidecar_path(path, "dsh-desktop-previous")?;
    let temporary = atomic_sidecar_path(path, "dsh-desktop-temporary")?;
    if !path.exists() && previous.is_file() {
        fs::rename(&previous, path)
            .map_err(|error| format!("无法恢复迁移前的数据 {}：{error}", path.display()))?;
    } else if path.exists() && previous.exists() {
        fs::remove_file(&previous)
            .map_err(|error| format!("无法清理迁移备份 {}：{error}", previous.display()))?;
    }
    if temporary.exists() {
        fs::remove_file(&temporary)
            .map_err(|error| format!("无法清理迁移临时文件 {}：{error}", temporary.display()))?;
    }
    Ok(())
}

fn write_json_atomically<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("无法创建数据目录 {}：{error}", parent.display()))?;
    }
    recover_interrupted_atomic_write(path)?;
    let temporary = atomic_sidecar_path(path, "dsh-desktop-temporary")?;
    let previous = atomic_sidecar_path(path, "dsh-desktop-previous")?;
    let mut data = serde_json::to_vec_pretty(value)
        .map_err(|error| format!("无法生成迁移数据 {}：{error}", path.display()))?;
    data.push(b'\n');

    let mut file = File::create(&temporary)
        .map_err(|error| format!("无法创建迁移临时文件 {}：{error}", temporary.display()))?;
    file.write_all(&data)
        .and_then(|_| file.sync_all())
        .map_err(|error| format!("无法写入迁移临时文件 {}：{error}", temporary.display()))?;

    let had_previous = path.exists();
    if had_previous {
        fs::rename(path, &previous)
            .map_err(|error| format!("无法暂存现有数据 {}：{error}", path.display()))?;
    }
    if let Err(error) = fs::rename(&temporary, path) {
        if had_previous {
            let _ = fs::rename(&previous, path);
        }
        return Err(format!("无法启用迁移数据 {}：{error}", path.display()));
    }
    if previous.exists() {
        fs::remove_file(&previous)
            .map_err(|error| format!("无法清理迁移临时备份 {}：{error}", previous.display()))?;
    }
    Ok(())
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
        .on_menu_event(|app, event| {
            if event.id() == PROJECT_HOMEPAGE_MENU_ID {
                #[allow(deprecated)]
                if let Err(error) = app.shell().open(PROJECT_HOMEPAGE, None) {
                    log::warn!("failed to open project homepage: {error}");
                }
            }
        })
        .manage(manager.clone())
        .invoke_handler(tauri::generate_handler![backend_status, restart_backend])
        .setup(move |app| {
            #[cfg(target_os = "macos")]
            {
                use tauri::menu::{AboutMetadataBuilder, MenuBuilder, SubmenuBuilder};

                let about = AboutMetadataBuilder::new()
                    .name(Some("DSH Desktop"))
                    .short_version(Some(app.package_info().version.to_string()))
                    .version(Some(format!("Harness {HARNESS_VERSION}")))
                    .copyright(Some("Copyright © 2026 DSH Desktop contributors"))
                    .credits(Some(
                        "非官方 DeepSeek Harness 桌面客户端\n\n内置 Node.js · 无需命令行\n开源项目 · MIT License",
                    ))
                    .icon(app.default_window_icon().cloned())
                    .build();

                let app_menu = SubmenuBuilder::new(app, "DSH Desktop")
                    .about_with_text("关于 DSH Desktop", Some(about))
                    .text(PROJECT_HOMEPAGE_MENU_ID, "项目主页…")
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
    use std::{fs, path::Path, process::Command, time::SystemTime};

    use super::{
        archive_link_stays_inside, ensure_harness_runtime, merge_workspace_storage,
        migrate_legacy_harness_data, read_workspace_storage, readiness_url, safe_archive_path,
        StorageUnit, WorkspaceGlobal, WorkspaceRecord, WorkspaceStorage, WorkspaceTables,
        HARNESS_MIGRATION_VERSION,
    };
    use serde_json::Map;
    use std::collections::BTreeMap;

    fn test_workspace_storage(id: &str, path: &str, sessions: &[&str]) -> WorkspaceStorage {
        let mut workspaces = BTreeMap::new();
        workspaces.insert(
            id.to_owned(),
            WorkspaceRecord {
                path: path.to_owned(),
                title: Path::new(path)
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or(path)
                    .to_owned(),
                session_ids: sessions
                    .iter()
                    .map(|session| (*session).to_owned())
                    .collect(),
                created_at: "2026-08-01T00:00:00.000Z".into(),
                updated_at: "2026-08-02T00:00:00.000Z".into(),
                extra: Map::new(),
            },
        );
        WorkspaceStorage {
            unit: StorageUnit {
                name: "workspace".into(),
                version: 2,
                extra: Map::new(),
            },
            global: WorkspaceGlobal {
                initialized: true,
                workspace_ids: vec![id.to_owned()],
                archived_session_ids: Vec::new(),
                extra: Map::new(),
            },
            tables: WorkspaceTables {
                workspaces,
                extra: Map::new(),
            },
            extra: Map::new(),
        }
    }

    fn unique_test_dir(name: &str) -> std::path::PathBuf {
        let nonce = SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("test clock should be valid")
            .as_nanos();
        std::env::temp_dir().join(format!("dsh-desktop-{name}-{}-{nonce}", std::process::id()))
    }

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
    fn merges_legacy_sessions_into_an_existing_workspace_path() {
        let mut target = test_workspace_storage("new-id", "/project", &["new-session"]);
        let mut legacy =
            test_workspace_storage("legacy-id", "/project", &["legacy-session", "new-session"]);
        legacy
            .global
            .archived_session_ids
            .push("legacy-session".into());

        assert_eq!(
            merge_workspace_storage(&mut target, &legacy).expect("workspace merge should succeed"),
            0
        );
        assert_eq!(target.global.workspace_ids, ["new-id"]);
        assert_eq!(target.tables.workspaces.len(), 1);
        assert_eq!(
            target.tables.workspaces["new-id"].session_ids,
            ["new-session", "legacy-session"]
        );
        assert_eq!(target.global.archived_session_ids, ["legacy-session"]);
    }

    #[test]
    fn migrates_legacy_workspaces_and_preserves_current_rc8_data() {
        let root = unique_test_dir("migration");
        let legacy_home = root.join("harness");
        let target_home = legacy_home.join("rc8");
        let legacy_workspace_path = legacy_home.join("storages/workspace.json");
        let target_workspace_path = target_home.join("storages/workspace.json");
        fs::create_dir_all(legacy_workspace_path.parent().unwrap())
            .expect("legacy storage directory should be created");
        fs::create_dir_all(target_workspace_path.parent().unwrap())
            .expect("target storage directory should be created");
        fs::write(
            &legacy_workspace_path,
            serde_json::to_vec_pretty(&test_workspace_storage(
                "legacy-id",
                "/legacy-project",
                &["legacy-session"],
            ))
            .unwrap(),
        )
        .unwrap();
        fs::write(
            &target_workspace_path,
            serde_json::to_vec_pretty(&test_workspace_storage(
                "current-id",
                "/current-project",
                &["current-session"],
            ))
            .unwrap(),
        )
        .unwrap();
        let legacy_session =
            legacy_home.join("sessions/legacy-project/legacy-session/session.jsonl.zstd");
        fs::create_dir_all(legacy_session.parent().unwrap()).unwrap();
        fs::write(&legacy_session, b"legacy-session-log").unwrap();
        let cache_path = target_home.join("storages/session_projcache.json");
        fs::write(&cache_path, b"current-cache-must-not-be-overwritten").unwrap();

        migrate_legacy_harness_data(&legacy_home, &target_home)
            .expect("legacy Harness data should migrate");

        let merged = read_workspace_storage(&target_workspace_path).unwrap();
        assert_eq!(merged.global.workspace_ids, ["current-id", "legacy-id"]);
        assert_eq!(merged.tables.workspaces.len(), 2);
        assert_eq!(
            fs::read(target_home.join("sessions/legacy-project/legacy-session/session.jsonl.zstd"))
                .unwrap(),
            b"legacy-session-log"
        );
        assert_eq!(
            fs::read(&cache_path).unwrap(),
            b"current-cache-must-not-be-overwritten"
        );
        assert!(legacy_home
            .join(format!(
                "migration-backups/{HARNESS_MIGRATION_VERSION}/workspace.before-migration.json"
            ))
            .is_file());
        assert!(target_home
            .join(format!(
                ".dsh-desktop-migration-{HARNESS_MIGRATION_VERSION}.json"
            ))
            .is_file());

        // A completed migration is intentionally one-shot. Re-running must not
        // duplicate project or session records.
        migrate_legacy_harness_data(&legacy_home, &target_home).unwrap();
        let repeated = read_workspace_storage(&target_workspace_path).unwrap();
        assert_eq!(repeated.global.workspace_ids, ["current-id", "legacy-id"]);

        fs::remove_dir_all(root).expect("migration test directory should be removable");
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
            "0.1.1-rc.1"
        );
        fs::remove_dir_all(&data_dir).expect("temporary runtime should be removable");
    }
}
