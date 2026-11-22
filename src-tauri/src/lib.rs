mod commands;
mod git;
mod library;
mod verification;
mod worktree;

pub use git::run_gc_worker;
pub use library::{
    BookView, ChangeKind, CleanupReport, FileHistory, LibraryEngine, LibraryTransition,
    LibraryView, MoveConfirmation, MoveItem, MoveRequest, MoveSelection, MoveStatus,
    MoveTransition, NovelFormat, NovelScanRequest, ReadRequest, ReadTransition, RestoreRequest,
    ScanControl, ScanPhase, ScanPlan, ScanResult, ScanReviewEdit, SwitchPreview,
};
pub use verification::{verify_runtime, EngineReport};

pub fn run() {
    let builder = tauri::Builder::default()
        .manage(commands::NovelScans::default())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init());
    #[cfg(target_os = "macos")]
    let builder = builder
        .menu(|app| {
            use tauri::menu::{Menu, MenuItem, MenuItemKind, HELP_SUBMENU_ID};

            let menu = Menu::default(app)?;
            if let Some(MenuItemKind::Submenu(help)) = menu.get(HELP_SUBMENU_ID) {
                help.set_text("帮助")?;
                help.append(&MenuItem::with_id(app, "diagnostics", "诊断", true, None::<&str>)?)?;
            }
            Ok(menu)
        })
        .on_menu_event(|app, event| {
            use tauri::Emitter;

            if event.id().as_ref() == "diagnostics" {
                let _ = app.emit_to("main", "open-diagnostics", ());
            }
        });
    builder
        .invoke_handler(tauri::generate_handler![
            commands::verify_git_engine,
            commands::choose_library_directory,
            commands::resume_library,
            commands::adopt_library,
            commands::create_library,
            commands::create_branch,
            commands::set_default_branch,
            commands::switch_branch,
            commands::create_book,
            commands::move_files,
            commands::read_directory,
            commands::start_novel_scan,
            commands::stop_novel_scan,
            commands::file_history,
            commands::restore_version,
            commands::collect_garbage,
            commands::open_library_path,
        ])
        .run(tauri::generate_context!())
        .expect("运行 FileManagementMaster 失败");
}
