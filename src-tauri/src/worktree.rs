use std::{
    collections::BTreeMap,
    ffi::OsStr,
    fs,
    io::{self, Write},
    path::{Component, Path, PathBuf},
    time::Duration,
};

use anyhow::{bail, ensure, Context, Result};
use serde::{Deserialize, Serialize};
use tempfile::TempDir;

use crate::git::{ExitedProcess, ProcessStamp, RefEdit, Repository, RunningGit};

const MARKER: &str = "library-v1";

pub(crate) fn owned_library(repo: &Repository<'_>) -> Result<bool> {
    let git_dir = repo.root().join(".git");
    let Some(git) = metadata(&git_dir)? else {
        return Ok(false);
    };
    if !git.is_dir() || is_link(&git) {
        return Ok(false);
    }
    let control = repo.control_dir();
    let Some(control_meta) = metadata(&control)? else {
        return Ok(false);
    };
    if !control_meta.is_dir() || is_link(&control_meta) {
        return Ok(false);
    }
    Ok(metadata(&control.join(MARKER))?.is_some_and(|m| m.is_file() && !is_link(&m)))
}

pub(crate) fn claim_library(repo: &Repository<'_>) -> Result<()> {
    let git_dir = repo.root().join(".git");
    ensure!(
        metadata(&git_dir)?.is_none(),
        "目录已有 Git 仓库，请选择独立书库目录"
    );
    // 只创建自己的控制目录，原目录中的文件原地保留。
    fs::create_dir(&git_dir)?;
    fs::create_dir(repo.control_dir())?;
    fs::File::create_new(repo.control_dir().join(MARKER))?.sync_all()?;
    Ok(())
}

pub(crate) struct WriteLease {
    guard: fs::File,
    directory: PathBuf,
    pub(crate) recovered: Option<RefState>,
}

impl WriteLease {
    pub(crate) fn try_acquire(repository: &Repository<'_>) -> Result<Option<Self>> {
        let directory = repository.control_dir();
        let guard = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(directory.join("write.lock"))?;
        match guard.try_lock() {
            Ok(()) => Ok(Some(Self {
                guard,
                directory,
                recovered: None,
            })),
            Err(fs::TryLockError::WouldBlock) => Ok(None),
            Err(fs::TryLockError::Error(error)) => Err(error.into()),
        }
    }

    pub(crate) fn directory(&self) -> &Path {
        &self.directory
    }

    pub(crate) fn finish_record(&self) -> Result<()> {
        fs::remove_file(self.record_path())?;
        Ok(())
    }

    fn record_path(&self) -> PathBuf {
        self.directory.join("pending.json")
    }
}

impl Drop for WriteLease {
    fn drop(&mut self) {
        // 子进程启动期间可能短暂继承文件描述符，操作结束时显式释放写锁。
        let _ = self.guard.unlock();
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct BlobFile {
    pub(crate) oid: String,
    pub(crate) executable: bool,
}

pub(crate) type Files = BTreeMap<String, BlobFile>;

#[derive(Serialize, Deserialize)]
pub(crate) struct OperationRecord {
    pub(crate) references: Vec<RefEdit>,
    pub(crate) writer: Option<ProcessStamp>,
    #[serde(default)]
    pub(crate) started_unlocked: bool,
    #[serde(default)]
    pub(crate) plan: Option<WorktreePlan>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum RefState {
    BeforeCommit,
    AfterCommit,
}

impl OperationRecord {
    pub(crate) fn create(&self, lease: &WriteLease) -> Result<()> {
        let mut staged = tempfile::NamedTempFile::new_in(lease.directory())?;
        serde_json::to_writer(&mut staged, self)?;
        staged.flush()?;
        staged.as_file().sync_all()?;
        staged
            .persist_noclobber(lease.record_path())
            .context("发布未完成操作记录")?;
        Ok(())
    }

    pub(crate) fn load(lease: &WriteLease) -> Result<Self> {
        Self::load_pending(lease)?.context("未完成操作记录不存在")
    }

    fn load_pending(lease: &WriteLease) -> Result<Option<Self>> {
        match fs::File::open(lease.record_path()) {
            Ok(file) => Ok(Some(serde_json::from_reader(file)?)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    pub(crate) fn ref_state(&self, repo: &Repository<'_>) -> Result<RefState> {
        ensure!(!self.references.is_empty(), "操作记录未包含引用");
        let mut matches_old = true;
        let mut matches_new = true;
        for edit in &self.references {
            let oid = repo.reference(&edit.name)?;
            matches_old &= edit.old == oid;
            matches_new &= oid.as_deref() == Some(&edit.new);
        }
        if let Some(head) = self.plan.as_ref().and_then(|plan| plan.head.as_ref()) {
            let actual = repo.run(&["symbolic-ref", "HEAD"])?;
            matches_old &= actual == head.previous;
            matches_new &= actual == head.next;
        }
        match (matches_old, matches_new) {
            (true, false) => Ok(RefState::BeforeCommit),
            (false, true) => Ok(RefState::AfterCommit),
            _ => bail!("实际引用与未完成记录不一致"),
        }
    }

    pub(crate) fn reap_interrupted_ref_lock(
        &self,
        repo: &Repository<'_>,
        lease: &WriteLease,
        exited: ExitedProcess,
    ) -> Result<()> {
        ensure!(lease.directory == repo.control_dir(), "写锁属于另一个书库");
        ensure!(
            self.writer == Some(exited.stamp()),
            "退出的 Git 子进程与记录不符"
        );
        ensure!(
            self.ref_state(repo)? == RefState::BeforeCommit,
            "引用已发布"
        );
        let lock = ref_lock(repo);
        ensure!(
            fs::symlink_metadata(&lock)?.is_file(),
            "预期的 Git 引用锁不存在"
        );
        fs::remove_file(lock).context("清理已结束子进程留下的引用锁")?;
        Ok(())
    }

    fn settled_state(&self, repo: &Repository<'_>) -> Result<RefState> {
        if let Some(writer) = self.writer {
            wait_for_writer(writer)?;
        }
        let state = self.ref_state(repo)?;
        let lock = ref_lock(repo);
        if let Some(info) = metadata(&lock)? {
            // 书库独占写入。只清理由本次操作开始后出现、且写进程已经退出的锁。
            ensure!(
                self.started_unlocked
                    && info.is_file()
                    && !is_link(&info)
                    && self
                        .writer
                        .context("没有 Git 写进程记录")?
                        .predates(&info)?,
                "无法确认 Git 引用锁归属，已保留操作记录和原件"
            );
            fs::remove_file(lock).context("清理已结束子进程留下的引用锁")?;
        }
        Ok(state)
    }
}

#[derive(Serialize, Deserialize)]
struct FileChange {
    path: String,
    old: Option<BlobFile>,
    new: Option<BlobFile>,
}

#[derive(Serialize, Deserialize)]
struct SymbolicHead {
    previous: String,
    next: String,
}

#[derive(Serialize, Deserialize)]
pub(crate) struct WorktreePlan {
    old_index: Option<String>,
    new_index: String,
    head: Option<SymbolicHead>,
    stage: Option<String>,
    changes: Vec<FileChange>,
}

impl WorktreePlan {
    pub(crate) fn index(old_index: Option<String>, new_index: String) -> Self {
        Self {
            old_index,
            new_index,
            head: None,
            stage: None,
            changes: Vec::new(),
        }
    }

    pub(crate) fn switch(
        repo: &Repository<'_>,
        lease: &WriteLease,
        head: (&str, &str),
        commits: (&str, &str),
        old: &Files,
        new: &Files,
    ) -> Result<(Self, TempDir)> {
        Self::stage(
            repo,
            lease,
            commits,
            old,
            new,
            Some(SymbolicHead {
                previous: head.0.to_owned(),
                next: head.1.to_owned(),
            }),
        )
    }

    pub(crate) fn move_out(
        repo: &Repository<'_>,
        lease: &WriteLease,
        commits: (&str, &str),
        old: &Files,
        new: &Files,
    ) -> Result<(Self, TempDir)> {
        ensure!(
            new.iter().all(|(path, file)| old.get(path) == Some(file)),
            "跨分支移动只移出原分支的所选文件"
        );
        Self::update(repo, lease, commits, old, new)
    }

    pub(crate) fn update(
        repo: &Repository<'_>,
        lease: &WriteLease,
        commits: (&str, &str),
        old: &Files,
        new: &Files,
    ) -> Result<(Self, TempDir)> {
        #[cfg(target_os = "macos")]
        {
            use std::os::unix::fs::MetadataExt;
            let device = fs::metadata(lease.directory())?.dev();
            for (path, _) in old
                .iter()
                .filter(|(path, file)| new.get(*path) != Some(file))
            {
                let path = resolved_path(repo.root(), path, false)?;
                ensure!(
                    fs::symlink_metadata(&path)?.dev() == device,
                    "暂存位置与原件不在同一卷 {}",
                    path.display()
                );
            }
        }
        Self::stage(repo, lease, commits, old, new, None)
    }

    fn stage(
        repo: &Repository<'_>,
        lease: &WriteLease,
        commits: (&str, &str),
        old: &Files,
        new: &Files,
        head: Option<SymbolicHead>,
    ) -> Result<(Self, TempDir)> {
        let mut changes = Vec::new();
        for path in old.keys() {
            if old.get(path) != new.get(path) {
                changes.push(FileChange {
                    path: path.clone(),
                    old: old.get(path).cloned(),
                    new: new.get(path).cloned(),
                });
            }
        }
        for (path, file) in new {
            if !old.contains_key(path) {
                changes.push(FileChange {
                    path: path.clone(),
                    old: None,
                    new: Some(file.clone()),
                });
            }
        }

        let stage = tempfile::Builder::new()
            .prefix("switch-")
            .tempdir_in(lease.directory())?;
        fs::create_dir(stage.path().join("new"))?;
        fs::create_dir(stage.path().join("old"))?;
        for (index, change) in changes.iter().enumerate() {
            if let Some(next) = &change.new {
                let target = fs::File::create_new(staged_path(stage.path(), "new", index))?;
                repo.extract_blob(&next.oid, &target)?;
                set_executable(&target, next.executable)?;
                target.sync_all()?;
            }
        }
        let stage_name = stage
            .path()
            .file_name()
            .and_then(OsStr::to_str)
            .context("无效暂存目录")?
            .to_owned();
        Ok((
            Self {
                old_index: Some(commits.0.to_owned()),
                new_index: commits.1.to_owned(),
                head,
                stage: Some(stage_name),
                changes,
            },
            stage,
        ))
    }

    fn apply_files(&self, repo: &Repository<'_>, stage: &Path) -> Result<()> {
        for (index, change) in self.changes.iter().enumerate() {
            Self::stage_original(repo, stage, index, change)?;
        }
        for change in &self.changes {
            if change.old.is_some() {
                prune_empty_parents(repo.root(), &change.path)?;
            }
        }
        for (index, change) in self.changes.iter().enumerate() {
            if change.new.is_some() {
                let path = resolved_path(repo.root(), &change.path, true)?;
                rename_exclusive(&staged_path(stage, "new", index), &path)?;
            }
        }
        Ok(())
    }

    fn stage_original(
        repo: &Repository<'_>,
        stage: &Path,
        index: usize,
        change: &FileChange,
    ) -> Result<()> {
        if let Some(old) = &change.old {
            let path = resolved_path(repo.root(), &change.path, false)?;
            ensure!(
                observe(repo, &path)?.as_ref() == Some(old),
                "文件已变更，请重新确认 {}",
                path.display()
            );
            let backup = staged_path(stage, "old", index);
            rename_exclusive(&path, &backup)?;
            ensure!(
                observe(repo, &backup)?.as_ref() == Some(old),
                "暂存期间文件已变更 {}",
                path.display()
            );
        }
        Ok(())
    }

    fn restore_files(&self, repo: &Repository<'_>, stage: &Path) -> Result<()> {
        // 新文件仍在暂存目录时，说明其可见路径没有被本次操作写入。
        for (index, change) in self.changes.iter().enumerate().rev() {
            if let Some(new) = &change.new {
                let incoming = staged_path(stage, "new", index);
                if metadata(&incoming)?.is_none() {
                    let path = resolved_path(repo.root(), &change.path, false)?;
                    ensure!(
                        observe(repo, &path)? == Some(new.clone()),
                        "路径出现外部变动，暂存已保留 {}",
                        path.display()
                    );
                    rename_exclusive(&path, &incoming)?;
                    prune_empty_parents(repo.root(), &change.path)?;
                }
            }
        }
        for (index, change) in self.changes.iter().enumerate().rev() {
            let backup = staged_path(stage, "old", index);
            if metadata(&backup)?.is_some() {
                ensure!(
                    observe(repo, &backup)? == change.old,
                    "暂存原件出现变动，已保留 {}",
                    backup.display()
                );
                let path = resolved_path(repo.root(), &change.path, true)?;
                rename_exclusive(&backup, &path)?;
            }
        }
        Ok(())
    }

    fn finish(&self, repo: &Repository<'_>, lease: &WriteLease, state: &RefState) -> Result<()> {
        let stage = self
            .stage
            .as_deref()
            .map(|name| stage_directory(lease, name))
            .transpose()?;
        match state {
            RefState::BeforeCommit => {
                if let Some(stage) = &stage {
                    self.restore_files(repo, stage)?;
                }
                if let Some(old) = &self.old_index {
                    repo.write_worktree_index(old)?;
                }
            }
            RefState::AfterCommit => {
                if let Some(stage) = &stage {
                    for (index, change) in self.changes.iter().enumerate() {
                        if self.head.is_none() {
                            let path = resolved_path(repo.root(), &change.path, false)?;
                            if let Some(next) = &change.new {
                                ensure!(
                                    observe(repo, &path)?.as_ref() == Some(next),
                                    "写入路径出现外部变动，已保留 {}",
                                    path.display()
                                );
                            } else {
                                ensure!(
                                    metadata(&path)?.is_none(),
                                    "移出路径出现外部变动，已保留 {}",
                                    path.display()
                                );
                            }
                        }
                        if let Some(old) = &change.old {
                            let backup = staged_path(stage, "old", index);
                            ensure!(
                                observe(repo, &backup)? == Some(old.clone()),
                                "暂存原件出现变动，已保留 {}",
                                backup.display()
                            );
                        }
                    }
                }
                repo.write_worktree_index(&self.new_index)?;
                mark_for_gc(lease)?;
            }
        }
        lease.finish_record()?;
        if let Some(stage) = stage {
            fs::remove_dir_all(stage)?;
        }
        Ok(())
    }
}

pub(crate) fn apply_operation(
    repo: &Repository<'_>,
    lease: &WriteLease,
    references: Vec<RefEdit>,
    plan: WorktreePlan,
    stage: Option<TempDir>,
) -> Result<()> {
    let (mut git, record) = begin_operation(repo, lease, references, plan)?;
    let staged_dir = stage.map(TempDir::keep);
    let plan = record.plan.as_ref().context("缺少工作目录操作")?;
    let outcome = (|| -> Result<()> {
        if let Some(staged_dir) = &staged_dir {
            plan.apply_files(repo, staged_dir)?;
            ensure!(
                scan(repo)? == snapshot_files(repo, &plan.new_index)?,
                "操作期间目录出现外部改动"
            );
        }
        if let Some(head) = &plan.head {
            // reftable 会将原分支校验扩展到 HEAD，原快照在持有书库锁时单独核对。
            ensure!(
                repo.reference(&head.previous)? == plan.old_index,
                "切换前原分支快照已变化"
            );
            git.prepare_switch(
                &head.previous,
                &head.next,
                &plan.new_index,
                &record.references,
            )?;
        } else {
            git.prepare_references(&record.references)?;
        }
        git.commit_prepared()
    })();
    // 以持久引用为准，Git 子进程在提交后发生错误也允许完成索引。
    let state = record.settled_state(repo)?;
    plan.finish(repo, lease, &state)?;
    if state == RefState::BeforeCommit {
        outcome?;
        bail!("Git 未发布书库操作");
    }
    Ok(())
}

fn begin_operation(
    repo: &Repository<'_>,
    lease: &WriteLease,
    references: Vec<RefEdit>,
    plan: WorktreePlan,
) -> Result<(RunningGit, OperationRecord)> {
    ensure!(metadata(&ref_lock(repo))?.is_none(), "Git 引用已被占用");
    let git = repo.start_ref_update()?;
    let record = OperationRecord {
        references,
        writer: Some(git.stamp()),
        started_unlocked: true,
        plan: Some(plan),
    };
    record.create(lease)?;
    Ok((git, record))
}

pub(crate) fn recover_operation(
    repo: &Repository<'_>,
    lease: &WriteLease,
) -> Result<Option<RefState>> {
    let gc_writer = lease.directory().join("gc.writer");
    if let Some(file) = match fs::File::open(&gc_writer) {
        Ok(file) => Some(file),
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => return Err(error.into()),
    } {
        let writer: ProcessStamp = serde_json::from_reader(file)?;
        wait_for_writer(writer)?;
        fs::remove_file(gc_writer)?;
    }
    if let Some(record) = OperationRecord::load_pending(lease)? {
        let state = record.settled_state(repo)?;
        record
            .plan
            .as_ref()
            .context("缺少可恢复的工作目录记录")?
            .finish(repo, lease, &state)?;
        return Ok(Some(state));
    }
    Ok(None)
}

fn wait_for_writer(writer: ProcessStamp) -> Result<()> {
    let mut remaining = 30;
    while writer.is_running()? {
        ensure!(remaining > 0, "上次 Git 写进程仍在运行，已保留操作记录");
        std::thread::sleep(Duration::from_millis(40));
        remaining -= 1;
    }
    Ok(())
}

pub(crate) fn run_recorded_gc(
    repo: &Repository<'_>,
    lease: &WriteLease,
    worker: &Path,
) -> Result<()> {
    let mut git = repo.start_recorded_gc(worker)?;
    let mut staged = tempfile::NamedTempFile::new_in(lease.directory())?;
    serde_json::to_writer(&mut staged, &git.stamp())?;
    staged.flush()?;
    staged.as_file().sync_all()?;
    let writer = lease.directory().join("gc.writer");
    staged.persist_noclobber(&writer)?;
    git.release_gc_worker()?;
    git.finish_gc()?;
    fs::remove_file(writer)?;
    Ok(())
}

pub(crate) fn mark_for_gc(lease: &WriteLease) -> Result<()> {
    fs::File::options()
        .create(true)
        .append(true)
        .open(lease.directory().join("gc.pending"))?
        .sync_all()?;
    Ok(())
}

fn ref_lock(repo: &Repository<'_>) -> PathBuf {
    repo.root().join(".git/reftable/tables.list.lock")
}

fn stage_directory(lease: &WriteLease, name: &str) -> Result<PathBuf> {
    ensure!(
        name.starts_with("switch-") && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'),
        "无效暂存目录"
    );
    let stage = lease.directory.join(name);
    ensure!(
        metadata(&stage)?.is_some_and(|m| m.is_dir() && !is_link(&m)),
        "缺少书库暂存目录 {}",
        stage.display()
    );
    Ok(stage)
}

fn staged_path(stage: &Path, kind: &str, index: usize) -> PathBuf {
    stage.join(kind).join(index.to_string())
}

pub(crate) fn scan(repo: &Repository<'_>) -> Result<Files> {
    scan_source(repo, repo.root(), |_| true)
}

pub(crate) fn scan_source(
    repo: &Repository<'_>,
    root: &Path,
    accepts: impl Fn(&str) -> bool,
) -> Result<Files> {
    let mut files = Files::new();
    scan_dir(repo, root, root, &accepts, &mut files)?;
    Ok(files)
}

pub(crate) fn snapshot_files(repo: &Repository<'_>, commit: &str) -> Result<Files> {
    let data = repo.data(&["ls-tree", "-rz", "--full-tree", commit])?;
    data.split(|byte| *byte == 0)
        .filter(|entry| !entry.is_empty())
        .map(|entry| {
            let entry = std::str::from_utf8(entry)?;
            let (metadata, path) = entry.split_once('\t').context("无效快照条目")?;
            let (mode, oid) = metadata
                .split_once(" blob ")
                .context("快照条目不是普通文件")?;
            ensure!(matches!(mode, "100644" | "100755"), "快照文件模式无效");
            Ok((
                path.to_owned(),
                BlobFile {
                    oid: oid.to_owned(),
                    executable: mode == "100755",
                },
            ))
        })
        .collect()
}

fn scan_dir(
    repo: &Repository<'_>,
    root: &Path,
    directory: &Path,
    accepts: &impl Fn(&str) -> bool,
    files: &mut Files,
) -> Result<()> {
    for entry in
        fs::read_dir(directory).with_context(|| format!("读取目录 {}", directory.display()))?
    {
        let entry = entry?;
        let name = entry.file_name();
        if name.eq_ignore_ascii_case(".git")
            || name == ".DS_Store"
            || (cfg!(windows)
                && (name.eq_ignore_ascii_case("desktop.ini")
                    || name.eq_ignore_ascii_case("Thumbs.db")))
        {
            continue;
        }
        let path = entry.path();
        let info = fs::symlink_metadata(&path)?;
        if is_link(&info) {
            continue;
        }
        if info.is_dir() {
            if metadata(&path.join(".git"))?.is_none() {
                scan_dir(repo, root, &path, accepts, files)?;
            }
        } else if info.is_file() && accepts(name.to_str().context("文件名不是有效的 Unicode")?)
        {
            let relative = path
                .strip_prefix(root)?
                .iter()
                .map(|name| name.to_str().context("文件名不是有效的 Unicode"))
                .collect::<Result<Vec<_>>>()?
                .join("/");
            files.insert(
                relative,
                BlobFile {
                    oid: repo.file_oid(&path, false)?,
                    executable: executable(&info),
                },
            );
        }
    }
    Ok(())
}

pub(crate) fn save_file(repo: &Repository<'_>, path: &str, expected: &BlobFile) -> Result<()> {
    save_from(repo, repo.root(), path, expected)
}

pub(crate) fn save_from(
    repo: &Repository<'_>,
    root: &Path,
    path: &str,
    expected: &BlobFile,
) -> Result<()> {
    let file = resolved_path(root, path, false)?;
    let info = fs::symlink_metadata(&file)?;
    ensure!(
        info.is_file() && !is_link(&info) && executable(&info) == expected.executable,
        "文件已变更 {}",
        file.display()
    );
    ensure!(
        repo.file_oid(&file, true)? == expected.oid,
        "文件已变更，请重新确认 {}",
        file.display()
    );
    Ok(())
}

fn observe(repo: &Repository<'_>, path: &Path) -> Result<Option<BlobFile>> {
    let Some(info) = metadata(path)? else {
        return Ok(None);
    };
    ensure!(
        info.is_file() && !is_link(&info),
        "路径已被占用 {}",
        path.display()
    );
    Ok(Some(BlobFile {
        oid: repo.file_oid(path, false)?,
        executable: executable(&info),
    }))
}

fn metadata(path: &Path) -> io::Result<Option<fs::Metadata>> {
    match fs::symlink_metadata(path) {
        Ok(info) => Ok(Some(info)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn relative_parts(path: &str) -> Result<Vec<&OsStr>> {
    let parts: Vec<_> = Path::new(path)
        .components()
        .map(|component| match component {
            Component::Normal(name) if !name.eq_ignore_ascii_case(".git") => Ok(name),
            _ => bail!("无效书库路径 {path}"),
        })
        .collect::<Result<_>>()?;
    ensure!(!parts.is_empty(), "书库路径为空");
    Ok(parts)
}

pub(crate) fn resolved_path(root: &Path, relative: &str, create_parents: bool) -> Result<PathBuf> {
    let parts = relative_parts(relative)?;
    let mut resolved = root.to_owned();
    for part in &parts[..parts.len() - 1] {
        resolved.push(part);
        match metadata(&resolved)? {
            Some(info) => ensure!(
                info.is_dir() && !is_link(&info),
                "目录路径已被占用 {}",
                resolved.display()
            ),
            None if create_parents => fs::create_dir(&resolved)?,
            None => {}
        }
    }
    resolved.push(parts[parts.len() - 1]);
    Ok(resolved)
}

fn prune_empty_parents(root: &Path, path: &str) -> Result<()> {
    let path = resolved_path(root, path, false)?;
    for parent in path
        .ancestors()
        .skip(1)
        .take_while(|parent| *parent != root)
    {
        match fs::remove_dir(parent) {
            Ok(()) => continue,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) if error.kind() == io::ErrorKind::DirectoryNotEmpty => break,
            Err(error) => {
                return Err(error).with_context(|| format!("整理空目录 {}", parent.display()))
            }
        }
    }
    Ok(())
}

pub(crate) fn is_link(metadata: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        return metadata.file_attributes()
            & windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT
            != 0;
    }
    #[cfg(not(windows))]
    metadata.file_type().is_symlink()
}

fn executable(metadata: &fs::Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o100 != 0
    }
    #[cfg(not(unix))]
    {
        let _ = metadata;
        false
    }
}

fn set_executable(file: &fs::File, executable: bool) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = file.metadata()?.permissions();
        if executable {
            permissions.set_mode(permissions.mode() | 0o100);
            file.set_permissions(permissions)?;
        }
    }
    #[cfg(not(unix))]
    let _ = (file, executable);
    Ok(())
}

fn rename_exclusive(source: &Path, target: &Path) -> Result<()> {
    #[cfg(target_os = "macos")]
    let result = {
        use std::{ffi::CString, os::unix::ffi::OsStrExt};
        let original = CString::new(source.as_os_str().as_bytes())?;
        let destination = CString::new(target.as_os_str().as_bytes())?;
        // 文件在同一卷内改名，目的路径须空闲。
        unsafe { libc::renamex_np(original.as_ptr(), destination.as_ptr(), libc::RENAME_EXCL) == 0 }
    };
    #[cfg(windows)]
    let result = {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Storage::FileSystem::MoveFileExW;
        let original: Vec<_> = source.as_os_str().encode_wide().chain([0]).collect();
        let destination: Vec<_> = target.as_os_str().encode_wide().chain([0]).collect();
        unsafe { MoveFileExW(original.as_ptr(), destination.as_ptr(), 0) != 0 }
    };
    #[cfg(any(target_os = "macos", windows))]
    if !result {
        return Err(io::Error::last_os_error())
            .with_context(|| format!("移动 {} 到 {}", source.display(), target.display()));
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    bail!("当前平台不支持书库文件移动");
    #[cfg(any(target_os = "macos", windows))]
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn lease_releases_lock_even_while_a_descriptor_is_still_open() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("write.lock");
        let guard = fs::File::create_new(&path).unwrap();
        guard.try_lock().unwrap();
        let duplicate = guard.try_clone().unwrap();
        let lease = WriteLease {
            guard,
            directory: temp.path().to_owned(),
        };
        drop(lease);
        let next = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();
        next.try_lock().unwrap();
        drop(duplicate);
    }
}

#[cfg(test)]
pub(crate) fn interrupt_move_for_test(
    repo: &Repository<'_>,
    lease: &WriteLease,
    references: Vec<RefEdit>,
    plan: WorktreePlan,
    stage: TempDir,
    boundary: &str,
) -> Result<()> {
    let (mut git, record) = begin_operation(repo, lease, references, plan)?;
    let stage = stage.keep();
    let plan = record.plan.as_ref().unwrap();
    match boundary {
        "partly-staged" => {
            WorktreePlan::stage_original(repo, &stage, 0, &plan.changes[0])?;
            drop(git);
        }
        "prepared" => {
            plan.apply_files(repo, &stage)?;
            git.prepare_references(&record.references)?;
            git.interrupt()?;
            ensure!(ref_lock(repo).is_file(), "Git 应留下待恢复的引用锁");
        }
        "published" => {
            plan.apply_files(repo, &stage)?;
            git.prepare_references(&record.references)?;
            git.commit_prepared()?;
        }
        _ => bail!("未识别的中断测试阶段 {boundary}"),
    }
    // 跳过操作完成和写锁析构，让另一个进程通过正常开库入口恢复。
    std::process::exit(0)
}
