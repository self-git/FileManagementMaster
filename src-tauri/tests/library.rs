use std::{env, fs, path::PathBuf};

use filemanagement_master::{ChangeKind, LibraryEngine};

fn engine() -> LibraryEngine {
    let runtime = env::var_os("FILEMANAGEMENT_BUNDLED_GIT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources/git"));
    LibraryEngine::new(&runtime).unwrap()
}

#[test]
fn desktop_view_opens_only_paths_in_the_current_managed_branch() {
    let engine = engine();
    let sandbox = tempfile::tempdir().unwrap();
    let root = sandbox.path().join("管理 目录");
    let folder = root.join("01 文稿");
    let relative = "01 文稿/《远航》 1-24 作者：舟.TXT";
    fs::create_dir_all(&folder).unwrap();
    fs::write(root.join(relative), b"one").unwrap();
    let adopted = engine.adopt_or_open(&root).unwrap();
    let view = serde_json::to_value(&adopted.library).unwrap();
    assert_eq!(view["fileDetails"][relative]["bytes"], 3);
    assert!(view["fileDetails"][relative]["modifiedAt"].is_u64());
    assert_eq!(view["novels"][relative]["format"], "txt");
    assert_eq!(view["novels"][relative]["author"], "舟");
    assert!(view["novels"][relative]["progress"].is_string());
    assert_eq!(
        engine.path_for_open(&root, "常看", relative).unwrap(),
        root.join(relative).canonicalize().unwrap()
    );
    assert_eq!(
        engine.path_for_open(&root, "常看", "01 文稿").unwrap(),
        folder.canonicalize().unwrap()
    );
    assert!(engine.path_for_open(&root, "常看", ".git").is_err());
    assert!(engine.path_for_open(&root, "常看", "../同级目录").is_err());

    engine.create_branch(&root, "资料").unwrap();
    engine.switch(&root, "资料", None).unwrap();
    assert!(engine.path_for_open(&root, "常看", relative).is_err());
    engine.switch(&root, "常看", None).unwrap();

    #[cfg(unix)]
    {
        let outside = sandbox.path().join("其他文件.txt");
        fs::write(&outside, b"outside").unwrap();
        fs::remove_file(root.join(relative)).unwrap();
        std::os::unix::fs::symlink(outside, root.join(relative)).unwrap();
        assert!(engine.path_for_open(&root, "常看", relative).is_err());
    }
}

#[test]
fn adoption_saves_external_changes_and_reopens_default_branch() {
    let engine = engine();
    let sandbox = tempfile::tempdir().unwrap();
    let root = sandbox.path().join("原有书库");
    fs::create_dir(&root).unwrap();
    fs::create_dir(root.join("图书")).unwrap();
    let original = root.join("图书").join("甲.txt");
    fs::write(&original, "初版\r\n").unwrap();

    let adopted = engine.adopt_or_open(&root).unwrap();
    assert_eq!(adopted.library.directory, root.canonicalize().unwrap());
    assert_eq!(adopted.library.current_branch, "常看");
    assert_eq!(adopted.library.files, ["图书/甲.txt"]);
    assert_eq!(fs::read(&original).unwrap(), "初版\r\n".as_bytes());

    engine.create_branch(&root, "收藏").unwrap();
    assert_eq!(
        engine.set_default(&root, "收藏").unwrap().current_branch,
        "常看"
    );
    fs::write(&original, "外部修改稿\r\n").unwrap();
    fs::write(root.join("新文件.txt"), "新增").unwrap();

    let pending = engine.open_default(&root).unwrap().pending.unwrap();
    assert_eq!(pending.source_branch, "常看");
    assert_eq!(pending.target_branch, "收藏");
    assert!(pending
        .changes
        .iter()
        .any(|c| c.kind == ChangeKind::Modified && c.path == "图书/甲.txt"));
    assert!(pending
        .changes
        .iter()
        .any(|c| c.kind == ChangeKind::Added && c.path == "新文件.txt"));

    // 第二次外部修改使之前的预览过期，确认仍停留在原分支。
    fs::write(&original, "外部修改第二稿\r\n").unwrap();
    let fresh = engine.switch(&root, "收藏", Some(&pending.token)).unwrap();
    assert_eq!(fresh.library.current_branch, "常看");
    let fresh = fresh.pending.unwrap();
    assert_ne!(fresh.token, pending.token);
    let switched = engine.switch(&root, "收藏", Some(&fresh.token)).unwrap();
    assert_eq!(switched.library.current_branch, "收藏");
    assert_eq!(switched.library.directory, adopted.library.directory);
    assert!(!original.exists());
    assert!(!root.join("新文件.txt").exists());

    fs::write(root.join("收藏.txt"), "收藏分支内容").unwrap();
    engine.set_default(&root, "常看").unwrap();
    let reopened = engine.open_default(&root).unwrap();
    assert_eq!(reopened.library.current_branch, "收藏");
    let pending = reopened.pending.unwrap();
    assert_eq!(pending.target_branch, "常看");
    let main = engine.switch(&root, "常看", Some(&pending.token)).unwrap();
    assert_eq!(main.library.files, ["图书/甲.txt", "新文件.txt"]);
    assert_eq!(fs::read(original).unwrap(), "外部修改第二稿\r\n".as_bytes());
    assert!(!root.join("收藏.txt").exists());
    let collected = engine.switch(&root, "收藏", None).unwrap();
    assert_eq!(collected.library.files, ["收藏.txt"]);
    assert_eq!(
        fs::read_to_string(root.join("收藏.txt")).unwrap(),
        "收藏分支内容"
    );
}

#[test]
fn independent_child_does_not_join_parent_and_conflict_restores_parent_files() {
    let engine = engine();
    let sandbox = tempfile::tempdir().unwrap();
    let root = sandbox.path().join("书库");
    fs::create_dir(&root).unwrap();
    fs::write(root.join("原分支.txt"), "保留").unwrap();
    engine.adopt_or_open(&root).unwrap();

    engine.create_branch(&root, "备用").unwrap();
    engine.switch(&root, "备用", None).unwrap();
    fs::write(root.join("子书库"), "备用分支的同名文件").unwrap();
    let pending = engine.switch(&root, "常看", None).unwrap().pending.unwrap();
    engine.switch(&root, "常看", Some(&pending.token)).unwrap();

    let child = engine.create(&root, "子书库").unwrap();
    assert!(child.library.files.is_empty());
    assert!(child.library.directory.join(".git").is_dir());
    assert!(root.join(".git").is_dir());

    let failure = engine.switch(&root, "备用", None);
    assert!(failure.is_err());
    assert_eq!(fs::read_to_string(root.join("原分支.txt")).unwrap(), "保留");
    assert!(child.library.directory.join(".git").is_dir());
    assert!(!root.join(".git/filemanagement/pending.json").exists());
    assert_eq!(
        engine.open_default(&root).unwrap().library.current_branch,
        "常看"
    );
}
