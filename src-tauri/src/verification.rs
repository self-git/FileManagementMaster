use std::{fs, path::Path};

use anyhow::{ensure, Context, Result};
use serde::Serialize;

use crate::{
    git::{Git, RefEdit, Repository},
    worktree::{OperationRecord, RefState, WriteLease},
};

const READING: &str = "refs/heads/reading";
const COLLECTION: &str = "refs/heads/collection";
const STATE: &str = "refs/filemanagement/state";
const TXT: &str = "0911/群山 航线.txt";
const EPUB: &str = "0929/群山 航线.epub";
const SHARED: &str = "原始资料.bin";

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EngineReport {
    pub git_version: String,
    pub binary_path: String,
    pub ref_format: String,
    pub checks: Vec<EngineCheck>,
    pub object_bytes_before: u64,
    pub object_bytes_after: u64,
}

#[derive(Debug, Serialize)]
pub struct EngineCheck {
    pub name: &'static str,
    pub detail: &'static str,
}

impl EngineCheck {
    fn passed(name: &'static str, detail: &'static str) -> Self {
        Self { name, detail }
    }
}

pub fn verify_runtime(runtime_dir: &Path) -> Result<EngineReport> {
    let git = Git::bundled(runtime_dir)?;
    let workspace = tempfile::Builder::new()
        .prefix("filemanagement-engine-")
        .tempdir()?;
    let git_version = git.version(workspace.path())?;
    let repo = Repository::create(&git, workspace.path().join("临时书库 Git"))
        .context("初始化 reftable 临时仓库")?;
    let ref_format = repo.run(&["rev-parse", "--show-ref-format"])?;
    ensure!(ref_format == "reftable", "仓库引用格式为 {ref_format}");

    let lease = WriteLease::try_acquire(&repo)?.context("临时仓库写锁不可用")?;
    let fixture = Fixture::seed(&repo).context("装配临时索引和初始提交")?;
    let moved = verify_move(&repo, &lease, &fixture).context("验证跨分支引用发布和索引恢复")?;
    let switched = verify_head(&repo, &fixture, &moved).context("验证符号 HEAD 与当前文件")?;
    verify_interruption(&repo, &lease, &fixture, &switched).context("验证子进程中断与遗留锁")?;
    let (object_bytes_before, object_bytes_after) =
        verify_gc(&repo, &lease, &fixture, &moved.collection, &switched).context("验证对象回收")?;

    drop(lease);
    workspace.close().context("清理引擎验证的临时书库")?;
    Ok(EngineReport {
        git_version,
        binary_path: git.binary().display().to_string(),
        ref_format,
        checks: vec![
            EngineCheck::passed(
                "随包运行时",
                "由应用资源目录启动固定版 Git，成功初始化 reftable 仓库。",
            ),
            EngineCheck::passed(
                "引用事务",
                "旧目标引用不匹配时三项引用均保持原值，正确提交后源、目标和目录状态一起发布。",
            ),
            EngineCheck::passed(
                "索引与分支",
                "从已提交快照重建索引，符号 HEAD 切换后同一目录呈现目标分支文件。",
            ),
            EngineCheck::passed(
                "进程中断",
                "终止已准备的 Git 子进程，按退出凭证清理遗留引用锁。",
            ),
            EngineCheck::passed(
                "回收与写锁",
                "各格式保留 10 个旧版，保护其他分支的当前版和共享对象，实际清理过期对象。",
            ),
        ],
        object_bytes_before,
        object_bytes_after,
    })
}

struct Fixture {
    txt: Vec<String>,
    epub: Vec<String>,
    reading: String,
    collection: String,
    state: String,
}

impl Fixture {
    fn seed(repo: &Repository<'_>) -> Result<Self> {
        let txt = versions(repo, 1, 15)?;
        let epub = versions(repo, 101, 13)?;
        let reading = repo.snapshot(
            [
                (TXT, txt[14].as_str()),
                (EPUB, epub[12].as_str()),
                (SHARED, txt[1].as_str()),
            ],
            "初始阅读分支",
        )?;
        let collection = repo.snapshot([(TXT, txt[2].as_str())], "初始收藏分支")?;
        let state = make_catalog(repo, "初始历史", &txt, &epub)?;
        repo.update_references(&[
            RefEdit::create(READING, &reading),
            RefEdit::create(COLLECTION, &collection),
            RefEdit::create(STATE, &state),
        ])?;
        put_blob(repo, TXT, &txt[14])?;
        put_blob(repo, EPUB, &epub[12])?;
        put_blob(repo, SHARED, &txt[1])?;
        repo.run(&["read-tree", &reading])?;
        ensure!(
            repo.run(&["rev-list", "--count", &reading])? == "1",
            "分支快照应为无父提交"
        );
        assert_clean(repo)?;
        Ok(Self {
            txt,
            epub,
            reading,
            collection,
            state,
        })
    }

    fn retained_catalog(&self, repo: &Repository<'_>, operation: &str) -> Result<String> {
        // TXT 第 3 和 15 版与 EPUB 第 13 版由分支当前快照保护。
        make_catalog(repo, operation, &self.txt[4..14], &self.epub[2..12])
    }
}

struct Moved {
    collection: String,
    state: String,
}

fn verify_move(repo: &Repository<'_>, lease: &WriteLease, fixture: &Fixture) -> Result<Moved> {
    let reading = repo.snapshot(
        [
            (TXT, fixture.txt[14].as_str()),
            (SHARED, fixture.txt[1].as_str()),
        ],
        "从阅读移除 EPUB",
    )?;
    let collection = repo.snapshot(
        [
            (TXT, fixture.txt[2].as_str()),
            (EPUB, fixture.epub[12].as_str()),
        ],
        "向收藏加入 EPUB",
    )?;
    let state = fixture.retained_catalog(repo, "移动并限制历史")?;
    let record = OperationRecord {
        started_unlocked: false,
        references: vec![
            RefEdit::replace(READING, &fixture.reading, &reading),
            RefEdit::replace(COLLECTION, &fixture.collection, &collection),
            RefEdit::replace(STATE, &fixture.state, &state),
        ],
        writer: None,
        plan: None,
    };

    let original = repo.root().join(EPUB);
    let staged = lease.directory().join("staged.epub");
    record.create(lease)?;
    fs::rename(&original, &staged)?;

    let mut incorrect = record.references.clone();
    incorrect[1].old = Some(fixture.reading.clone());
    ensure!(
        repo.update_references(&incorrect).is_err(),
        "旧引用不匹配的事务被接受"
    );
    let persisted = OperationRecord::load(lease)?;
    ensure!(
        persisted.ref_state(repo)? == RefState::BeforeCommit,
        "失败事务留下部分引用更新"
    );
    ensure!(!original.exists(), "原件应该已在此验证的暂存目录");
    fs::rename(&staged, &original)?;
    ensure!(
        fs::read(&original)? == repo.data(&["cat-file", "blob", &fixture.epub[12]])?,
        "暂存恢复字节不符"
    );
    assert_clean(repo)?;

    fs::rename(&original, &staged)?;
    repo.update_references(&record.references)?;
    ensure!(
        OperationRecord::load(lease)?.ref_state(repo)? == RefState::AfterCommit,
        "实际引用与已提交记录不符"
    );
    ensure!(
        !repo.data(&["ls-files", "-z", "--", EPUB])?.is_empty(),
        "索引应仍处于提交前状态"
    );
    repo.run(&["read-tree", &reading])?;
    ensure!(
        repo.data(&["ls-files", "-z", "--", EPUB])?.is_empty(),
        "已移入收藏的文件仍在源索引"
    );
    assert_clean(repo)?;
    fs::remove_file(staged)?;
    lease.finish_record()?;
    Ok(Moved { collection, state })
}

fn verify_head(repo: &Repository<'_>, fixture: &Fixture, moved: &Moved) -> Result<String> {
    let state = fixture.retained_catalog(repo, "切换到收藏")?;
    put_blob(repo, TXT, &fixture.txt[2])?;
    put_blob(repo, EPUB, &fixture.epub[12])?;
    fs::remove_file(repo.root().join(SHARED))?;
    repo.switch_symbolic_head(
        READING,
        COLLECTION,
        &RefEdit::replace(STATE, &moved.state, &state),
    )?;
    repo.run(&["read-tree", &moved.collection])?;
    ensure!(
        repo.run(&["symbolic-ref", "HEAD"])? == COLLECTION,
        "HEAD 未切换至收藏"
    );
    ensure!(
        repo.run(&["rev-parse", "--verify", STATE])? == state,
        "HEAD 和状态引用未共同更新"
    );
    ensure!(
        fs::read(repo.root().join(EPUB))? == repo.data(&["cat-file", "blob", &fixture.epub[12]])?,
        "目标文件字节不符"
    );
    assert_clean(repo)?;
    Ok(state)
}

fn verify_interruption(
    repo: &Repository<'_>,
    lease: &WriteLease,
    fixture: &Fixture,
    state: &str,
) -> Result<()> {
    let next = fixture.retained_catalog(repo, "中断前准备的状态")?;
    let lock_path = repo.root().join(".git/reftable/tables.list.lock");
    ensure!(!lock_path.exists(), "验证开始前已有 Git 引用锁");
    let mut child = repo.start_ref_update()?;
    let record = OperationRecord {
        started_unlocked: false,
        references: vec![RefEdit::replace(STATE, state, &next)],
        writer: Some(child.stamp()),
        plan: None,
    };
    // update-ref 等待输入，先持久化该次写进程的记录，再发送 prepare。
    record.create(lease)?;
    child.prepare_references(&record.references)?;
    ensure!(lock_path.is_file(), "prepare 后 Git 引用锁缺失");
    ensure!(
        record.ref_state(repo)? == RefState::BeforeCommit,
        "prepare 提前发布了引用"
    );
    let exited = child.interrupt()?;
    OperationRecord::load(lease)?.reap_interrupted_ref_lock(repo, lease, exited)?;
    ensure!(!lock_path.exists(), "已退出的 Git 子进程仍占用引用锁");
    ensure!(
        record.ref_state(repo)? == RefState::BeforeCommit,
        "中断后错误发布了引用"
    );
    lease.finish_record()
}

fn verify_gc(
    repo: &Repository<'_>,
    lease: &WriteLease,
    fixture: &Fixture,
    collection: &str,
    state: &str,
) -> Result<(u64, u64)> {
    let objects = repo.root().join(".git/objects");
    let before = file_bytes(&objects)?;
    run_exclusive_gc(repo, lease)?;

    for i in [0, 3] {
        ensure!(
            !repo.object_exists(&fixture.txt[i])?,
            "过期 TXT 对象仍在库中"
        );
    }
    for oid in &fixture.epub[..2] {
        ensure!(!repo.object_exists(oid)?, "过期 EPUB 对象仍在库中");
    }
    ensure!(
        repo.object_exists(&fixture.txt[2])?,
        "另一分支正在用的 TXT 第 3 版被清理"
    );

    let latest = repo.snapshot(
        [
            (TXT, fixture.txt[14].as_str()),
            (EPUB, fixture.epub[12].as_str()),
        ],
        "释放旧 TXT 的最后一个分支引用",
    )?;
    let next_state = fixture.retained_catalog(repo, "重新计算受保护的当前版")?;
    put_blob(repo, TXT, &fixture.txt[14])?;
    repo.update_references(&[
        RefEdit::replace(COLLECTION, collection, &latest),
        RefEdit::replace(STATE, state, &next_state),
    ])?;
    repo.run(&["read-tree", &latest])?;
    run_exclusive_gc(repo, lease)?;

    ensure!(
        !repo.object_exists(&fixture.txt[2])?,
        "失去最后一个分支引用的 TXT 未回收"
    );
    ensure!(repo.object_exists(&fixture.txt[1])?, "共享 blob 被错误清理");
    for oid in fixture.txt[4..].iter().chain(&fixture.epub[2..]) {
        ensure!(repo.object_exists(oid)?, "应保留的版本已丢失");
    }
    ensure!(
        repo.run(&["ls-tree", "-r", "--name-only", &next_state, "history/txt"])?
            .lines()
            .count()
            == 10,
        "TXT 历史数量不符"
    );
    ensure!(
        repo.run(&["ls-tree", "-r", "--name-only", &next_state, "history/epub"])?
            .lines()
            .count()
            == 10,
        "EPUB 历史数量不符"
    );
    ensure!(
        repo.run(&["reflog", "show", "--all"])?.is_empty(),
        "发现会延长旧对象寿命的 reflog"
    );
    repo.run(&["fsck", "--full", "--strict"])?;
    assert_clean(repo)?;

    let after = file_bytes(&objects)?;
    ensure!(
        after < before,
        "回收后对象库大小没有下降（{before} → {after}）"
    );
    Ok((before, after))
}

fn run_exclusive_gc(repo: &Repository<'_>, lease: &WriteLease) -> Result<()> {
    ensure!(
        lease.directory() == repo.control_dir(),
        "回收未持有该书库的写锁"
    );
    let child = repo.start_gc()?;
    ensure!(
        WriteLease::try_acquire(repo)?.is_none(),
        "回收期间第二个写入者进入仓库"
    );
    child.finish_gc()
}

fn make_catalog(
    repo: &Repository<'_>,
    label: &str,
    txt: &[String],
    epub: &[String],
) -> Result<String> {
    let catalog = repo.write_blob(&serde_json::to_vec(
        &serde_json::json!({ "operation": label }),
    )?)?;
    let mut entries = vec![("catalog.json".to_owned(), catalog.as_str())];
    for (format, versions) in [("txt", txt), ("epub", epub)] {
        for (index, oid) in versions.iter().enumerate() {
            entries.push((format!("history/{format}/{}", index + 1), oid.as_str()));
        }
    }
    repo.snapshot(
        entries.iter().map(|(path, oid)| (path.as_str(), *oid)),
        label,
    )
}

fn put_blob(repo: &Repository<'_>, file: &str, oid: &str) -> Result<()> {
    let target = repo.root().join(file);
    fs::create_dir_all(target.parent().context("验证文件缺少父目录")?)?;
    fs::write(target, repo.data(&["cat-file", "blob", oid])?)?;
    Ok(())
}

fn assert_clean(repo: &Repository<'_>) -> Result<()> {
    ensure!(
        repo.data(&["status", "--porcelain=v2", "-z"])?.is_empty(),
        "实际文件目录和当前索引不一致"
    );
    Ok(())
}

fn versions(repo: &Repository<'_>, seed: u32, count: u32) -> Result<Vec<String>> {
    (seed..seed + count)
        .map(|mut value| {
            // 可重复的验收数据，避免压缩率掩盖过期对象的删除。
            let data: Vec<_> = (0..(32 * 1024))
                .map(|_| {
                    value ^= value << 13;
                    value ^= value >> 17;
                    value ^= value << 5;
                    value as u8
                })
                .collect();
            repo.write_blob(&data)
        })
        .collect()
}

fn file_bytes(path: &Path) -> Result<u64> {
    let mut bytes = 0;
    for item in fs::read_dir(path)? {
        let item = item?;
        let metadata = item.metadata()?;
        bytes += if metadata.is_dir() {
            file_bytes(&item.path())?
        } else {
            metadata.len()
        };
    }
    Ok(bytes)
}
