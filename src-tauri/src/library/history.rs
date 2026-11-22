use super::{
    movement::{validate_destination, TargetPaths},
    *,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Unit {
    File(u64),
    Novel(u64, NovelFormat),
}

pub(super) struct HistoryIndex<'a> {
    books: BTreeMap<u64, u64>,
    current: BTreeSet<(Unit, &'a str)>,
}

impl<'a> HistoryIndex<'a> {
    pub(super) fn new(branches: &'a BTreeMap<String, Branch>, books: &BTreeMap<u64, Book>) -> Self {
        let mut index = Self {
            books: books
                .iter()
                .flat_map(|(&book, item)| item.file_ids.iter().map(move |&file| (file, book)))
                .collect(),
            current: BTreeSet::new(),
        };
        for branch in branches.values() {
            for (path, entry) in &branch.entries {
                index
                    .current
                    .insert((index.unit(entry.id, Some(path)), &entry.file.oid));
            }
        }
        index
    }

    fn unit(&self, id: u64, path: Option<&str>) -> Unit {
        match (self.books.get(&id), path.and_then(NovelFormat::of)) {
            (Some(&book), Some(format)) => Unit::Novel(book, format),
            _ => Unit::File(id),
        }
    }

    pub(super) fn is_current(&self, id: u64, version: &Version) -> bool {
        self.current.contains(&(
            self.unit(id, version.path.as_deref()),
            version.file.oid.as_str(),
        ))
    }

    pub(super) fn kept_orders(&self, versions: &BTreeMap<u64, Vec<Version>>) -> BTreeSet<u64> {
        let mut old: Vec<_> = versions
            .iter()
            .flat_map(|(&id, versions)| versions.iter().map(move |version| (id, version)))
            .filter(|(id, version)| !self.is_current(*id, version))
            .collect();
        old.sort_unstable_by_key(|(_, version)| std::cmp::Reverse(version.order));
        let mut counted = BTreeMap::<Unit, BTreeSet<&str>>::new();
        let mut kept = BTreeSet::new();
        for (id, version) in old {
            let contents = counted
                .entry(self.unit(id, version.path.as_deref()))
                .or_default();
            if contents.len() < HISTORY_LIMIT && contents.insert(&version.file.oid) {
                kept.insert(version.order);
            }
        }
        kept
    }

    fn old_versions<'b>(&self, catalog: &'b Catalog, unit: Unit) -> Vec<&'b Version> {
        let kept = self.kept_orders(&catalog.versions);
        let mut versions: Vec<_> = catalog
            .versions
            .iter()
            .flat_map(|(&id, versions)| versions.iter().map(move |version| (id, version)))
            .filter_map(|(id, version)| {
                (self.unit(id, version.path.as_deref()) == unit && kept.contains(&version.order))
                    .then_some(version)
            })
            .collect();
        versions.sort_unstable_by_key(|version| std::cmp::Reverse(version.order));
        versions
    }
}

pub(super) fn hydrate_paths(catalog: &mut Catalog) {
    // 前几阶段保存的记录没有文件名，优先以该文件仍保存的实际路径补齐。
    let mut current_paths = BTreeMap::new();
    let mut by_content = BTreeMap::new();
    for branch in catalog.branches.values() {
        for (path, entry) in &branch.entries {
            current_paths.entry(entry.id).or_insert(path);
            by_content
                .entry((entry.id, entry.file.oid.as_str()))
                .or_insert(path);
        }
    }
    for (&id, versions) in &mut catalog.versions {
        for version in versions.iter_mut().filter(|version| version.path.is_none()) {
            version.path = by_content
                .get(&(id, version.file.oid.as_str()))
                .or_else(|| current_paths.get(&id))
                .map(|path| (*path).clone());
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileHistory {
    pub(super) token: String,
    pub(super) current: Vec<CurrentFile>,
    pub(super) versions: Vec<OldVersion>,
}

#[derive(Debug, Serialize)]
pub struct CurrentFile {
    pub(super) branch: String,
    pub(super) path: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OldVersion {
    pub(super) order: u64,
    pub(super) saved_path: String,
    pub(super) destination: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RestoreRequest {
    pub branch: String,
    pub path: String,
    pub token: String,
    pub order: u64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CleanupReport {
    pub before_bytes: u64,
    pub after_bytes: u64,
}

impl LibraryEngine {
    pub fn file_history(&self, directory: &Path, branch: &str, path: &str) -> Result<FileHistory> {
        let repo = self.repository(directory)?;
        let _lease = open(&repo)?;
        let state = State::load(&repo)?;
        ensure!(state.head == branch, "当前分支已变化，请重新选择");
        let entry = state
            .catalog
            .branch(branch)?
            .entries
            .get(path)
            .context("文件不在当前分支")?;
        let index = HistoryIndex::new(&state.catalog.branches, &state.catalog.books);
        let unit = index.unit(entry.id, Some(path));
        let versions = index
            .old_versions(&state.catalog, unit)
            .into_iter()
            .map(|version| OldVersion {
                order: version.order,
                saved_path: version.path.as_deref().unwrap_or(path).to_owned(),
                destination: restored_path(unit, version, path),
            })
            .collect();
        let current = state
            .catalog
            .branches
            .iter()
            .flat_map(|(branch, item)| {
                item.entries
                    .iter()
                    .map(move |(path, entry)| (branch, path, entry))
            })
            .filter(|(_, path, entry)| index.unit(entry.id, Some(path)) == unit)
            .map(|(branch, path, _)| CurrentFile {
                branch: branch.clone(),
                path: path.clone(),
            })
            .collect();
        Ok(FileHistory {
            token: state.oid,
            versions,
            current,
        })
    }

    pub fn restore_version(
        &self,
        directory: &Path,
        request: &RestoreRequest,
    ) -> Result<LibraryView> {
        let repo = self.repository(directory)?;
        let lease = open(&repo)?;
        let mut state = State::load(&repo)?;
        ensure!(
            state.head == request.branch && state.oid == request.token,
            "版本预览已变化，请重新查看"
        );
        let mut branch = state.catalog.branch(&state.head)?.clone();
        ensure!(
            worktree::scan(&repo)? == branch.files(),
            "请先保存当前分支的外部改动"
        );
        let replaced = branch
            .entries
            .remove(&request.path)
            .context("文件不在当前分支")?;
        let index = HistoryIndex::new(&state.catalog.branches, &state.catalog.books);
        let unit = index.unit(replaced.id, Some(&request.path));
        let version = index
            .old_versions(&state.catalog, unit)
            .into_iter()
            .find(|version| version.order == request.order)
            .context("找不到仍保留的旧版")?
            .clone();
        let destination = restored_path(unit, &version, &request.path);
        ensure_destination(&repo, &branch, &request.path, &destination)?;
        let entry = state
            .catalog
            .file_entry(Some(replaced.id), version.file, &destination);
        let saved = state
            .catalog
            .versions
            .get_mut(&entry.id)
            .unwrap()
            .iter_mut()
            .find(|saved| {
                saved.file.oid == entry.file.oid
                    && saved.path.as_deref().and_then(NovelFormat::of)
                        == NovelFormat::of(&destination)
            })
            .context("找不到恢复后的版本记录")?;
        saved.order = state.catalog.next_save_order;
        saved.path = Some(destination.clone());
        state.catalog.next_save_order += 1;
        branch.entries.insert(destination, entry);
        let updated = BTreeMap::from([(state.head.clone(), branch)]);
        publish_branches(&repo, &lease, state, updated, "恢复旧版")
    }

    pub fn collect_garbage(
        &self,
        directory: &Path,
        worker: &Path,
    ) -> Result<Option<CleanupReport>> {
        let repo = self.repository(directory)?;
        let lease = open(&repo)?;
        let pending = lease.directory().join("gc.pending");
        if !pending.try_exists()? {
            return Ok(None);
        }
        let state = State::load(&repo)?;
        let current = state.catalog.branch(&state.head)?;
        if worktree::scan(&repo)? != current.files() {
            return Ok(None);
        }
        repo.run(&["diff", "--cached", "--quiet", &current.commit, "--"])?;
        let before_bytes = repo.object_bytes()?;
        worktree::run_recorded_gc(&repo, &lease, worker)?;
        let after_bytes = repo.object_bytes()?;
        fs::remove_file(pending)?;
        Ok(Some(CleanupReport {
            before_bytes,
            after_bytes,
        }))
    }
}

fn restored_path(unit: Unit, version: &Version, selected: &str) -> String {
    // 小说保留旧版文件名中的进度，所在子目录仍用当前所选文件的目录。
    if matches!(unit, Unit::Novel(_, _)) {
        if let Some(saved) = version
            .path
            .as_deref()
            .and_then(|path| path.rsplit('/').next())
        {
            return selected.rsplit_once('/').map_or_else(
                || saved.to_owned(),
                |(parent, _)| format!("{parent}/{saved}"),
            );
        }
    }
    selected.to_owned()
}

pub(super) fn ensure_destination(
    repo: &Repository<'_>,
    branch: &Branch,
    original: &str,
    destination: &str,
) -> Result<()> {
    validate_destination(repo, original, destination)?;
    if let Some(occupied) = TargetPaths::from_branch(branch).conflict(destination, None) {
        anyhow::bail!("目标路径与现有文件冲突 {destination}（{occupied}）");
    }
    Ok(())
}
