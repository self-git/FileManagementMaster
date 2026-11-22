use std::{
    env, fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

use filemanagement_master::{
    LibraryEngine, NovelFormat, NovelScanRequest, ReadRequest, RestoreRequest,
    ScanControl, ScanPhase, ScanReviewEdit,
};
use serde_json::{json, Value};

fn runtime() -> PathBuf {
    env::var_os("FILEMANAGEMENT_BUNDLED_GIT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("resources/git"))
}

fn engine() -> LibraryEngine {
    LibraryEngine::new(&runtime()).unwrap()
}

fn git(root: &Path, args: &[&str]) -> Output {
    let binary = if cfg!(windows) {
        "ucrt64/bin/git.exe"
    } else {
        "bin/git"
    };
    Command::new(runtime().join(binary))
        .current_dir(root)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env(
            "GIT_CONFIG_GLOBAL",
            if cfg!(windows) { "NUL" } else { "/dev/null" },
        )
        .args(args)
        .output()
        .unwrap()
}

fn put(root: &Path, path: &str, data: &[u8]) {
    let path = root.join(path);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, data).unwrap();
}

fn send(
    engine: &LibraryEngine,
    root: &Path,
    source: &Value,
    formats: &Value,
    confirmation: Value,
) -> Value {
    let request: ReadRequest = serde_json::from_value(json!({
        "source": source, "formats": formats, "confirmation": confirmation,
    }))
    .unwrap();
    serde_json::to_value(engine.read_directory(root, &request).unwrap()).unwrap()
}

fn preview(engine: &LibraryEngine, root: &Path, source: &Value, formats: &Value) -> Value {
    let result = send(engine, root, source, formats, Value::Null);
    assert_eq!(result["status"]["kind"], "preview");
    result["status"]["preview"].clone()
}

fn apply(
    engine: &LibraryEngine,
    root: &Path,
    source: &Value,
    formats: &Value,
    scanned: &Value,
    selections: &[Value],
) -> Value {
    send(
        engine,
        root,
        source,
        formats,
        json!({"kind":"apply","token":scanned["token"],"selections":selections}),
    )
}

fn item<'a>(scanned: &'a Value, path: &str) -> &'a Value {
    scanned["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["path"] == path)
        .unwrap_or_else(|| panic!("扫描未找到 {path}"))
}

fn choice(item: &Value, branch: &str, action: &str, replaces: &[&str]) -> Value {
    json!({
        "path": item["path"], "book": item["preferredBook"], "branch": branch,
        "action": action, "destination": item["path"], "replaces": replaces,
    })
}

fn start_dir(root: &Path, name: &str) -> PathBuf {
    let path = root.join(name);
    fs::create_dir(&path).unwrap();
    path
}

#[test]
fn scans_nested_files_and_dedupes_only_confirmed_book_and_format() {
    let engine = engine();
    let temp = tempfile::tempdir().unwrap();
    let root = start_dir(temp.path(), "原有小说库");
    let old = "0911/二级/《远海2》 1-111 未完结 作者：舟.txt";
    let new = "0929/《远海2》 1-116 已完结 作者：舟.TXT";
    let duplicate = "0929/《远海2》 1-116 已完结 作者：舟 (1).txt";
    let epub = "0911/《远海2》 1-110 作者：舟.EPUB";
    let other = "0929/《远海2》 1-116 作者：另一个人.txt";
    let unknown = "没有作者/远海2 1-115.txt";
    put(&root, old, b"old CRLF\r\n");
    put(&root, new, b"updated\n");
    put(&root, duplicate, b"updated\n");
    put(&root, epub, b"epub");
    put(&root, other, b"another author");
    put(&root, unknown, b"ambiguous");
    put(&root, "扫描选项/远海2.FB2.ZIP", b"compressed");
    put(&root, "扫描选项/普通压缩.zip", b"zip");
    put(&root, "扫描选项/远海2.pdf", b"pdf");
    put(&root, "另一书库/.git/control", b"skip");
    put(&root, "另一书库/不应纳管.txt", b"skip");
    engine.adopt_or_open(&root).unwrap();
    let source = json!({"kind":"library"});
    let defaults = json!(["epub", "txt", "mobi", "azw3"]);
    let scanned = preview(&engine, &root, &source, &defaults);
    assert_eq!(scanned["items"].as_array().unwrap().len(), 6);
    let winner = item(&scanned, new);
    assert_eq!(winner["novel"]["title"], "远海2");
    assert_eq!(winner["novel"]["author"], "舟");
    assert_eq!(
        winner["preferredBook"],
        item(&scanned, old)["preferredBook"]
    );
    assert_eq!(
        winner["preferredBook"],
        item(&scanned, epub)["preferredBook"]
    );
    assert_ne!(
        winner["preferredBook"],
        item(&scanned, other)["preferredBook"]
    );
    assert!(item(&scanned, unknown)["preferredBook"].is_null());
    assert_eq!(item(&scanned, duplicate)["novel"]["copy"], 1);
    assert_eq!(item(&scanned, old)["supersededBy"], new);
    assert!(item(&scanned, old)["coveringPaths"]
        .as_array()
        .unwrap()
        .contains(&json!(new)));

    let optional = preview(&engine, &root, &source, &json!(["fb2.zip", "pdf"]));
    assert_eq!(optional["items"].as_array().unwrap().len(), 2);
    assert_eq!(
        item(&optional, "扫描选项/远海2.FB2.ZIP")["novel"]["format"],
        "fb2.zip"
    );
    let done = apply(
        &engine,
        &root,
        &source,
        &defaults,
        &scanned,
        &[
            choice(winner, "常看", "keep", &[old, duplicate]),
            choice(item(&scanned, old), "常看", "history", &[]),
            choice(item(&scanned, duplicate), "常看", "history", &[]),
            choice(item(&scanned, epub), "常看", "keep", &[]),
            choice(item(&scanned, other), "常看", "keep", &[]),
        ],
    );
    assert_eq!(done["status"]["kind"], "applied");
    assert!(!root.join(old).exists());
    assert!(!root.join(duplicate).exists());
    assert_eq!(fs::read(root.join(new)).unwrap(), b"updated\n");
    assert_eq!(fs::read(root.join(epub)).unwrap(), b"epub");
    assert!(root.join(unknown).exists() && root.join(other).exists());
    let text_history =
        serde_json::to_value(engine.file_history(&root, "常看", new).unwrap()).unwrap();
    assert_eq!(text_history["versions"].as_array().unwrap().len(), 1);
    assert_eq!(text_history["versions"][0]["savedPath"], old);
    let epub_history =
        serde_json::to_value(engine.file_history(&root, "常看", epub).unwrap()).unwrap();
    assert!(epub_history["versions"].as_array().unwrap().is_empty());
}

#[test]
fn background_scan_groups_same_txt_names_and_preserves_all_current_files() {
    let engine = engine();
    let temp = tempfile::tempdir().unwrap();
    let root = start_dir(temp.path(), "后台扫描");
    let copied = if cfg!(windows) { "下载/山海录.txt" } else { "引号\"和反斜线\\与换行\n/山海录.txt" };
    let editions = ["0911/山海录.txt", "0929/山海录.TXT", copied];
    let content: [&[u8]; 3] = [b"first\r\n", b"another edition", b"first\r\n"];
    for (path, bytes) in editions.iter().zip(content) {
        put(&root, path, bytes);
    }
    put(&root, "0911/连载2 1-100 作者：舟.txt", b"early");
    put(&root, "0929/[已完结] 连载2 1-130 作者：舟.txt", b"full");
    put(&root, "0929/连载2 1-130 作者：江.txt", b"other");
    put(&root, "0911/连载2 作者：舟.epub", b"epub");
    put(&root, "附件.bin", b"unchecked");
    let adopted = engine.adopt_or_open(&root).unwrap().library;
    let head = git(&root, &["rev-parse", "HEAD"]).stdout;
    let mut scope = NovelScanRequest {
        directory: root.clone(),
        branch: adopted.current_branch.clone(),
        revision: adopted.revision,
        formats: [NovelFormat::Txt, NovelFormat::Epub].into(),
    };
    let verified_while_open = std::cell::Cell::new(false);
    let (result, _) = engine.scan_novels(&scope, &ScanControl::default(), |phase| {
        if matches!(phase, ScanPhase::Verifying { .. }) {
            assert!(engine.path_for_open(&root, &scope.branch, editions[0]).is_ok());
            verified_while_open.set(true);
        }
    }).unwrap();
    assert!(verified_while_open.get());
    assert_eq!(result.matched, 7);
    assert_eq!(result.library.books.len(), 3);
    let book = result.library.books.iter().find(|book| book.title == "山海录").unwrap();
    assert_eq!(book.files.len(), 3);
    assert_eq!(git(&root, &["rev-parse", "HEAD"]).stdout, head);
    for (path, bytes) in editions.iter().zip(content) {
        assert_eq!(fs::read(root.join(path)).unwrap(), bytes);
    }
    let report = serde_json::to_value(&result.groups).unwrap();
    assert!(report.as_array().unwrap().iter().flat_map(|group| group["files"].as_array().unwrap())
        .any(|file| file["path"] == editions[2] && file["sameAs"] == editions[0]));
    assert!(report.as_array().unwrap().iter().flat_map(|group| group["files"].as_array().unwrap())
        .any(|file| file["path"] == "0911/连载2 1-100 作者：舟.txt"
            && file["supersededBy"] == "0929/[已完结] 连载2 1-130 作者：舟.txt"));

    scope.revision = result.library.revision;
    let (again, _) = engine.scan_novels(&scope, &ScanControl::default(), |_| {}).unwrap();
    assert_eq!(again.library.revision, scope.revision);
    scope.formats = [NovelFormat::Pdf].into();
    let (empty, _) = engine.scan_novels(&scope, &ScanControl::default(), |phase| {
        assert!(!matches!(phase, ScanPhase::Verifying { .. } | ScanPhase::Publishing));
    }).unwrap();
    assert_eq!(empty.matched, 0);
    assert_eq!(empty.library.revision, scope.revision);
}

#[test]
fn stopped_scan_and_stale_review_preserve_files_and_git_references() {
    let engine = engine();
    let temp = tempfile::tempdir().unwrap();
    let root = start_dir(temp.path(), "停止扫描");
    let first = "早期/旧书.txt";
    let second = "新版/旧书.txt";
    put(&root, first, b"early");
    put(&root, second, b"later");
    let adopted = engine.adopt_or_open(&root).unwrap().library;
    let before = git(&root, &["for-each-ref"]).stdout;
    let scope = NovelScanRequest {
        directory: root.clone(),
        branch: adopted.current_branch.clone(),
        revision: adopted.revision,
        formats: [NovelFormat::Txt].into(),
    };
    let control = ScanControl::default();
    let stopped = engine.scan_novels(&scope, &control, |phase| {
        if matches!(phase, ScanPhase::Verifying { .. }) {
            assert!(control.cancel());
        }
    }).err().expect("扫描应停止");
    assert_eq!(stopped.to_string(), "扫描已停止");
    assert_eq!(git(&root, &["for-each-ref"]).stdout, before);
    assert_eq!(fs::read(root.join(first)).unwrap(), b"early");
    assert_eq!(fs::read(root.join(second)).unwrap(), b"later");
    assert!(!root.join(".git/filemanagement/pending.json").exists());

    let (result, plan) = engine.scan_novels(&scope, &ScanControl::default(), |_| {}).unwrap();
    assert_eq!(result.library.books.len(), 1);
    let changes = [ScanReviewEdit::Archive { group: 0, paths: [first.to_owned()].into() }];
    let grouped = git(&root, &["for-each-ref"]).stdout;
    put(&root, first, b"edited");
    let stale = engine.review_novel_scan(&plan, &changes, &ScanControl::default(), |_| {});
    assert!(stale.is_err());
    assert_eq!(fs::read(root.join(first)).unwrap(), b"edited");
    assert_eq!(git(&root, &["for-each-ref"]).stdout, grouped);
    put(&root, first, b"early");
    let cancel = ScanControl::default();
    let stopped_review = engine.review_novel_scan(&plan, &changes, &cancel, |phase| {
        if matches!(phase, ScanPhase::Verifying { .. }) {
            assert!(cancel.cancel());
        }
    });
    assert!(stopped_review.is_err());
    assert_eq!(git(&root, &["for-each-ref"]).stdout, grouped);

    let (reviewed, current_plan) = engine.review_novel_scan(&plan, &changes, &ScanControl::default(), |_| {}).unwrap();
    assert_eq!(reviewed.library.books[0].files, [second]);
    assert!(!root.join(first).exists());
    let history = serde_json::to_value(engine.file_history(&root, &scope.branch, second).unwrap()).unwrap();
    assert_eq!(history["versions"][0]["savedPath"], first);
    engine.create_branch(&root, "另一分支").unwrap();
    engine.switch(&root, "另一分支", None).unwrap();
    assert!(engine.review_novel_scan(&current_plan, &changes, &ScanControl::default(), |_| {}).is_err());
}

#[test]
fn manual_read_validates_fresh_bytes_and_updates_only_the_selected_path() {
    let engine = engine();
    let temp = tempfile::tempdir().unwrap();
    let root = engine
        .create(temp.path(), "独立库")
        .unwrap()
        .library
        .directory;
    let external = start_dir(temp.path(), "来源目录");
    let path = "文档/进度.md";
    let source = json!({"kind":"directory","directory":external});
    put(&external, path, b"draft");
    let pre = preview(&engine, &root, &source, &Value::Null);
    put(&external, path, b"draft updated");
    let refreshed = apply(
        &engine,
        &root,
        &source,
        &Value::Null,
        &pre,
        &[choice(item(&pre, path), "常看", "keep", &[])],
    );
    assert_eq!(refreshed["status"]["kind"], "preview");
    assert!(!root.join(path).exists());
    let fresh = &refreshed["status"]["preview"];
    assert_eq!(
        apply(
            &engine,
            &root,
            &source,
            &Value::Null,
            fresh,
            &[choice(item(fresh, path), "常看", "keep", &[])]
        )["status"]["kind"],
        "applied"
    );
    assert_eq!(fs::read(root.join(path)).unwrap(), b"draft updated");
    put(&external, path, b"second");
    put(&external, "文档/未选.txt", b"skip");
    put(&root, "Finder改动.md", b"local edit");
    let pre = preview(&engine, &root, &source, &Value::Null);
    assert_eq!(pre["externalChanges"][0]["path"], "Finder改动.md");
    assert_eq!(
        send(
            &engine,
            &root,
            &source,
            &Value::Null,
            json!({"kind":"saveExternal","token":pre["token"]})
        )["status"]["kind"],
        "savedExternal"
    );
    let pre = preview(&engine, &root, &source, &Value::Null);
    assert_eq!(
        apply(
            &engine,
            &root,
            &source,
            &Value::Null,
            &pre,
            &[choice(item(&pre, path), "常看", "keep", &[path])]
        )["status"]["kind"],
        "applied"
    );
    assert_eq!(fs::read(external.join(path)).unwrap(), b"second");
    assert_eq!(fs::read(root.join(path)).unwrap(), b"second");
    assert!(!root.join("文档/未选.txt").exists());
    let history = serde_json::to_value(engine.file_history(&root, "常看", path).unwrap()).unwrap();
    let request: RestoreRequest = serde_json::from_value(json!({
        "branch":"常看","path":path,"token":history["token"],"order":history["versions"][0]["order"],
    })).unwrap();
    engine.restore_version(&root, &request).unwrap();
    assert_eq!(fs::read(root.join(path)).unwrap(), b"draft updated");
    assert_eq!(fs::read(root.join("Finder改动.md")).unwrap(), b"local edit");
}

#[test]
fn first_external_import_can_keep_an_older_download_in_history() {
    let engine = engine();
    let temp = tempfile::tempdir().unwrap();
    let root = engine
        .create(temp.path(), "空书库")
        .unwrap()
        .library
        .directory;
    let external = start_dir(temp.path(), "下载来源");
    let older = "0911/《潮位》 1-20 作者：舟.TXT";
    let latest = "0929/《潮位》 1-30 作者：舟.txt";
    put(&external, older, b"older body");
    put(&external, latest, b"new body");
    let source = json!({"kind":"directory","directory":external});
    let formats = json!(["txt"]);
    let scanned = preview(&engine, &root, &source, &formats);
    assert_eq!(item(&scanned, older)["supersededBy"], latest);
    let done = apply(
        &engine,
        &root,
        &source,
        &formats,
        &scanned,
        &[
            choice(item(&scanned, latest), "常看", "keep", &[]),
            choice(item(&scanned, older), "常看", "history", &[]),
        ],
    );
    assert_eq!(done["status"]["kind"], "applied");
    assert_eq!(done["library"]["files"], json!([latest]));
    let history = order_history(&engine, &root, latest);
    assert_eq!(history["versions"].as_array().unwrap().len(), 1);
    assert_eq!(history["versions"][0]["savedPath"], older);
    assert_eq!(fs::read(external.join(older)).unwrap(), b"older body");
    assert!(!root.join(older).exists());
}

fn sample(version: usize, format: usize) -> Vec<u8> {
    let mut state = 1 + (format * 100 + version) as u64;
    (0..(76 * 1024))
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state >> 24) as u8
        })
        .collect()
}

fn novel_path(version: usize, format: &str) -> String {
    format!("试读/《航线2》 1-{}章 作者：舟.{format}", 100 + version)
}

fn order_history(engine: &LibraryEngine, root: &Path, path: &str) -> Value {
    serde_json::to_value(engine.file_history(root, "常看", path).unwrap()).unwrap()
}

#[test]
fn garbage_collection_keeps_ten_per_format_and_every_current_blob() {
    let engine = engine();
    let temp = tempfile::tempdir().unwrap();
    let root = start_dir(temp.path(), "回收书库");
    let external = start_dir(temp.path(), "下载更新");
    let paths = |v| [novel_path(v, "txt"), novel_path(v, "epub")];
    let source = json!({"kind":"directory","directory":external});
    let formats = json!(["txt", "epub"]);
    let mut initial = Vec::new();
    for (i, path) in paths(0).iter().enumerate() {
        put(&root, path, &sample(0, i));
        let hash = git(&root, &["hash-object", path]);
        assert!(hash.status.success());
        initial.push(String::from_utf8(hash.stdout).unwrap().trim().to_owned());
    }
    put(&root, "共享对象.dat", &sample(0, 1));
    engine.adopt_or_open(&root).unwrap();
    engine.create_branch(&root, "收藏").unwrap();

    let mut saved = Vec::new();
    for version in 1..=14 {
        let from = paths(version);
        let previous = paths(version - 1);
        let mut hashes = Vec::new();
        for (i, path) in from.iter().enumerate() {
            put(&external, path, &sample(version, i));
            let hash = git(&external, &["hash-object", path]);
            assert!(hash.status.success());
            hashes.push(String::from_utf8(hash.stdout).unwrap().trim().to_owned());
        }
        saved.push(hashes);
        let scanned = preview(&engine, &root, &source, &formats);
        let selected: Vec<_> = from
            .iter()
            .zip(&previous)
            .map(|(incoming, old)| {
                let row = item(&scanned, incoming);
                let option = row["books"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|book| book["key"] == row["preferredBook"])
                    .unwrap();
                let locations = option["locations"].as_array().unwrap();
                assert!(locations
                    .iter()
                    .any(|location| location["path"] == *old && location["relation"] == "newer"));
                if version > 1 {
                    assert!(locations
                        .iter()
                        .any(|location| location["branch"] == "收藏"));
                } else {
                    assert!(locations
                        .iter()
                        .all(|location| location["branch"] == "常看"));
                }
                choice(row, "常看", "keep", &[old])
            })
            .collect();
        assert_eq!(
            apply(&engine, &root, &source, &formats, &scanned, &selected)["status"]["kind"],
            "applied"
        );
        if version == 1 {
            let scanned = preview(&engine, &root, &source, &formats);
            let archived: Vec<_> = from
                .iter()
                .map(|path| choice(item(&scanned, path), "收藏", "keep", &[]))
                .collect();
            assert_eq!(
                apply(&engine, &root, &source, &formats, &scanned, &archived)["status"]["kind"],
                "applied"
            );
        }
        for path in &from {
            fs::remove_file(external.join(path)).unwrap();
        }
    }

    let current = paths(14);
    for (i, path) in current.iter().enumerate() {
        let history = order_history(&engine, &root, path);
        let old = history["versions"].as_array().unwrap();
        let current = history["current"].as_array().unwrap();
        assert_eq!(old.len(), 10);
        assert_eq!(current.len(), 2);
        assert!(current
            .iter()
            .any(|item| item["branch"] == "收藏" && item["path"] == paths(1)[i]));
        assert_eq!(old[9]["savedPath"], paths(4)[i]);
        assert_eq!(fs::read(root.join(path)).unwrap(), sample(14, i));
    }
    let worker = env::var_os("FILEMANAGEMENT_GC_WORKER")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_BIN_EXE_filemanagement-master")));
    let report = engine.collect_garbage(&root, &worker).unwrap().unwrap();
    println!(
        "应用书库对象库实际占用 {} → {} 字节",
        report.before_bytes, report.after_bytes
    );
    assert!(report.after_bytes < report.before_bytes);
    assert!(!root.join(".git/filemanagement/gc.pending").exists());
    assert!(!root.join(".git/filemanagement/gc.writer").exists());
    let exists = |oid: &str| git(&root, &["cat-file", "-e", oid]).status.success();
    assert!(!exists(&initial[0]) && exists(&initial[1]));
    for (i, versions) in saved.iter().enumerate() {
        assert!(
            versions.iter().all(|oid| exists(oid) == (i == 0 || i >= 3)),
            "version {}",
            i + 1
        );
    }
    let check = git(&root, &["fsck", "--full", "--strict"]);
    assert!(
        check.status.success(),
        "{}",
        String::from_utf8_lossy(&check.stderr)
    );
    let history = order_history(&engine, &root, &current[1]);
    let request: RestoreRequest = serde_json::from_value(json!({
        "branch":"常看","path":current[1],"token":history["token"],"order":history["versions"][9]["order"],
    })).unwrap();
    engine.restore_version(&root, &request).unwrap();
    assert_eq!(fs::read(root.join(&paths(4)[1])).unwrap(), sample(4, 1));
    assert_eq!(fs::read(root.join(&current[0])).unwrap(), sample(14, 0));
    engine.switch(&root, "收藏", None).unwrap();
    assert_eq!(fs::read(root.join(&paths(1)[0])).unwrap(), sample(1, 0));
    assert_eq!(fs::read(root.join(&paths(1)[1])).unwrap(), sample(1, 1));
}
