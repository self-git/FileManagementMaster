use std::{
    fmt,
    sync::atomic::{AtomicU8, Ordering},
    time::{Duration, Instant},
};

use super::{
    novel::{aliases, identity, normalized_leaf},
    *,
};

const SCANNING: u8 = 0;
const STOPPING: u8 = 1;
const PUBLISHING: u8 = 2;
const FINISHED: u8 = 3;

#[derive(Default)]
pub struct ScanControl(AtomicU8);

#[derive(Debug)]
pub(crate) struct ScanStopped;

impl fmt::Display for ScanStopped {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("扫描已停止")
    }
}

impl std::error::Error for ScanStopped {}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum ScanStop {
    Requested,
    Publishing,
    Finished,
}

impl ScanControl {
    pub fn cancel(&self) -> bool {
        matches!(self.request_stop(), ScanStop::Requested)
    }

    pub(crate) fn request_stop(&self) -> ScanStop {
        match self
            .0
            .compare_exchange(SCANNING, STOPPING, Ordering::AcqRel, Ordering::Acquire)
        {
            Ok(_) | Err(STOPPING) => ScanStop::Requested,
            Err(PUBLISHING) => ScanStop::Publishing,
            Err(_) => ScanStop::Finished,
        }
    }

    pub(crate) fn finish(&self) -> bool {
        self.0.swap(FINISHED, Ordering::AcqRel) == STOPPING
    }

    fn checkpoint(&self) -> Result<()> {
        if self.0.load(Ordering::Acquire) == STOPPING {
            return Err(ScanStopped.into());
        }
        Ok(())
    }

    fn publish(&self) -> Result<()> {
        self.0
            .compare_exchange(SCANNING, PUBLISHING, Ordering::AcqRel, Ordering::Acquire)
            .map(|_| ())
            .map_err(|_| ScanStopped.into())
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NovelScanRequest {
    pub directory: PathBuf,
    pub branch: String,
    pub revision: u64,
    pub formats: BTreeSet<NovelFormat>,
}

#[derive(Clone, Copy, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ScanPhase {
    Discovering { found: usize },
    Identifying { processed: usize, total: usize },
    Verifying { total: usize },
    Publishing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
enum Review {
    Unsaved,
    AuthorUnclear,
    AuthorConflict,
    BookConflict,
    HistoryLimit,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScannedFile {
    path: String,
    novel: NovelName,
    saved: bool,
    same_as: Option<String>,
    superseded_by: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScannedBook {
    id: usize,
    title: String,
    author: Option<String>,
    book_id: Option<u64>,
    #[serde(skip)]
    book_ids: BTreeSet<u64>,
    files: Vec<ScannedFile>,
    review: Option<Review>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanResult {
    pub library: LibraryView,
    pub groups: Vec<ScannedBook>,
    pub matched: usize,
}

// 结果只能在生成它的书库修订上提交，文件选择由后端保留。
#[derive(Clone)]
pub struct ScanPlan {
    directory: PathBuf,
    branch: String,
    revision: u64,
    oid: String,
    groups: Vec<ScannedBook>,
    matched: usize,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ScanReviewEdit {
    Merge {
        group: usize,
    },
    Archive {
        group: usize,
        paths: BTreeSet<String>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum NameKey {
    Named(String, String),
    Txt(String),
    File(String),
}

impl NameKey {
    fn for_file(path: &str, novel: &NovelName) -> Self {
        match &novel.author {
            Some(author) => Self::Named(identity(&novel.title), identity(author)),
            None if novel.format == NovelFormat::Txt => Self::Txt(normalized_leaf(path)),
            None => Self::File(path.to_owned()),
        }
    }
}

#[derive(PartialEq, Eq, PartialOrd, Ord)]
enum GroupKey {
    Existing(u64),
    Name(NameKey),
}

impl LibraryEngine {
    pub fn scan_novels(
        &self,
        request: &NovelScanRequest,
        control: &ScanControl,
        progress: impl Fn(ScanPhase),
    ) -> Result<(ScanResult, ScanPlan)> {
        ensure!(!request.formats.is_empty(), "请至少选择一种小说格式");
        progress(ScanPhase::Discovering { found: 0 });
        control.checkpoint()?;
        let repo = self.repository(&request.directory)?;
        let mut state = {
            let _lease = open(&repo)?;
            let state = State::load(&repo)?;
            check_scope(&state, &request.branch, request.revision)?;
            state
        };

        let mut files = BTreeMap::new();
        let mut last_progress = Instant::now();
        worktree::visit_files(
            repo.root(),
            &|leaf| NovelFormat::of(leaf).is_some_and(|format| request.formats.contains(&format)),
            &|| control.checkpoint(),
            &mut |path, _, metadata| {
                files.insert(path, metadata);
                if last_progress.elapsed() >= Duration::from_millis(100) {
                    progress(ScanPhase::Discovering { found: files.len() });
                    last_progress = Instant::now();
                }
                Ok(())
            },
        )?;

        let total = files.len();
        progress(ScanPhase::Identifying {
            processed: 0,
            total,
        });
        let mut drafts = Vec::with_capacity(total);
        let entries = &state.catalog.branch(&state.head)?.entries;
        let book_by_file: BTreeMap<_, _> = state
            .catalog
            .books
            .iter()
            .flat_map(|(&id, book)| book.file_ids.iter().map(move |&file| (file, id)))
            .collect();
        let mut known = BTreeMap::<NameKey, BTreeSet<u64>>::new();
        for (&id, book) in &state.catalog.books {
            control.checkpoint()?;
            if let Some(author) = &book.author {
                known
                    .entry(NameKey::Named(identity(&book.title), identity(author)))
                    .or_default()
                    .insert(id);
            }
        }
        for (index, (path, metadata)) in files.iter().enumerate() {
            control.checkpoint()?;
            let novel = NovelName::parse(path).context("匹配的文件后缀发生变化")?;
            let key = NameKey::for_file(path, &novel);
            let entry = entries.get(path);
            if let Some(book) = entry.and_then(|entry| book_by_file.get(&entry.id)) {
                known.entry(key.clone()).or_default().insert(*book);
            }
            let saved =
                entry.is_some_and(|entry| entry.file.executable == worktree::executable(metadata));
            drafts.push((
                key,
                ScannedFile {
                    path: path.clone(),
                    novel,
                    saved,
                    same_as: None,
                    superseded_by: None,
                },
            ));
            if last_progress.elapsed() >= Duration::from_millis(100) {
                progress(ScanPhase::Identifying {
                    processed: index + 1,
                    total,
                });
                last_progress = Instant::now();
            }
        }

        let mut buckets = BTreeMap::<GroupKey, ScannedBook>::new();
        for (key, file) in drafts {
            control.checkpoint()?;
            let ids = known.get(&key);
            let bucket = match ids {
                Some(ids) if ids.len() == 1 => GroupKey::Existing(*ids.first().unwrap()),
                _ => GroupKey::Name(key),
            };
            let group = buckets.entry(bucket).or_insert_with(|| ScannedBook {
                id: 0,
                title: file.novel.title.clone(),
                author: file.novel.author.clone(),
                book_id: None,
                book_ids: ids.cloned().unwrap_or_default(),
                files: Vec::new(),
                review: None,
            });
            group.files.push(file);
        }
        let mut groups: Vec<_> = buckets.into_values().collect();
        for (id, group) in groups.iter_mut().enumerate() {
            group.id = id;
            if let Some(book) = group
                .book_ids
                .first()
                .and_then(|id| state.catalog.books.get(id))
            {
                group.title.clone_from(&book.title);
                if book.author.is_some() {
                    group.author.clone_from(&book.author);
                }
            }
        }

        let compare_paths = version_paths(&groups, &|| control.checkpoint())?;
        if !compare_paths.is_empty() {
            progress(ScanPhase::Verifying {
                total: compare_paths.len(),
            });
        }
        let oids = repo.file_oids(&compare_paths, &|| control.checkpoint())?;
        let compared: BTreeMap<_, _> = compare_paths
            .into_iter()
            .map(str::to_owned)
            .zip(oids)
            .collect();
        let titles = title_candidates(&groups, &|| control.checkpoint())?;
        let history = current_contents(&state.catalog, &|| control.checkpoint())?;
        let mut changed = false;
        for group in &mut groups {
            control.checkpoint()?;
            let entries = &state.catalog.branch(&state.head)?.entries;
            describe_versions(group, entries, &compared, &|| control.checkpoint())?;
            let members = member_ids(&state.catalog, entries, group);
            let authors: BTreeSet<_> = group
                .book_ids
                .iter()
                .filter_map(|id| state.catalog.books[id].author.as_deref())
                .chain(
                    group
                        .files
                        .iter()
                        .filter_map(|file| file.novel.author.as_deref()),
                )
                .map(identity)
                .collect();
            group.review = if group.files.iter().any(|file| !file.saved) {
                Some(Review::Unsaved)
            } else if authors.len() > 1 {
                Some(Review::AuthorConflict)
            } else if group.book_ids.len() > 1 {
                Some(Review::BookConflict)
            } else if titles.contains(&group.id) {
                Some(Review::AuthorUnclear)
            } else if exceeds_history(&state.catalog, &members, &history, &|| control.checkpoint())?
            {
                Some(Review::HistoryLimit)
            } else {
                None
            };
            if group.review.is_none() {
                let (book, updated) = assign_book(&mut state.catalog, group, members);
                changed |= updated;
                group.book_id = Some(book);
            }
        }

        let lease = open(&repo)?;
        let current = State::load(&repo)?;
        check_scope(&current, &request.branch, request.revision)?;
        ensure!(current.oid == state.oid, "书库已变化，请重新扫描");
        for (path, before) in &files {
            control.checkpoint()?;
            let resolved = worktree::resolved_path(repo.root(), path, false)?;
            ensure!(
                same_metadata(before, &fs::symlink_metadata(resolved)?),
                "扫描期间文件已变化 {path}"
            );
        }
        let oid = if changed {
            worktree::mark_for_gc(&lease)?;
            let next = state.catalog.snapshot(&repo, "扫描并归组书目")?;
            control.checkpoint()?;
            // 自动归组只发布书库目录引用。分支、索引和原始文件无需重写。
            control.publish()?;
            progress(ScanPhase::Publishing);
            repo.update_references(&[RefEdit::replace(STATE_REF, &state.oid, &next)])?;
            next
        } else {
            control.checkpoint()?;
            state.oid.clone()
        };
        let library = state.view(&repo)?;
        Ok(scan_output(library, groups, total, oid))
    }

    pub fn review_novel_scan(
        &self,
        plan: &ScanPlan,
        edits: &[ScanReviewEdit],
        control: &ScanControl,
        progress: impl Fn(ScanPhase),
    ) -> Result<(ScanResult, ScanPlan)> {
        ensure!(!edits.is_empty(), "请选择要处理的书目版本");
        let repo = self.repository(&plan.directory)?;
        let mut state = {
            let _lease = open(&repo)?;
            let state = State::load(&repo)?;
            check_scope(&state, &plan.branch, plan.revision)?;
            ensure!(state.oid == plan.oid, "书库已变化，请重新扫描");
            state
        };
        let mut groups = plan.groups.clone();
        let original = state.catalog.branch(&state.head)?.clone();
        let mut branch = original.clone();
        let mut handled = BTreeSet::new();
        let mut affected = BTreeSet::new();
        let mut archiving = false;
        for edit in edits {
            control.checkpoint()?;
            let id = match edit {
                ScanReviewEdit::Merge { group } | ScanReviewEdit::Archive { group, .. } => *group,
            };
            ensure!(handled.insert(id), "重复选择书目");
            let group = groups.get_mut(id).context("书目已变化，请重新扫描")?;
            match edit {
                ScanReviewEdit::Merge { .. } => {
                    ensure!(
                        matches!(
                            group.review,
                            Some(Review::BookConflict | Review::HistoryLimit)
                        ),
                        "请先核对文件和作者"
                    );
                    // 一个已有书目可能还有其他扫描分组，变更后统一刷新其归属。
                    ensure!(
                        group
                            .book_ids
                            .iter()
                            .all(|id| state.catalog.books.contains_key(id)),
                        "书目已变化，请重新扫描"
                    );
                    affected.extend(group.files.iter().map(|file| file.path.clone()));
                    let members = member_ids(&state.catalog, &original.entries, group);
                    let old_ids = group.book_ids.clone();
                    let (book, _) = assign_book(&mut state.catalog, group, members);
                    group.book_ids = BTreeSet::from([book]);
                    group.book_id = Some(book);
                    group.review = None;
                    for other in &mut groups {
                        if other.id != id && !other.book_ids.is_disjoint(&old_ids) {
                            other.book_ids.retain(|id| !old_ids.contains(id));
                            other.book_ids.insert(book);
                            if other.book_id.is_some_and(|id| old_ids.contains(&id)) {
                                other.book_id = Some(book);
                            }
                        }
                    }
                }
                ScanReviewEdit::Archive { paths, .. } => {
                    ensure!(
                        group.review.is_none() && !paths.is_empty(),
                        "请选择已归组的版本"
                    );
                    let id = group.book_id.context("书目尚未归组")?;
                    let members = state.catalog.books[&id].file_ids.clone();
                    let mut keep = BTreeSet::new();
                    let mut archive = BTreeSet::new();
                    for (path, entry) in &branch.entries {
                        if members.contains(&entry.id) {
                            if let Some(format) = NovelFormat::of(path) {
                                if paths.contains(path) {
                                    archive.insert(format);
                                } else {
                                    keep.insert(format);
                                }
                            }
                        }
                    }
                    ensure!(archive.is_subset(&keep), "请为每种格式保留至少一个当前文件");
                    for path in paths {
                        control.checkpoint()?;
                        ensure!(
                            group.files.iter().any(|file| file.path == *path),
                            "文件不在本次扫描结果中 {path}"
                        );
                        let entry = branch
                            .entries
                            .remove(path)
                            .context("当前文件已变化，请重新扫描")?;
                        ensure!(members.contains(&entry.id), "文件已移出书目 {path}");
                        state.catalog.file_entry(Some(entry.id), entry.file, path);
                    }
                    group.files.retain(|file| !paths.contains(&file.path));
                    for file in &mut group.files {
                        if file
                            .same_as
                            .as_ref()
                            .is_some_and(|path| paths.contains(path))
                        {
                            file.same_as = None;
                        }
                        if file
                            .superseded_by
                            .as_ref()
                            .is_some_and(|path| paths.contains(path))
                        {
                            file.superseded_by = None;
                        }
                    }
                    archiving = true;
                }
            }
        }
        control.checkpoint()?;
        progress(ScanPhase::Verifying {
            total: if archiving {
                original.entries.len()
            } else {
                affected.len()
            },
        });
        if archiving {
            let files = worktree::scan_checked(&repo, &|| control.checkpoint())?;
            ensure!(
                original.files() == files,
                "书库目录有未保存的改动，请保存后重新扫描"
            );
        } else {
            let paths: Vec<_> = affected.iter().map(String::as_str).collect();
            for path in &paths {
                control.checkpoint()?;
                let resolved = worktree::resolved_path(repo.root(), path, false)?;
                let info = fs::symlink_metadata(resolved)?;
                ensure!(
                    info.is_file()
                        && !worktree::is_link(&info)
                        && worktree::executable(&info) == original.entries[*path].file.executable,
                    "文件已变化 {path}"
                );
            }
            let oids = repo.file_oids(&paths, &|| control.checkpoint())?;
            for (path, oid) in paths.into_iter().zip(oids) {
                ensure!(original.entries[path].file.oid == oid, "文件已变化 {path}");
            }
        }
        let lease = open(&repo)?;
        let current = State::load(&repo)?;
        check_scope(&current, &plan.branch, plan.revision)?;
        ensure!(state.oid == current.oid, "书库已变化，请重新扫描");
        let checked = worktree::OperationChecks {
            checkpoint: &|| control.checkpoint(),
            before_publish: &|| {
                control.publish()?;
                progress(ScanPhase::Publishing);
                Ok(())
            },
        };
        let updated = BTreeMap::from([(state.head.clone(), branch)]);
        let library =
            publish_branches_checked(&repo, &lease, state, updated, "整理扫描结果", Some(checked))?;
        let oid = repo.reference(STATE_REF)?.context("书库目录引用已变化")?;
        Ok(scan_output(library, groups, plan.matched, oid))
    }
}

fn check_scope(state: &State, branch: &str, revision: u64) -> Result<()> {
    ensure!(
        state.head == branch && state.catalog.revision == revision,
        "书库已变化，请重新扫描"
    );
    Ok(())
}

fn scan_output(
    library: LibraryView,
    groups: Vec<ScannedBook>,
    matched: usize,
    oid: String,
) -> (ScanResult, ScanPlan) {
    let plan = ScanPlan {
        directory: library.directory.clone(),
        branch: library.current_branch.clone(),
        revision: library.revision,
        oid,
        groups: groups.clone(),
        matched,
    };
    (
        ScanResult {
            library,
            groups,
            matched,
        },
        plan,
    )
}

fn same_metadata(before: &fs::Metadata, after: &fs::Metadata) -> bool {
    if !after.is_file()
        || worktree::is_link(after)
        || before.len() != after.len()
        || before.modified().ok() != after.modified().ok()
        || worktree::executable(before) != worktree::executable(after)
    {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        before.dev() == after.dev()
            && before.ino() == after.ino()
            && before.ctime() == after.ctime()
            && before.ctime_nsec() == after.ctime_nsec()
    }
    #[cfg(not(unix))]
    true
}

fn version_paths<'a>(
    groups: &'a [ScannedBook],
    check: &dyn Fn() -> Result<()>,
) -> Result<Vec<&'a str>> {
    let mut paths = Vec::new();
    for group in groups {
        let mut counts = BTreeMap::new();
        for file in &group.files {
            check()?;
            *counts.entry(file.novel.format).or_insert(0) += 1;
        }
        for file in &group.files {
            if counts[&file.novel.format] > 1 || !file.saved {
                paths.push(file.path.as_str());
            }
        }
    }
    Ok(paths)
}

fn describe_versions(
    group: &mut ScannedBook,
    entries: &BTreeMap<String, Entry>,
    oids: &BTreeMap<String, String>,
    check: &dyn Fn() -> Result<()>,
) -> Result<()> {
    let mut contents = BTreeMap::<(NovelFormat, &str), usize>::new();
    let mut covers = BTreeMap::<(NovelFormat, bool, bool), ((u32, u32, bool), usize)>::new();
    for (index, file) in group.files.iter_mut().enumerate() {
        check()?;
        if let Some(oid) = oids.get(&file.path) {
            file.saved &= entries
                .get(&file.path)
                .is_some_and(|entry| entry.file.oid == *oid);
            contents.entry((file.novel.format, oid)).or_insert(index);
        }
        if let Some((volumes, low, high, completed)) = file.novel.coverage_rank() {
            for requires_completed in [false, true] {
                if !requires_completed || completed {
                    let order = (low, high, completed);
                    let best = covers
                        .entry((file.novel.format, volumes, requires_completed))
                        .or_insert((order, index));
                    if order > best.0 {
                        *best = (order, index);
                    }
                }
            }
        }
    }
    for index in 0..group.files.len() {
        check()?;
        let file = &group.files[index];
        let same = oids
            .get(&file.path)
            .and_then(|oid| contents.get(&(file.novel.format, oid.as_str())))
            .filter(|&&other| other != index)
            .map(|&other| group.files[other].path.clone());
        let newer = file
            .novel
            .coverage_rank()
            .and_then(|(volumes, _, _, completed)| {
                let (_, best) = covers.get(&(file.novel.format, volumes, completed))?;
                group.files[*best]
                    .novel
                    .covers(&file.novel)
                    .then(|| group.files[*best].path.clone())
            });
        group.files[index].same_as = same;
        group.files[index].superseded_by = newer;
    }
    Ok(())
}

fn title_candidates(
    groups: &[ScannedBook],
    check: &dyn Fn() -> Result<()>,
) -> Result<BTreeSet<usize>> {
    let titles: Vec<_> = groups.iter().map(|group| identity(&group.title)).collect();
    let mut indexed = BTreeMap::<&str, Vec<usize>>::new();
    for (id, title) in titles.iter().enumerate() {
        check()?;
        for name in std::iter::once(title.as_str()).chain(aliases(title)) {
            let indices = indexed.entry(name).or_default();
            if indices.last() != Some(&id) {
                indices.push(id);
            }
        }
    }
    let mut pending = BTreeSet::new();
    for group in groups {
        check()?;
        if group.files.len() != 1 || !group.book_ids.is_empty() {
            continue;
        }
        let title = &titles[group.id];
        let author = group.author.as_deref().map(identity);
        let other = std::iter::once(title.as_str())
            .chain(aliases(title))
            .filter_map(|name| indexed.get(name))
            .flatten()
            .copied()
            .any(|id| {
                id != group.id
                    && (author.is_none() || titles[id] != *title)
                    && match (&author, groups[id].author.as_deref()) {
                        (Some(author), Some(candidate)) => *author == identity(candidate),
                        _ => true,
                    }
            });
        if other {
            pending.insert(group.id);
        }
    }
    Ok(pending)
}

fn member_ids(
    catalog: &Catalog,
    entries: &BTreeMap<String, Entry>,
    group: &ScannedBook,
) -> BTreeSet<u64> {
    group
        .book_ids
        .iter()
        .flat_map(|id| catalog.books[id].file_ids.iter().copied())
        .chain(
            group
                .files
                .iter()
                .filter_map(|file| entries.get(&file.path).map(|entry| entry.id)),
        )
        .collect()
}

type CurrentContents = BTreeMap<u64, BTreeSet<(NovelFormat, String)>>;

fn current_contents(catalog: &Catalog, check: &dyn Fn() -> Result<()>) -> Result<CurrentContents> {
    let mut contents = CurrentContents::new();
    for branch in catalog.branches.values() {
        for (path, entry) in &branch.entries {
            check()?;
            if let Some(format) = NovelFormat::of(path) {
                contents
                    .entry(entry.id)
                    .or_default()
                    .insert((format, entry.file.oid.clone()));
            }
        }
    }
    Ok(contents)
}

fn exceeds_history(
    catalog: &Catalog,
    members: &BTreeSet<u64>,
    current: &CurrentContents,
    check: &dyn Fn() -> Result<()>,
) -> Result<bool> {
    let active: BTreeSet<_> = members
        .iter()
        .filter_map(|id| current.get(id))
        .flatten()
        .map(|(format, oid)| (*format, oid.as_str()))
        .collect();
    let mut old = BTreeMap::<NovelFormat, BTreeSet<&str>>::new();
    for id in members {
        check()?;
        for version in catalog.versions.get(id).into_iter().flatten() {
            let Some(format) = version.path.as_deref().and_then(NovelFormat::of) else {
                continue;
            };
            let oid = version.file.oid.as_str();
            if !active.contains(&(format, oid)) {
                let contents = old.entry(format).or_default();
                contents.insert(oid);
                if contents.len() > HISTORY_LIMIT {
                    return Ok(true);
                }
            }
        }
    }
    Ok(false)
}

fn assign_book(catalog: &mut Catalog, group: &ScannedBook, members: BTreeSet<u64>) -> (u64, bool) {
    let id = group
        .book_ids
        .first()
        .copied()
        .unwrap_or_else(|| catalog.next_book_id());
    let mut changed = !catalog.books.contains_key(&id) || group.book_ids.len() > 1;
    for &other in group.book_ids.iter().filter(|&&other| other != id) {
        catalog.books.remove(&other);
    }
    let book = catalog.books.entry(id).or_insert_with(|| Book {
        title: group.title.clone(),
        author: group.author.clone(),
        file_ids: BTreeSet::new(),
    });
    if book.author.is_none() && group.author.is_some() {
        book.author.clone_from(&group.author);
        changed = true;
    }
    changed |= book.file_ids != members;
    book.file_ids = members;
    (id, changed)
}
