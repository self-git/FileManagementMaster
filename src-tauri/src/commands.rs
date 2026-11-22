use std::{
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use anyhow::{Context, Result};
use tauri::{ipc::Channel, AppHandle, Manager, State};
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_opener::OpenerExt;
use serde::{Deserialize, Serialize};

use crate::{
    library::{NovelScanRequest, ScanControl, ScanPhase, ScanPlan, ScanResult, ScanReviewEdit, ScanStop, ScanStopped},
    verify_runtime, CleanupReport, EngineReport, FileHistory, LibraryEngine, LibraryTransition,
    LibraryView, MoveRequest, MoveTransition, ReadRequest, ReadTransition, RestoreRequest,
};

fn runtime_dir(app: &AppHandle) -> Result<PathBuf, String> {
    // 只从 Tauri 应用资源目录加载随包 Git。
    Ok(app
        .path()
        .resource_dir()
        .map_err(|err| err.to_string())?
        .join("git"))
}

async fn task<T, F>(operation: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T> + Send + 'static,
{
    tauri::async_runtime::spawn_blocking(operation)
        .await
        .map_err(|err| err.to_string())?
        .map_err(|err| format!("{err:#}"))
}

async fn library_task<T, F>(app: &AppHandle, operation: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce(LibraryEngine) -> Result<T> + Send + 'static,
{
    let runtime = runtime_dir(app)?;
    task(move || operation(LibraryEngine::new(&runtime)?)).await
}

fn preference_path(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(app
        .path()
        .app_data_dir()
        .map_err(|err| err.to_string())?
        .join("last-library.json"))
}

fn remember_library(preference: &Path, directory: &Path) -> Result<()> {
    let parent = preference.parent().context("缺少应用数据目录")?;
    fs::create_dir_all(parent)?;
    let mut staged = tempfile::NamedTempFile::new_in(parent)?;
    serde_json::to_writer(&mut staged, directory)?;
    staged.flush()?;
    staged.as_file().sync_all()?;
    staged.persist(preference)?;
    Ok(())
}

#[tauri::command]
pub async fn verify_git_engine(app: AppHandle) -> Result<EngineReport, String> {
    let runtime = runtime_dir(&app)?;
    task(move || verify_runtime(&runtime)).await
}

#[tauri::command]
pub async fn choose_library_directory(
    app: AppHandle,
    title: Option<String>,
) -> Result<Option<PathBuf>, String> {
    task(move || {
        let mut picker = app
            .dialog()
            .file()
            .set_title(title.as_deref().unwrap_or("选择管理目录"));
        if let Some(window) = app.get_webview_window("main") {
            picker = picker.set_parent(&window);
        }
        Ok(picker
            .blocking_pick_folder()
            .map(|path| path.into_path())
            .transpose()?)
    })
    .await
}

#[tauri::command]
pub async fn resume_library(app: AppHandle) -> Result<Option<LibraryTransition>, String> {
    let preference = preference_path(&app)?;
    library_task(&app, move |engine| {
        let file = match fs::File::open(preference) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        let directory: PathBuf = serde_json::from_reader(file)?;
        Ok(Some(engine.open_default(&directory)?))
    })
    .await
}

#[tauri::command]
pub async fn adopt_library(
    app: AppHandle,
    directory: PathBuf,
) -> Result<LibraryTransition, String> {
    let preference = preference_path(&app)?;
    library_task(&app, move |engine| {
        let result = engine.adopt_or_open(&directory)?;
        remember_library(&preference, &result.library.directory)?;
        Ok(result)
    })
    .await
}

#[tauri::command]
pub async fn create_library(
    app: AppHandle,
    parent: PathBuf,
    name: String,
) -> Result<LibraryTransition, String> {
    let preference = preference_path(&app)?;
    library_task(&app, move |engine| {
        let result = engine.create(&parent, &name)?;
        remember_library(&preference, &result.library.directory)?;
        Ok(result)
    })
    .await
}

#[tauri::command]
pub async fn create_branch(
    app: AppHandle,
    directory: PathBuf,
    name: String,
) -> Result<LibraryView, String> {
    library_task(&app, move |engine| engine.create_branch(&directory, &name)).await
}

#[tauri::command]
pub async fn set_default_branch(
    app: AppHandle,
    directory: PathBuf,
    name: String,
) -> Result<LibraryView, String> {
    library_task(&app, move |engine| engine.set_default(&directory, &name)).await
}

#[tauri::command]
pub async fn switch_branch(
    app: AppHandle,
    directory: PathBuf,
    name: String,
    token: Option<String>,
) -> Result<LibraryTransition, String> {
    library_task(&app, move |engine| {
        engine.switch(&directory, &name, token.as_deref())
    })
    .await
}

#[tauri::command]
pub async fn create_book(
    app: AppHandle,
    directory: PathBuf,
    source_branch: String,
    title: String,
    paths: Vec<String>,
) -> Result<LibraryView, String> {
    library_task(&app, move |engine| {
        engine.create_book(&directory, &source_branch, &title, &paths)
    })
    .await
}

#[tauri::command]
pub async fn move_files(
    app: AppHandle,
    directory: PathBuf,
    request: MoveRequest,
) -> Result<MoveTransition, String> {
    library_task(&app, move |engine| engine.move_files(&directory, &request)).await
}

#[tauri::command]
pub async fn read_directory(
    app: AppHandle,
    directory: PathBuf,
    request: ReadRequest,
) -> Result<ReadTransition, String> {
    library_task(&app, move |engine| {
        engine.read_directory(&directory, &request)
    })
    .await
}

#[derive(Default)]
pub(crate) struct NovelScans(Mutex<ScanTasks>);

#[derive(Default)]
struct ScanTasks {
    next: u64,
    active: Option<(u64, Arc<ScanControl>)>,
    result: Option<(u64, Arc<ScanPlan>)>,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub(crate) enum ScanAction {
    Scan { scope: NovelScanRequest },
    Review { id: u64, edits: Vec<ScanReviewEdit> },
}

#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub(crate) enum ScanEvent {
    Progress { phase: ScanPhase },
    Ready { result: ScanResult },
    Stopped,
    Failed { message: String },
}

#[tauri::command]
pub(crate) fn start_novel_scan(
    app: AppHandle,
    action: ScanAction,
    on_event: Channel<ScanEvent>,
) -> Result<u64, String> {
    let runtime = runtime_dir(&app)?;
    let control = Arc::new(ScanControl::default());
    let (id, plan) = {
        let scans = app.state::<NovelScans>();
        let mut tasks = scans.0.lock().map_err(|_| "无法读取扫描任务")?;
        if tasks.active.is_some() {
            return Err("已有扫描任务正在处理".to_owned());
        }
        let plan = match &action {
            ScanAction::Scan { .. } => {
                tasks.result = None;
                None
            }
            ScanAction::Review { id, .. } => Some(Arc::clone(
                &tasks.result.as_ref().filter(|(saved, _)| saved == id)
                    .ok_or("扫描结果已变化，请重新扫描")?.1
            )),
        };
        tasks.next += 1;
        let id = tasks.next;
        tasks.active = Some((id, Arc::clone(&control)));
        (id, plan)
    };
    tauri::async_runtime::spawn(async move {
        let progress_channel = on_event.clone();
        let worker_control = Arc::clone(&control);
        let result = tauri::async_runtime::spawn_blocking(move || -> Result<_> {
            let engine = LibraryEngine::new(&runtime)?;
            let progress = |phase| {
                let _ = progress_channel.send(ScanEvent::Progress { phase });
            };
            match action {
                ScanAction::Scan { scope } => engine.scan_novels(&scope, &worker_control, progress),
                ScanAction::Review { edits, .. } => engine.review_novel_scan(
                    plan.as_deref().context("扫描结果已变化，请重新扫描")?, &edits,
                    &worker_control, progress,
                ),
            }
        }).await;
        let stopped = control.finish();
        let scans = app.state::<NovelScans>();
        let mut tasks = scans.0.lock().expect("扫描任务状态");
        tasks.active = None;
        let event = match result {
            Ok(Ok(_)) if stopped => ScanEvent::Stopped,
            Ok(Ok((result, plan))) => {
                tasks.result = Some((id, Arc::new(plan)));
                ScanEvent::Ready { result }
            }
            Ok(Err(error)) if error.is::<ScanStopped>() => ScanEvent::Stopped,
            Ok(Err(error)) => ScanEvent::Failed { message: format!("{error:#}") },
            Err(error) => ScanEvent::Failed { message: error.to_string() },
        };
        drop(tasks);
        let _ = on_event.send(event);
    });
    Ok(id)
}

#[tauri::command]
pub(crate) fn stop_novel_scan(scans: State<'_, NovelScans>, id: u64) -> Result<ScanStop, String> {
    let tasks = scans.0.lock().map_err(|_| "无法读取扫描任务")?;
    Ok(tasks.active.as_ref().filter(|(task, _)| *task == id)
        .map_or(ScanStop::Finished, |(_, control)| control.request_stop()))
}

#[tauri::command]
pub async fn file_history(
    app: AppHandle,
    directory: PathBuf,
    branch: String,
    path: String,
) -> Result<FileHistory, String> {
    library_task(&app, move |engine| {
        engine.file_history(&directory, &branch, &path)
    })
    .await
}

#[tauri::command]
pub async fn restore_version(
    app: AppHandle,
    directory: PathBuf,
    request: RestoreRequest,
) -> Result<LibraryView, String> {
    library_task(&app, move |engine| {
        engine.restore_version(&directory, &request)
    })
    .await
}

#[tauri::command]
pub async fn collect_garbage(
    app: AppHandle,
    directory: PathBuf,
) -> Result<Option<CleanupReport>, String> {
    let worker = std::env::current_exe().map_err(|error| error.to_string())?;
    library_task(&app, move |engine| {
        engine.collect_garbage(&directory, &worker)
    })
    .await
}

#[tauri::command]
pub async fn open_library_path(
    app: AppHandle,
    directory: PathBuf,
    branch: String,
    relative: Option<String>,
    reveal: bool,
) -> Result<(), String> {
    let path = library_task(&app, move |engine| match relative {
        Some(path) => engine.path_for_open(&directory, &branch, &path),
        None => engine.directory_for_open(&directory),
    })
    .await?;
    let opener = app.opener();
    if reveal {
        opener.reveal_item_in_dir(path)
    } else {
        opener.open_path(path.to_string_lossy().into_owned(), None::<String>)
    }
    .map_err(|err| err.to_string())
}
