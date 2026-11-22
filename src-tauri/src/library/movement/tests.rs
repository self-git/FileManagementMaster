use std::{env, process::Command};

use super::*;

const SOURCE: &str = "常看";
const TARGET: &str = "收藏";
const REFS: [&str; 3] = ["refs/heads/常看", "refs/heads/收藏", STATE_REF];
const INTERRUPT_ROOT: &str = "FILEMANAGEMENT_TEST_MOVE_ROOT";
const INTERRUPT_AT: &str = "FILEMANAGEMENT_TEST_MOVE_BOUNDARY";
const FIRST: &str = "图书/01 甲.txt";
const SECOND: &str = "图书/02 乙.epub";

fn engine() -> LibraryEngine {
    let runtime = env::var_os("FILEMANAGEMENT_BUNDLED_GIT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources/git"));
    LibraryEngine::new(&runtime).unwrap()
}

fn put(root: &Path, path: &str, content: &[u8]) {
    let path = root.join(path);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}

fn request(source: &str, target: &str, selection: MoveSelection) -> MoveRequest {
    MoveRequest {
        source_branch: source.to_owned(),
        target_branch: target.to_owned(),
        selection,
        confirmation: None,
    }
}

fn files(source: &str, target: &str, paths: &[&str]) -> MoveRequest {
    request(
        source,
        target,
        MoveSelection::Files {
            paths: paths.iter().map(|path| (*path).to_owned()).collect(),
        },
    )
}

fn preview(engine: &LibraryEngine, root: &Path, request: &MoveRequest) -> (String, Vec<MoveItem>) {
    let MoveStatus::Preview { token, items } = engine.move_files(root, request).unwrap().status
    else {
        panic!("应返回移动预览");
    };
    (token, items)
}

fn confirm<const N: usize>(token: &str, renames: [(&str, &str); N]) -> Option<MoveConfirmation> {
    Some(MoveConfirmation::Move {
        token: token.to_owned(),
        destinations: renames
            .into_iter()
            .map(|(old, new)| (old.to_owned(), new.to_owned()))
            .collect(),
    })
}

fn references(repo: &Repository<'_>) -> [String; 3] {
    REFS.map(|name| repo.reference(name).unwrap().unwrap())
}

fn assert_clean(root: &Path) {
    let control = root.join(".git/filemanagement");
    assert!(!control.join("pending.json").exists());
    assert!(!root.join(".git/reftable/tables.list.lock").exists());
    assert!(fs::read_dir(control).unwrap().all(|entry| {
        !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with("switch-")
    }));
}

#[test]
fn equivalent_unicode_names_are_reported_as_collisions() {
    let file = BlobFile {
        oid: "same".to_owned(),
        executable: false,
    };
    let mut paths = TargetPaths::default();
    paths.insert("Café/原稿.txt", &file);
    assert_eq!(
        paths.conflict("Cafe\u{301}/移入.txt", Some(&file)),
        Some("Café/原稿.txt")
    );
    assert_eq!(paths.conflict("Café/移入.txt", Some(&file)), None);
}

#[test]
fn move_book_then_one_file_preserves_its_id_and_history() {
    let engine = engine();
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let installments = ["海图.epub", "海图 卷一.txt", "海图 卷二.txt"];
    for path in installments {
        put(root, path, b"first\r\n");
    }
    put(root, installments[2], b"second volume\r\n");
    put(root, "留在原分支.txt", b"untouched");
    engine.adopt_or_open(root).unwrap();
    engine.create_branch(root, TARGET).unwrap();
    let grouped = engine
        .create_book(root, SOURCE, "海图", &installments.map(str::to_owned))
        .unwrap();
    let book = grouped.books[0].id;
    let repo = engine.repository(root).unwrap();
    let refs = references(&repo);
    let previous = State::load(&repo).unwrap();
    let installment = previous.catalog.branch(SOURCE).unwrap().entries[installments[1]].clone();

    put(root, installments[1], b"second\r\n");
    let mut movement = request(SOURCE, TARGET, MoveSelection::Book { id: book });
    let MoveStatus::SaveExternal { token, changes } =
        engine.move_files(root, &movement).unwrap().status
    else {
        panic!("需要先确认保存");
    };
    assert_eq!(changes[0].path, installments[1]);
    assert_eq!(references(&repo), refs);
    movement.confirmation = Some(MoveConfirmation::SaveExternal { token });
    assert!(matches!(
        engine.move_files(root, &movement).unwrap().status,
        MoveStatus::SavedExternal
    ));
    assert_eq!(references(&repo)[1], refs[1]);

    movement.confirmation = None;
    let (token, items) = preview(&engine, root, &movement);
    assert_eq!(
        items
            .iter()
            .map(|item| item.path.as_str())
            .collect::<BTreeSet<_>>(),
        installments.into_iter().collect()
    );
    engine.set_default(root, TARGET).unwrap();
    let before = references(&repo);
    movement.confirmation = confirm(&token, []);
    let (refreshed, _) = preview(&engine, root, &movement);
    assert_ne!(refreshed, token);
    assert_eq!(references(&repo), before);
    movement.confirmation = confirm(&refreshed, []);

    let result = engine.move_files(root, &movement).unwrap();
    assert!(matches!(result.status, MoveStatus::Moved));
    assert_eq!(result.library.current_branch, SOURCE);
    assert_eq!(result.library.files, ["留在原分支.txt"]);
    assert!(result.library.books.is_empty());
    assert!(installments.iter().all(|path| !root.join(path).exists()));
    let after = references(&repo);
    assert!(after.iter().zip(before).all(|(new, old)| *new != old));
    let state = State::load(&repo).unwrap();
    let target = state.catalog.branch(TARGET).unwrap();
    assert_eq!(
        worktree::snapshot_files(&repo, &after[1]).unwrap(),
        target.files()
    );
    assert_eq!(target.entries[installments[1]].id, installment.id);
    let version = state.catalog.versions[&installment.id]
        .iter()
        .find(|version| version.file == installment.file)
        .unwrap();
    let history = format!("{}:history/{}/{}", state.oid, installment.id, version.order);
    assert_eq!(repo.data(&["show", &history]).unwrap(), b"first\r\n");
    assert_eq!(
        repo.run(&["write-tree"]).unwrap(),
        repo.run(&["rev-parse", "HEAD^{tree}"]).unwrap()
    );
    assert_clean(root);

    let target = engine.switch(root, TARGET, None).unwrap().library;
    assert_eq!(target.books[0].id, book);
    assert_eq!(target.books[0].files.len(), 3);
    assert_eq!(fs::read(root.join(installments[1])).unwrap(), b"second\r\n");
    let mut one = files(TARGET, SOURCE, &[installments[2]]);
    let (token, items) = preview(&engine, root, &one);
    assert_eq!(items.len(), 1);
    one.confirmation = confirm(&token, []);
    let result = engine.move_files(root, &one).unwrap();
    assert_eq!(result.library.books[0].id, book);
    assert_eq!(result.library.books[0].files.len(), 2);
    assert!(!root.join(installments[2]).exists());
    let source = engine.switch(root, SOURCE, None).unwrap().library;
    assert_eq!(source.books[0].files, [installments[2]]);
    assert_eq!(source.books[0].id, book);
    assert_eq!(
        fs::read(root.join(installments[2])).unwrap(),
        b"second volume\r\n"
    );
}

#[test]
fn same_name_and_path_collisions_require_valid_new_destinations() {
    let engine = engine();
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    put(root, "同名.txt", b"incoming");
    put(root, "books/卷.txt", b"case folder");
    put(root, "资料/卷.txt", b"directory");
    put(root, "共享.txt", b"same bytes");
    engine.adopt_or_open(root).unwrap();
    engine.create_branch(root, TARGET).unwrap();
    engine.switch(root, TARGET, None).unwrap();
    put(root, "同名.txt", b"keep target");
    put(root, "Books/目标.txt", b"keep case");
    put(root, "资料", b"target file");
    put(root, "共享.txt", b"same bytes");
    let pending = engine.switch(root, SOURCE, None).unwrap().pending.unwrap();
    engine.switch(root, SOURCE, Some(&pending.token)).unwrap();
    let repo = engine.repository(root).unwrap();
    let before = references(&repo);
    let index_before = repo.run(&["write-tree"]).unwrap();

    let mut movement = files(
        SOURCE,
        TARGET,
        &["同名.txt", "books/卷.txt", "资料/卷.txt", "共享.txt"],
    );
    let (token, items) = preview(&engine, root, &movement);
    let conflicts: BTreeMap<_, _> = items
        .iter()
        .filter_map(|item| {
            item.conflict
                .as_deref()
                .map(|path| (item.path.as_str(), path))
        })
        .collect();
    assert_eq!(
        conflicts,
        BTreeMap::from([
            ("books/卷.txt", "Books/目标.txt"),
            ("同名.txt", "同名.txt"),
            ("资料/卷.txt", "资料"),
        ])
    );

    for invalid in [
        "同名.txt",
        "../书.txt",
        ".git/书.txt",
        "Books/卷.txt",
        "同名.epub",
    ] {
        movement.confirmation = confirm(
            &token,
            [
                ("同名.txt", invalid),
                ("books/卷.txt", "Books/卷.txt"),
                ("资料/卷.txt", "其他/卷.txt"),
            ],
        );
        assert!(engine.move_files(root, &movement).is_err(), "{invalid}");
        assert_eq!(references(&repo), before);
        assert_eq!(repo.run(&["write-tree"]).unwrap(), index_before);
        assert_eq!(fs::read(root.join("同名.txt")).unwrap(), b"incoming");
        assert_clean(root);
    }
    movement.confirmation = confirm(&token, []);
    assert!(engine.move_files(root, &movement).is_err());
    assert_eq!(references(&repo), before);
    movement.confirmation = confirm(
        &token,
        [
            ("同名.txt", "同名 移入.txt"),
            ("books/卷.txt", "Books/卷.txt"),
            ("资料/卷.txt", "其他/卷.txt"),
        ],
    );
    assert!(matches!(
        engine.move_files(root, &movement).unwrap().status,
        MoveStatus::Moved
    ));
    assert!(references(&repo)
        .iter()
        .zip(before)
        .all(|(new, old)| *new != old));
    assert!(!root.join("同名.txt").exists());
    engine.switch(root, TARGET, None).unwrap();
    assert_eq!(fs::read(root.join("同名.txt")).unwrap(), b"keep target");
    assert_eq!(fs::read(root.join("同名 移入.txt")).unwrap(), b"incoming");
    assert_eq!(fs::read(root.join("Books/卷.txt")).unwrap(), b"case folder");
    assert_eq!(fs::read(root.join("其他/卷.txt")).unwrap(), b"directory");
    assert_eq!(fs::read(root.join("共享.txt")).unwrap(), b"same bytes");
    assert_clean(root);
}

#[test]
fn interruption_worker() {
    let Some(root) = env::var_os(INTERRUPT_ROOT) else {
        return;
    };
    let engine = engine();
    let repo = engine.repository(Path::new(&root)).unwrap();
    let lease = open(&repo).unwrap();
    let paths = BTreeMap::from([
        (FIRST.to_owned(), FIRST.to_owned()),
        (SECOND.to_owned(), SECOND.to_owned()),
    ]);
    let (references, plan, stage) =
        prepare_move(&repo, &lease, State::load(&repo).unwrap(), TARGET, &paths).unwrap();
    worktree::interrupt_move_for_test(
        &repo,
        &lease,
        references,
        plan,
        stage,
        &env::var(INTERRUPT_AT).unwrap(),
    )
    .unwrap();
}

#[test]
fn reopening_rolls_back_staging_and_finishes_published_move() {
    let engine = engine();
    for boundary in ["partly-staged", "prepared", "published"] {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        put(root, FIRST, b"one\r\n");
        put(root, SECOND, b"two\0");
        engine.adopt_or_open(root).unwrap();
        engine.create_branch(root, TARGET).unwrap();
        let repo = engine.repository(root).unwrap();
        let before = references(&repo);
        let old_index = repo.run(&["write-tree"]).unwrap();

        let child = Command::new(env::current_exe().unwrap())
            .args([
                "--exact",
                "library::movement::tests::interruption_worker",
                "--nocapture",
            ])
            .env(INTERRUPT_ROOT, root)
            .env(INTERRUPT_AT, boundary)
            .output()
            .unwrap();
        assert!(
            child.status.success(),
            "{boundary} {:?} {:?}",
            child.stdout,
            child.stderr
        );
        assert!(!root.join(FIRST).exists());
        assert_eq!(root.join(SECOND).exists(), boundary == "partly-staged");
        assert!(root.join(".git/filemanagement/pending.json").exists());
        assert_eq!(repo.run(&["write-tree"]).unwrap(), old_index);
        let interrupted = references(&repo);
        if boundary == "published" {
            assert!(interrupted.iter().zip(&before).all(|(new, old)| new != old));
            put(root, FIRST, b"external write after publication");
            let error = engine.open_default(root).unwrap_err();
            assert!(error.to_string().contains("移出路径出现外部变动"));
            assert_eq!(
                fs::read(root.join(FIRST)).unwrap(),
                b"external write after publication"
            );
            assert!(root.join(".git/filemanagement/pending.json").exists());
            fs::remove_file(root.join(FIRST)).unwrap();
        } else {
            assert_eq!(interrupted, before);
        }
        let reopened = engine.open_default(root).unwrap();
        assert_eq!(reopened.library.current_branch, SOURCE);
        assert!(reopened.pending.is_none());
        assert_clean(root);
        assert_eq!(references(&repo), interrupted);
        assert_eq!(
            repo.run(&["write-tree"]).unwrap(),
            repo.run(&["rev-parse", "HEAD^{tree}"]).unwrap()
        );
        if boundary == "published" {
            assert!(reopened.library.files.is_empty());
            assert!(!root.join(FIRST).exists());
            engine.switch(root, TARGET, None).unwrap();
        } else {
            assert_eq!(reopened.library.files, [FIRST, SECOND]);
        }
        assert_eq!(fs::read(root.join(FIRST)).unwrap(), b"one\r\n");
        assert_eq!(fs::read(root.join(SECOND)).unwrap(), b"two\0");
    }
}

#[cfg(windows)]
#[test]
fn windows_occupied_file_aborts_the_whole_move() {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::{FILE_SHARE_READ, FILE_SHARE_WRITE};

    let engine = engine();
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    put(root, FIRST, b"one");
    put(root, SECOND, b"two");
    engine.adopt_or_open(root).unwrap();
    engine.create_branch(root, TARGET).unwrap();
    let repo = engine.repository(root).unwrap();
    let before = references(&repo);

    let mut movement = files(SOURCE, TARGET, &[FIRST, SECOND]);
    let (token, _) = preview(&engine, root, &movement);
    movement.confirmation = confirm(&token, []);
    let occupied = fs::OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
        .open(root.join(SECOND))
        .unwrap();
    let error = format!("{:#}", engine.move_files(root, &movement).unwrap_err());
    assert!(error.contains("乙.epub"), "{error}");
    assert_eq!(references(&repo), before);
    assert_eq!(fs::read(root.join(FIRST)).unwrap(), b"one");
    assert_eq!(fs::read(root.join(SECOND)).unwrap(), b"two");
    assert_clean(root);
    drop(occupied);
    assert!(matches!(
        engine.move_files(root, &movement).unwrap().status,
        MoveStatus::Moved
    ));
}
