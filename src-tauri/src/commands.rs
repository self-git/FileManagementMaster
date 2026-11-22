use std::{
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use tauri::{AppHandle, Manager};
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_opener::OpenerExt;

use crate::{
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
