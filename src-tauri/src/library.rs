use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Component, Path, PathBuf},
    time::UNIX_EPOCH,
};

use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};

use crate::{
    git::{Git, RefEdit, Repository},
    worktree::{self, BlobFile, Files, RefState, WorktreePlan, WriteLease},
};

mod history;
mod movement;
mod novel;
mod reading;
mod scan;

pub use history::{CleanupReport, FileHistory, RestoreRequest};
pub use movement::{
    MoveConfirmation, MoveItem, MoveRequest, MoveSelection, MoveStatus, MoveTransition,
};
pub use novel::NovelFormat;
use novel::NovelName;
pub use reading::{ReadRequest, ReadTransition};
pub use scan::{NovelScanRequest, ScanControl, ScanPhase, ScanPlan, ScanReviewEdit, ScanResult};
pub(crate) use scan::{ScanStop, ScanStopped};

const STATE_REF: &str = "refs/filemanagement/state";
const INITIAL_BRANCH: &str = "常看";
const HISTORY_LIMIT: usize = 10;

pub struct LibraryEngine {
    git: Git,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryView {
    pub directory: PathBuf,
    pub current_branch: String,
    pub default_branch: String,
    pub branches: Vec<String>,
    pub files: Vec<String>,
    pub books: Vec<BookView>,
    novels: BTreeMap<String, NovelName>,
    file_details: BTreeMap<String, FileDetails>,
    pub revision: u64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct FileDetails {
    bytes: u64,
    modified_at: Option<u64>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BookView {
    pub id: u64,
    pub title: String,
    pub author: Option<String>,
    pub files: Vec<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryTransition {
    pub library: LibraryView,
    pub pending: Option<SwitchPreview>,
    pub(crate) recovery: Option<RefState>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SwitchPreview {
    pub source_branch: String,
    pub target_branch: String,
    pub token: String,
    pub changes: Vec<ExternalChange>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ChangeKind {
    Added,
    Modified,
    Removed,
    Renamed,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExternalChange {
    pub kind: ChangeKind,
    pub path: String,
    pub previous_path: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Entry {
    id: u64,
    file: BlobFile,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Branch {
    commit: String,
    entries: BTreeMap<String, Entry>,
}

impl Branch {
    fn files(&self) -> Files {
        self.entries
            .iter()
            .map(|(path, entry)| (path.clone(), entry.file.clone()))
            .collect()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Version {
    file: BlobFile,
    order: u64,
    #[serde(default)]
    path: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct Book {
    title: String,
    #[serde(default)]
    author: Option<String>,
    file_ids: BTreeSet<u64>,
}

#[derive(Debug, Serialize, Deserialize)]
struct Catalog {
    schema: u32,
    revision: u64,
    default_branch: String,
    next_file_id: u64,
    next_save_order: u64,
    branches: BTreeMap<String, Branch>,
    versions: BTreeMap<u64, Vec<Version>>,
    #[serde(default)]
    books: BTreeMap<u64, Book>,
}

impl Catalog {
    fn newborn() -> Self {
        Self {
            schema: 1,
            revision: 0,
            default_branch: INITIAL_BRANCH.to_owned(),
            next_file_id: 1,
            next_save_order: 1,
            branches: BTreeMap::new(),
            versions: BTreeMap::new(),
            books: BTreeMap::new(),
        }
    }

    fn branch(&self, name: &str) -> Result<&Branch> {
        self.branches
            .get(name)
            .with_context(|| format!("找不到分支 {name}"))
    }

    fn next_book_id(&self) -> u64 {
        self.books.last_key_value().map_or(1, |(&id, _)| id + 1)
    }

    fn file_entry(&mut self, id: Option<u64>, file: BlobFile, path: &str) -> Entry {
        let id = id.unwrap_or_else(|| {
            let id = self.next_file_id;
            self.next_file_id += 1;
            id
        });
        let versions = self.versions.entry(id).or_default();
        if !versions.iter().any(|version| {
            version.file.oid == file.oid
                && version.path.as_deref().and_then(NovelFormat::of) == NovelFormat::of(path)
        }) {
            versions.push(Version {
                file: file.clone(),
                order: self.next_save_order,
                path: Some(path.to_owned()),
            });
            self.next_save_order += 1;
        }
        Entry { id, file }
    }

    fn retain_history(&mut self) {
        let index = history::HistoryIndex::new(&self.branches, &self.books);
        let kept = index.kept_orders(&self.versions);
        self.versions.retain(|id, versions| {
            versions
                .retain(|version| index.is_current(*id, version) || kept.contains(&version.order));
            versions.sort_by_key(|version| std::cmp::Reverse(version.order));
            !versions.is_empty()
        });
    }

    fn snapshot(&mut self, repo: &Repository<'_>, message: &str) -> Result<String> {
        self.revision += 1;
        self.retain_history();
        let catalog_oid = repo.write_blob(&serde_json::to_vec(self)?)?;
        let index = history::HistoryIndex::new(&self.branches, &self.books);
        let mut entries = vec![("catalog.json".to_owned(), catalog_oid.as_str(), false)];
        for (id, versions) in &self.versions {
            for version in versions {
                if !index.is_current(*id, version) {
                    // 保留文件直接挂在 Git 树上，供对象回收判断可达性。
                    entries.push((
                        format!("history/{id}/{}", version.order),
                        version.file.oid.as_str(),
                        version.file.executable,
                    ));
                }
            }
        }
        repo.snapshot_modes(
            entries
                .iter()
                .map(|(path, oid, executable)| (path.as_str(), *oid, *executable)),
            message,
        )
    }
}

struct State {
    oid: String,
    head: String,
    catalog: Catalog,
}

impl State {
    fn load(repo: &Repository<'_>) -> Result<Self> {
        let oid = repo.reference(STATE_REF)?.context("书库尚未完成纳管")?;
        let mut catalog: Catalog =
            serde_json::from_slice(&repo.data(&["show", &format!("{oid}:catalog.json")])?)?;
        ensure!(catalog.schema == 1, "不支持此书库格式");
        catalog.branch(&catalog.default_branch)?;
        let head = repo.run(&["symbolic-ref", "HEAD"])?;
        let head = head
            .strip_prefix("refs/heads/")
            .context("书库 HEAD 未指向分支")?
            .to_owned();
        catalog.branch(&head)?;
        let refs = repo.run(&[
            "for-each-ref",
            "--format=%(refname)%09%(objectname)",
            "refs/heads",
        ])?;
        let actual: BTreeMap<_, _> = refs
            .lines()
            .map(|line| line.split_once('\t').context("书库分支格式无效"))
            .collect::<Result<_>>()?;
        ensure!(
            actual.len() == catalog.branches.len(),
            "书库分支与目录记录不一致"
        );
        for (name, branch) in &catalog.branches {
            ensure!(
                actual.get(refname(name).as_str()) == Some(&branch.commit.as_str()),
                "书库分支已发生外部变化 {name}"
            );
        }
        history::hydrate_paths(&mut catalog);
        Ok(Self { oid, head, catalog })
    }

    fn view(&self, repo: &Repository<'_>) -> Result<LibraryView> {
        let entries = &self.catalog.branch(&self.head)?.entries;
        let book_by_file: BTreeMap<_, _> = self
            .catalog
            .books
            .iter()
            .flat_map(|(&id, book)| book.file_ids.iter().map(move |&file| (file, (id, book))))
            .collect();
        let mut books = BTreeMap::new();
        for (path, entry) in entries {
            if let Some(&(id, book)) = book_by_file.get(&entry.id) {
                books
                    .entry(id)
                    .or_insert_with(|| BookView {
                        id,
                        title: book.title.clone(),
                        author: book.author.clone(),
                        files: Vec::new(),
                    })
                    .files
                    .push(path.clone());
            }
        }
        Ok(LibraryView {
            directory: repo.root().to_owned(),
            current_branch: self.head.clone(),
            default_branch: self.catalog.default_branch.clone(),
            branches: self.catalog.branches.keys().cloned().collect(),
            files: entries.keys().cloned().collect(),
            books: books.into_values().collect(),
            novels: entries
                .keys()
                .filter_map(|path| NovelName::parse(path).map(|name| (path.clone(), name)))
                .collect(),
            // 文件清单来自 HEAD，大小和修改时间取自当前工作目录中的对应常规文件。
            file_details: entries
                .keys()
                .filter_map(|path| {
                    let resolved = worktree::resolved_path(repo.root(), path, false).ok()?;
                    let info = fs::symlink_metadata(resolved).ok()?;
                    (info.is_file() && !worktree::is_link(&info)).then(|| {
                        (path.clone(), FileDetails {
                            bytes: info.len(),
                            modified_at: info.modified().ok()
                                .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                                .map(|duration| duration.as_millis() as u64),
                        })
                    })
                })
                .collect(),
            revision: self.catalog.revision,
        })
    }
}

impl LibraryEngine {
    pub fn new(runtime: &Path) -> Result<Self> {
        let git = Git::bundled(runtime)?;
        git.version(runtime)?;
        Ok(Self { git })
    }

    pub fn adopt_or_open(&self, directory: &Path) -> Result<LibraryTransition> {
        let repo = self.repository(directory)?;
        if !worktree::owned_library(&repo)? {
            worktree::claim_library(&repo)?;
        }
        let mut lease = lock(&repo)?;
        // 初次纳管中途退出时，先按已持久化的提交恢复，再检查初始状态。
        if repo.root().join(".git").join("reftable").is_dir() {
            lease.recovered = worktree::recover_operation(&repo, &lease)?;
            if repo.reference(STATE_REF)?.is_some() {
                let state = State::load(&repo)?;
                let target = state.catalog.default_branch.clone();
                return switch_locked(&repo, &lease, state, &target, None);
            }
        }

        repo.initialize(INITIAL_BRANCH)?;
        ensure!(
            repo.run(&["rev-parse", "--show-ref-format"])? == "reftable",
            "书库引用格式不匹配"
        );
        ensure!(
            repo.reference(&refname(INITIAL_BRANCH))?.is_none(),
            "初始分支已有其他提交"
        );
        let mut catalog = Catalog::newborn();
        let mut entries = BTreeMap::new();
        for (path, file) in worktree::scan(&repo)? {
            worktree::save_file(&repo, &path, &file)?;
            let entry = catalog.file_entry(None, file, &path);
            entries.insert(path, entry);
        }
        let commit = snapshot_branch(&repo, &entries, "初次纳管")?;
        catalog.branches.insert(
            INITIAL_BRANCH.to_owned(),
            Branch {
                commit: commit.clone(),
                entries,
            },
        );
        let state = catalog.snapshot(&repo, "创建书库目录")?;
        worktree::apply_operation(
            &repo,
            &lease,
            vec![
                RefEdit::create(&refname(INITIAL_BRANCH), &commit),
                RefEdit::create(STATE_REF, &state),
            ],
            WorktreePlan::index(None, commit),
            None,
        )?;
        completed(&repo, &lease)
    }

    pub fn create(&self, parent: &Path, name: &str) -> Result<LibraryTransition> {
        ensure!(name == name.trim() && !name.is_empty(), "请输入书库名称");
        ensure!(
            Path::new(name)
                .components()
                .eq([Component::Normal(name.as_ref())])
                && !name.eq_ignore_ascii_case(".git")
                && !name
                    .chars()
                    .any(|c| c.is_control() || "\\/:*?\"<>|".contains(c))
                && !name.ends_with('.'),
            "书库名称必须是一个有效的目录名"
        );
        let parent = self.repository(parent)?;
        let destination = parent.root().join(name);
        fs::create_dir(&destination)
            .with_context(|| format!("创建独立书库 {}", destination.display()))?;
        self.adopt_or_open(&destination)
    }

    pub fn open_default(&self, directory: &Path) -> Result<LibraryTransition> {
        let repo = self.repository(directory)?;
        let lease = open(&repo)?;
        let state = State::load(&repo)?;
        let target = state.catalog.default_branch.clone();
        switch_locked(&repo, &lease, state, &target, None)
    }

    pub fn switch(
        &self,
        directory: &Path,
        target: &str,
        token: Option<&str>,
    ) -> Result<LibraryTransition> {
        let repo = self.repository(directory)?;
        let lease = open(&repo)?;
        switch_locked(&repo, &lease, State::load(&repo)?, target, token)
    }

    pub fn create_branch(&self, directory: &Path, name: &str) -> Result<LibraryView> {
        let repo = self.repository(directory)?;
        let lease = open(&repo)?;
        ensure!(
            repo.run(&["check-ref-format", "--branch", name])
                .with_context(|| format!("无效分支名称 {name}"))?
                == name,
            "无效分支名称 {name}"
        );
        let mut state = State::load(&repo)?;
        ensure!(
            !state.catalog.branches.contains_key(name),
            "分支已存在 {name}"
        );
        let commit = repo.snapshot([], "创建空分支")?;
        state.catalog.branches.insert(
            name.to_owned(),
            Branch {
                commit: commit.clone(),
                entries: BTreeMap::new(),
            },
        );
        let active_commit = state.catalog.branch(&state.head)?.commit.clone();
        let next = state.catalog.snapshot(&repo, "登记书库分支")?;
        worktree::apply_operation(
            &repo,
            &lease,
            vec![
                RefEdit::create(&refname(name), &commit),
                RefEdit::replace(STATE_REF, &state.oid, &next),
            ],
            WorktreePlan::index(Some(active_commit.clone()), active_commit),
            None,
        )?;
        State::load(&repo)?.view(&repo)
    }

    pub fn set_default(&self, directory: &Path, name: &str) -> Result<LibraryView> {
        let repo = self.repository(directory)?;
        let lease = open(&repo)?;
        let mut state = State::load(&repo)?;
        state.catalog.branch(name)?;
        if name != state.catalog.default_branch {
            state.catalog.default_branch = name.to_owned();
            let active_commit = state.catalog.branch(&state.head)?.commit.clone();
            let next = state.catalog.snapshot(&repo, "设置默认分支")?;
            worktree::apply_operation(
                &repo,
                &lease,
                vec![RefEdit::replace(STATE_REF, &state.oid, &next)],
                WorktreePlan::index(Some(active_commit.clone()), active_commit),
                None,
            )?;
        }
        State::load(&repo)?.view(&repo)
    }

    pub fn directory_for_open(&self, directory: &Path) -> Result<PathBuf> {
        let repo = self.repository(directory)?;
        ensure!(worktree::owned_library(&repo)?, "此目录不是已纳管的书库");
        Ok(repo.root().to_owned())
    }

    pub fn path_for_open(&self, directory: &Path, branch: &str, path: &str) -> Result<PathBuf> {
        let repo = self.repository(directory)?;
        let _lease = open(&repo)?;
        let state = State::load(&repo)?;
        ensure!(state.head == branch, "当前分支已变化，请重新选择");
        let entries = &state.catalog.branch(branch)?.entries;
        let is_file = entries.contains_key(path);
        let prefix = format!("{path}/");
        ensure!(
            is_file || entries.range(prefix.clone()..).next().is_some_and(|(entry, _)| entry.starts_with(&prefix)),
            "路径不在当前分支 {path}"
        );
        let resolved = worktree::resolved_path(repo.root(), path, false)?;
        let info = fs::symlink_metadata(&resolved)
            .with_context(|| format!("实际目录中找不到 {path}"))?;
        ensure!(
            !worktree::is_link(&info) && if is_file { info.is_file() } else { info.is_dir() },
            "实际路径已变更 {path}"
        );
        Ok(resolved)
    }

    fn repository(&self, directory: &Path) -> Result<Repository<'_>> {
        ensure!(directory.is_absolute(), "请选择有效的书库目录");
        let root = directory
            .canonicalize()
            .with_context(|| format!("读取目录 {}", directory.display()))?;
        ensure!(root.is_dir(), "请选择一个目录");
        Ok(Repository::at(&self.git, root))
    }
}

fn lock(repo: &Repository<'_>) -> Result<WriteLease> {
    WriteLease::try_acquire(repo)?.context("该书库正在执行另一项操作，请稍后重试")
}

fn open(repo: &Repository<'_>) -> Result<WriteLease> {
    ensure!(worktree::owned_library(repo)?, "此目录不是已纳管的书库");
    let mut lease = lock(repo)?;
    ensure!(
        repo.run(&["rev-parse", "--show-ref-format"])? == "reftable",
        "书库引用格式不匹配"
    );
    lease.recovered = worktree::recover_operation(repo, &lease)?;
    Ok(lease)
}

fn refname(branch: &str) -> String {
    format!("refs/heads/{branch}")
}

fn snapshot_branch(
    repo: &Repository<'_>,
    entries: &BTreeMap<String, Entry>,
    message: &str,
) -> Result<String> {
    repo.snapshot_modes(
        entries.iter().map(|(path, entry)| {
            (
                path.as_str(),
                entry.file.oid.as_str(),
                entry.file.executable,
            )
        }),
        message,
    )
}

fn publish_branches(
    repo: &Repository<'_>,
    lease: &WriteLease,
    state: State,
    updated: BTreeMap<String, Branch>,
    message: &str,
) -> Result<LibraryView> {
    publish_branches_checked(repo, lease, state, updated, message, None)
}

fn publish_branches_checked(
    repo: &Repository<'_>,
    lease: &WriteLease,
    mut state: State,
    updated: BTreeMap<String, Branch>,
    message: &str,
    checks: Option<worktree::OperationChecks<'_>>,
) -> Result<LibraryView> {
    let previous_head = state.catalog.branch(&state.head)?.clone();
    let mut edits = Vec::new();
    for (name, mut branch) in updated {
        let previous = state.catalog.branch(&name)?;
        let files = branch.files();
        if files != previous.files() {
            branch.commit = snapshot_branch(repo, &branch.entries, message)?;
            ensure!(
                worktree::snapshot_files(repo, &branch.commit)? == files,
                "分支包含无法写入 Git 的路径 {name}"
            );
            edits.push(RefEdit::replace(
                &refname(&name),
                &previous.commit,
                &branch.commit,
            ));
        }
        state.catalog.branches.insert(name, branch);
    }
    let head = state.catalog.branch(&state.head)?;
    let (plan, stage) = if head.commit != previous_head.commit {
        let (plan, stage) = WorktreePlan::update(
            repo,
            lease,
            (&previous_head.commit, &head.commit),
            &previous_head.files(),
            &head.files(),
        )?;
        (plan, Some(stage))
    } else {
        (
            WorktreePlan::index(Some(head.commit.clone()), head.commit.clone()),
            None,
        )
    };
    let next = state.catalog.snapshot(repo, message)?;
    edits.push(RefEdit::replace(STATE_REF, &state.oid, &next));
    worktree::apply_operation_checked(repo, lease, edits, plan, stage, checks)?;
    State::load(repo)?.view(repo)
}

fn completed(repo: &Repository<'_>, lease: &WriteLease) -> Result<LibraryTransition> {
    Ok(LibraryTransition {
        library: State::load(repo)?.view(repo)?,
        pending: None,
        recovery: lease.recovered,
    })
}

fn switch_locked(
    repo: &Repository<'_>,
    lease: &WriteLease,
    mut state: State,
    target: &str,
    token: Option<&str>,
) -> Result<LibraryTransition> {
    state.catalog.branch(target)?;
    if target == state.head {
        return Ok(LibraryTransition {
            library: state.view(repo)?,
            pending: None,
            recovery: lease.recovered,
        });
    }
    let mut observed = worktree::scan(repo)?;
    let changes = external_changes(&state.catalog.branch(&state.head)?.entries, &observed);
    if !changes.is_empty() {
        let pending = preview(&state, target, &observed, changes)?;
        if token != Some(pending.token.as_str()) {
            return Ok(LibraryTransition {
                library: state.view(repo)?,
                pending: Some(pending),
                recovery: lease.recovered,
            });
        }
        save_external(repo, lease, &mut state, observed, &pending.changes)?;
        observed = worktree::scan(repo)?;
        let changes = external_changes(&state.catalog.branch(&state.head)?.entries, &observed);
        if !changes.is_empty() {
            return Ok(LibraryTransition {
                library: state.view(repo)?,
                pending: Some(preview(&state, target, &observed, changes)?),
                recovery: lease.recovered,
            });
        }
    }

    let previous = state.catalog.branch(&state.head)?;
    let next = state.catalog.branch(target)?;
    let old_ref = refname(&state.head);
    let new_ref = refname(target);
    let (plan, stage) = WorktreePlan::switch(
        repo,
        lease,
        (&old_ref, &new_ref),
        (&previous.commit, &next.commit),
        &previous.files(),
        &next.files(),
    )?;
    let next_state = state.catalog.snapshot(repo, "切换书库分支")?;
    worktree::apply_operation(
        repo,
        lease,
        vec![RefEdit::replace(STATE_REF, &state.oid, &next_state)],
        plan,
        Some(stage),
    )?;
    completed(repo, lease)
}

fn preview(
    state: &State,
    target: &str,
    observed: &Files,
    changes: Vec<ExternalChange>,
) -> Result<SwitchPreview> {
    // 确认票据包含源状态、目标和实际文件内容。外部文件再次变动会得到新预览。
    let token = serde_json::to_string(&(&state.oid, &state.head, target, observed))?;
    Ok(SwitchPreview {
        source_branch: state.head.clone(),
        target_branch: target.to_owned(),
        token,
        changes,
    })
}

fn save_external(
    repo: &Repository<'_>,
    lease: &WriteLease,
    state: &mut State,
    observed: Files,
    changes: &[ExternalChange],
) -> Result<()> {
    let previous = state.catalog.branch(&state.head)?.clone();
    let renamed: BTreeMap<_, _> = changes
        .iter()
        .filter_map(|change| {
            change
                .previous_path
                .as_deref()
                .map(|old| (change.path.as_str(), old))
        })
        .collect();
    let mut entries = BTreeMap::new();
    for (path, file) in observed {
        let existing = previous.entries.get(&path).or_else(|| {
            renamed
                .get(path.as_str())
                .and_then(|old| previous.entries.get(*old))
        });
        if previous
            .entries
            .get(&path)
            .is_none_or(|old| old.file != file)
        {
            worktree::save_file(repo, &path, &file)?;
        }
        let entry = state
            .catalog
            .file_entry(existing.map(|entry| entry.id), file, &path);
        entries.insert(path, entry);
    }
    let commit = snapshot_branch(repo, &entries, "保存外部变动")?;
    state.catalog.branches.insert(
        state.head.clone(),
        Branch {
            commit: commit.clone(),
            entries,
        },
    );
    let next = state.catalog.snapshot(repo, "保存原分支的实际文件")?;
    worktree::apply_operation(
        repo,
        lease,
        vec![
            RefEdit::replace(&refname(&state.head), &previous.commit, &commit),
            RefEdit::replace(STATE_REF, &state.oid, &next),
        ],
        WorktreePlan::index(Some(previous.commit), commit),
        None,
    )?;
    state.oid = next;
    Ok(())
}

fn external_changes(previous: &BTreeMap<String, Entry>, current: &Files) -> Vec<ExternalChange> {
    let removed: Vec<_> = previous
        .iter()
        .filter(|(path, _)| !current.contains_key(*path))
        .collect();
    let added: Vec<_> = current
        .iter()
        .filter(|(path, _)| !previous.contains_key(*path))
        .collect();
    let mut changes = Vec::new();
    let mut renamed = BTreeSet::new();
    for (path, file) in &added {
        let matches: Vec<_> = removed
            .iter()
            .filter(|(_, old)| old.file == **file)
            .collect();
        let copies = added.iter().filter(|(_, new)| *new == *file).count();
        let previous_path = if matches.len() == 1 && copies == 1 {
            renamed.insert(matches[0].0.as_str());
            Some(matches[0].0.clone())
        } else {
            None
        };
        changes.push(ExternalChange {
            kind: if previous_path.is_some() {
                ChangeKind::Renamed
            } else {
                ChangeKind::Added
            },
            path: (*path).clone(),
            previous_path,
        });
    }
    for (path, entry) in previous {
        if let Some(file) = current.get(path) {
            if file != &entry.file {
                changes.push(ExternalChange {
                    kind: ChangeKind::Modified,
                    path: path.clone(),
                    previous_path: None,
                });
            }
        } else if !renamed.contains(path.as_str()) {
            changes.push(ExternalChange {
                kind: ChangeKind::Removed,
                path: path.clone(),
                previous_path: None,
            });
        }
    }
    changes.sort_by(|a, b| a.path.cmp(&b.path));
    changes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_keeps_ten_old_contents_and_both_current_contents() {
        let mut catalog = Catalog::newborn();
        let entries: Vec<_> = (0..15)
            .map(|i| {
                catalog.file_entry(
                    Some(1),
                    BlobFile {
                        oid: format!("content-{i}"),
                        executable: false,
                    },
                    "测试.txt",
                )
            })
            .collect();
        for (name, entry) in [("常看", &entries[14]), ("收藏", &entries[1])] {
            catalog.branches.insert(
                name.to_owned(),
                Branch {
                    commit: name.to_owned(),
                    entries: BTreeMap::from([("测试.txt".to_owned(), entry.clone())]),
                },
            );
        }
        catalog.retain_history();
        let live = history::HistoryIndex::new(&catalog.branches, &catalog.books);
        let versions = &catalog.versions[&1];
        assert_eq!(versions.len(), 12);
        assert_eq!(
            versions.iter().filter(|v| !live.is_current(1, v)).count(),
            10
        );
        assert!(versions.iter().any(|v| v.file.oid == "content-1"));
    }
}
