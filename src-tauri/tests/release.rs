use std::{
    env, fs,
    io::{self, Write},
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    sync::mpsc::{self, RecvTimeoutError},
    thread,
    time::{Duration, Instant},
};

use filemanagement_master::{
    LibraryEngine, MoveConfirmation, MoveRequest, MoveSelection, MoveStatus, ReadRequest,
    RestoreRequest,
};
use serde_json::{json, Value};

const LARGE_BYTES: u64 = 128 * 1024 * 1024;

fn git(runtime: &Path, root: &Path, args: &[&str], input: Option<&Path>) -> Output {
    let executable = if cfg!(windows) {
        "ucrt64/bin/git.exe"
    } else {
        "bin/git"
    };
    let mut command = Command::new(runtime.join(executable));
    command
        .current_dir(root)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env(
            "GIT_CONFIG_GLOBAL",
            if cfg!(windows) { "NUL" } else { "/dev/null" },
        )
        .args(["-c", "core.longpaths=true"])
        .args(args);
    if let Some(input) = input {
        command.stdin(Stdio::from(fs::File::open(input).unwrap()));
    }
    let result = command.output().unwrap();
    assert!(
        result.status.success(),
        "{args:?} {}",
        String::from_utf8_lossy(&result.stderr)
    );
    result
}

fn large_file(root: &Path, path: &str, mut state: u64) {
    let path = root.join(path);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut file = fs::File::create(path).unwrap();
    let mut block = vec![0_u8; 1024 * 1024];
    for _ in 0..(LARGE_BYTES / block.len() as u64) {
        for bytes in block.chunks_exact_mut(8) {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            bytes.copy_from_slice(&state.to_le_bytes());
        }
        file.write_all(&block).unwrap();
    }
    file.sync_all().unwrap();
}

#[derive(Clone, Copy, Default)]
struct Occupancy {
    logical: u64,
    #[cfg(unix)]
    allocated: u64,
}

fn occupancy(path: &Path) -> io::Result<Occupancy> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Occupancy::default()),
        Err(error) => return Err(error),
    };
    if metadata.is_dir() {
        let mut children = match fs::read_dir(path) {
            Ok(children) => children,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(Occupancy::default())
            }
            Err(error) => return Err(error),
        };
        children.try_fold(Occupancy::default(), |mut total, child| {
            let usage = occupancy(&child?.path())?;
            total.logical += usage.logical;
            #[cfg(unix)]
            {
                total.allocated += usage.allocated;
            }
            Ok(total)
        })
    } else {
        #[cfg(unix)]
        use std::os::unix::fs::MetadataExt;
        Ok(Occupancy {
            logical: metadata.len(),
            #[cfg(unix)]
            allocated: metadata.blocks() * 512,
        })
    }
}

#[test]
fn installed_runtime_preserves_long_path_and_large_versions() {
    let runtime = env::var_os("FILEMANAGEMENT_BUNDLED_GIT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources/git"));
    let worker = env::var_os("FILEMANAGEMENT_GC_WORKER")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_BIN_EXE_filemanagement-master")));
    let engine = LibraryEngine::new(&runtime).unwrap();
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("发行验收 书库");
    let incoming = temp.path().join("发行验收 下载");
    let deep = (1..=6)
        .map(|level| format!("第 {level} 层 {}", "long-path-".repeat(5)))
        .collect::<Vec<_>>()
        .join("/");
    let old = format!("{deep}/《远航》 1-111 作者：舟.EPUB");
    let next = format!("{deep}/《远航》 1-120 作者：舟.epub");
    assert!(old.encode_utf16().count() > 260);
    large_file(&root, &old, 0xca75_14c2);
    large_file(&incoming, &next, 0x58c3_a102);
    let hash = |file: &Path| {
        git(
            &runtime,
            temp.path(),
            &["hash-object", "--stdin"],
            Some(file),
        )
        .stdout
    };
    let old_oid = hash(&root.join(&old));
    let new_oid = hash(&incoming.join(&next));
    assert_ne!(old_oid, new_oid);
    assert_eq!(fs::metadata(root.join(&old)).unwrap().len(), LARGE_BYTES);

    let initial = occupancy(&root).unwrap();
    let monitor_root = root.clone();
    let (finished, samples) = mpsc::channel();
    let monitor = thread::spawn(move || {
        let mut peak = initial;
        loop {
            if let Ok(usage) = occupancy(&monitor_root) {
                peak.logical = peak.logical.max(usage.logical);
                #[cfg(unix)]
                {
                    peak.allocated = peak.allocated.max(usage.allocated);
                }
            }
            match samples.recv_timeout(Duration::from_millis(20)) {
                Err(RecvTimeoutError::Timeout) => continue,
                _ => return peak,
            }
        }
    });

    let start = Instant::now();
    engine.adopt_or_open(&root).unwrap();
    let adopted = start.elapsed();
    engine.create_branch(&root, "收藏").unwrap();
    let mut move_request = MoveRequest {
        source_branch: "常看".to_owned(),
        target_branch: "收藏".to_owned(),
        selection: MoveSelection::Files {
            paths: vec![old.clone()],
        },
        confirmation: None,
    };
    let MoveStatus::Preview { token, items } =
        engine.move_files(&root, &move_request).unwrap().status
    else {
        panic!("应预览长路径移动")
    };
    assert!(items[0].conflict.is_none());
    move_request.confirmation = Some(MoveConfirmation::Move {
        token,
        destinations: Default::default(),
    });
    assert!(matches!(
        engine.move_files(&root, &move_request).unwrap().status,
        MoveStatus::Moved
    ));
    assert!(!root.join(&old).exists());
    let moved = start.elapsed() - adopted;

    let source = json!({"kind":"directory","directory":incoming});
    let read = |confirmation: Value| {
        let request: ReadRequest = serde_json::from_value(json!({
            "source":source, "formats":["epub"], "confirmation":confirmation,
        }))
        .unwrap();
        serde_json::to_value(engine.read_directory(&root, &request).unwrap()).unwrap()
    };
    let preview = read(Value::Null);
    let item = &preview["status"]["preview"]["items"][0];
    assert_eq!(item["path"], next);
    assert_eq!(item["books"][0]["locations"][0]["branch"], "收藏");
    assert_eq!(item["books"][0]["locations"][0]["relation"], "newer");
    let imported = read(json!({
        "kind":"apply", "token":preview["status"]["preview"]["token"],
        "selections":[{"path":next, "book":item["preferredBook"], "branch":"收藏",
            "action":"keep", "destination":next, "replaces":[old]}],
    }));
    assert_eq!(imported["status"]["kind"], "applied");
    assert_eq!(imported["library"]["currentBranch"], "常看");
    assert!(!root.join(&next).exists());
    let imported = start.elapsed() - adopted - moved;

    let switching = Instant::now();
    engine.switch(&root, "收藏", None).unwrap();
    assert_eq!(hash(&root.join(&next)), new_oid);
    assert_eq!(hash(&incoming.join(&next)), new_oid);
    let switched = switching.elapsed();

    let restoring = Instant::now();
    let history = serde_json::to_value(engine.file_history(&root, "收藏", &next).unwrap()).unwrap();
    assert_eq!(history["versions"][0]["savedPath"], old);
    let request: RestoreRequest = serde_json::from_value(json!({
        "branch":"收藏", "path":next, "token":history["token"], "order":history["versions"][0]["order"],
    })).unwrap();
    engine.restore_version(&root, &request).unwrap();
    assert_eq!(hash(&root.join(&old)), old_oid);
    assert!(!root.join(&next).exists());
    let restored = restoring.elapsed();

    let reclaiming = Instant::now();
    let cleanup = engine.collect_garbage(&root, &worker).unwrap().unwrap();
    let reclaimed = reclaiming.elapsed();
    git(&runtime, &root, &["fsck", "--full", "--strict"], None);
    let final_size = occupancy(&root).unwrap();
    finished.send(()).unwrap();
    let peak = monitor.join().unwrap();
    println!(
        "发行样本每版 {LARGE_BYTES} B，相对路径 {} 字节，建库 {:.2?}，移出 {:.2?}，追更 {:.2?}，切换及校验 {:.2?}，恢复及校验 {:.2?}，回收 {:.2?}",
        old.len(), adopted, moved, imported, switched, restored, reclaimed,
    );
    println!(
        "书库文件字节 基线 {}，采样峰值 {}，最终 {}。对象库 {} → {} B",
        initial.logical,
        peak.logical,
        final_size.logical,
        cleanup.before_bytes,
        cleanup.after_bytes,
    );
    #[cfg(unix)]
    println!(
        "书库文件占用块字节 基线 {}，采样峰值 {}，最终 {}",
        initial.allocated, peak.allocated, final_size.allocated
    );
}
