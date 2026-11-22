#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    let mut args = std::env::args_os().skip(1);
    if args.next().as_deref() == Some(std::ffi::OsStr::new("--filemanagement-gc")) {
        if let (Some(runtime), Some(directory)) = (args.next(), args.next()) {
            match filemanagement_master::run_gc_worker(runtime.as_ref(), directory.as_ref()) {
                Ok(()) => return,
                Err(error) => eprintln!("{error:#}"),
            }
        }
        std::process::exit(1);
    }
    filemanagement_master::run();
}
