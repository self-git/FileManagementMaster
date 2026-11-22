use super::{
    history::ensure_destination,
    movement::{path_key, validate_destination},
    novel::{identity, possibly_same},
    *,
};

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ReadSource {
    Library,
    Directory { directory: PathBuf },
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadRequest {
    pub source: ReadSource,
    // None 是按相对路径读取普通文件。小说扫描只接受勾选的格式。
    pub formats: Option<BTreeSet<NovelFormat>>,
    pub confirmation: Option<ReadConfirmation>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ReadConfirmation {
    SaveExternal {
        token: String,
    },
    Apply {
        token: String,
        selections: Vec<ReadSelection>,
    },
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadSelection {
    pub path: String,
    pub book: Option<String>,
    pub branch: String,
    pub action: ReadAction,
    pub destination: String,
    pub replaces: Vec<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ReadAction {
    Keep,
    History,
}

#[derive(Debug, Serialize)]
pub struct ReadTransition {
    pub library: LibraryView,
    pub status: ReadStatus,
}

#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ReadStatus {
    Preview { preview: ReadPreview },
    SavedExternal,
    Applied,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadPreview {
    source: PathBuf,
    token: String,
    external_changes: Vec<ExternalChange>,
    items: Vec<ReadItem>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ReadItem {
    path: String,
    novel: Option<NovelName>,
    books: Vec<ReadBook>,
    preferred_book: Option<String>,
    superseded_by: Option<String>,
    same_paths: Vec<ReadLocation>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ReadBook {
    key: String,
    title: String,
    author: Option<String>,
    existing: bool,
    locations: Vec<ReadLocation>,
}

#[derive(Debug, Serialize)]
struct ReadLocation {
    branch: String,
    path: String,
    relation: Relationship,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
enum Relationship {
    Same,
    Newer,
    Older,
    Review,
}

struct Group {
    id: Option<u64>,
    title: String,
    author: Option<String>,
    members: BTreeSet<u64>,
    locations: Vec<Located>,
}

struct Located {
    branch: String,
    path: String,
    name: NovelName,
    file: BlobFile,
}

impl Group {
    fn new(title: &str, author: Option<&str>) -> Self {
        Self {
            id: None,
            title: title.to_owned(),
            author: author.map(str::to_owned),
            members: BTreeSet::new(),
            locations: Vec::new(),
        }
    }

    fn exact(&self, name: &NovelName) -> bool {
        let Some(author) = name.author.as_deref().map(identity) else {
            return false;
        };
        let title = identity(&name.title);
        let exact = |other_title: &str, other_author: Option<&str>| {
            identity(other_title) == title
                && other_author.is_some_and(|value| identity(value) == author)
        };
        exact(&self.title, self.author.as_deref())
            || self
                .locations
                .iter()
                .any(|item| exact(&item.name.title, item.name.author.as_deref()))
    }

    fn related(&self, name: &NovelName) -> bool {
        let matches = |title: &str, author: Option<&str>| {
            possibly_same(title, &name.title)
                && match (name.author.as_deref(), author) {
                    (Some(a), Some(b)) => possibly_same(a, b),
                    _ => true,
                }
        };
        matches(&self.title, self.author.as_deref())
            || self
                .locations
                .iter()
                .any(|item| matches(&item.name.title, item.name.author.as_deref()))
    }
}

impl LibraryEngine {
    pub fn read_directory(
        &self,
        directory: &Path,
        request: &ReadRequest,
    ) -> Result<ReadTransition> {
        let repo = self.repository(directory)?;
        let lease = open(&repo)?;
        let mut state = State::load(&repo)?;
        let source = match &request.source {
            ReadSource::Library => repo.root().to_owned(),
            ReadSource::Directory { directory } => {
                let source = self.repository(directory)?.root().to_owned();
                ensure!(
                    !source.starts_with(repo.root()),
                    "读取书库内的文件请使用扫描当前分支"
                );
                source
            }
        };
        let local = matches!(request.source, ReadSource::Library);
        if let Some(formats) = &request.formats {
            ensure!(!formats.is_empty(), "请至少选择一种小说格式");
        }
        let accepts = |path: &str| {
            request.formats.as_ref().is_none_or(|formats| {
                NovelFormat::of(path).is_some_and(|format| formats.contains(&format))
            })
        };
        let observed = worktree::scan(&repo)?;
        let external_changes =
            external_changes(&state.catalog.branch(&state.head)?.entries, &observed);
        let inputs = if local {
            observed
                .iter()
                .filter(|(path, _)| accepts(path))
                .map(|(path, file)| (path.clone(), file.clone()))
                .collect()
        } else {
            worktree::scan_source(&repo, &source, accepts)?
        };
        // 确认绑定来源字节、工作目录状态、格式选择和两类 Git 引用。
        let token = serde_json::to_string(&(
            &state.oid,
            &state.head,
            &source,
            &request.formats,
            &observed,
            &inputs,
        ))?;
        if !external_changes.is_empty() {
            if let Some(ReadConfirmation::SaveExternal { token: accepted }) = &request.confirmation
            {
                if *accepted == token {
                    save_external(&repo, &lease, &mut state, observed, &external_changes)?;
                    return Ok(ReadTransition {
                        library: state.view(&repo)?,
                        status: ReadStatus::SavedExternal,
                    });
                }
            }
        }
        let (items, groups) = if request.formats.is_some() {
            novel_preview(&state, &inputs, local)
        } else {
            (ordinary_preview(&state, &inputs), BTreeMap::new())
        };
        if let Some(ReadConfirmation::Apply {
            token: accepted,
            selections,
        }) = &request.confirmation
        {
            if *accepted == token && external_changes.is_empty() {
                let read = ReadInput {
                    source: &source,
                    local,
                    files: &inputs,
                    items: &items,
                    groups: &groups,
                };
                let library = apply_read(&repo, &lease, state, &read, selections)?;
                return Ok(ReadTransition {
                    library,
                    status: ReadStatus::Applied,
                });
            }
        }
        let preview = ReadPreview {
            source,
            token,
            external_changes,
            items,
        };
        Ok(ReadTransition {
            library: state.view(&repo)?,
            status: ReadStatus::Preview { preview },
        })
    }
}

fn group_key(name: &NovelName, separate: &str) -> String {
    name.author.as_deref().map_or_else(
        || separate.to_owned(),
        |author| format!("name/{:?}/{:?}", identity(&name.title), identity(author)),
    )
}

fn exact_key(groups: &BTreeMap<String, Group>, name: &NovelName) -> Option<String> {
    let mut matches = groups.iter().filter(|(_, group)| group.exact(name));
    let (key, _) = matches.next()?;
    matches.next().is_none().then(|| key.clone())
}

fn known_groups(state: &State) -> (BTreeMap<String, Group>, BTreeMap<u64, String>) {
    let mut by_file = BTreeMap::new();
    let mut groups = BTreeMap::new();
    for (&id, book) in &state.catalog.books {
        let key = format!("book/{id}");
        let mut group = Group::new(&book.title, book.author.as_deref());
        group.id = Some(id);
        group.members = book.file_ids.clone();
        for &file in &book.file_ids {
            by_file.insert(file, key.clone());
        }
        groups.insert(key, group);
    }
    for (branch, item) in &state.catalog.branches {
        for (path, entry) in &item.entries {
            let Some(name) = NovelName::parse(path) else {
                continue;
            };
            let key = by_file.entry(entry.id).or_insert_with(|| {
                exact_key(&groups, &name)
                    .unwrap_or_else(|| group_key(&name, &format!("file/{}", entry.id)))
            });
            let group = groups
                .entry(key.clone())
                .or_insert_with(|| Group::new(&name.title, name.author.as_deref()));
            group.members.insert(entry.id);
            group.locations.push(Located {
                name,
                branch: branch.clone(),
                path: path.clone(),
                file: entry.file.clone(),
            });
        }
    }
    (groups, by_file)
}

fn relationship(name: &NovelName, file: &BlobFile, known: &Located) -> Relationship {
    if file.oid == known.file.oid {
        Relationship::Same
    } else if name.covers(&known.name) {
        Relationship::Newer
    } else if known.name.covers(name) {
        Relationship::Older
    } else {
        Relationship::Review
    }
}

fn novel_preview(
    state: &State,
    inputs: &Files,
    local: bool,
) -> (Vec<ReadItem>, BTreeMap<String, Group>) {
    let (mut groups, by_file) = known_groups(state);
    let entries = &state.catalog.branches[&state.head].entries;
    let mut rows = Vec::new();
    for (path, file) in inputs {
        let Some(name) = NovelName::parse(path) else {
            continue;
        };
        let own = if local {
            entries
                .get(path)
                .and_then(|entry| by_file.get(&entry.id))
                .cloned()
        } else {
            None
        };
        let key = own
            .clone()
            .or_else(|| exact_key(&groups, &name))
            .unwrap_or_else(|| group_key(&name, &format!("input/{path}")));
        groups
            .entry(key.clone())
            .or_insert_with(|| Group::new(&name.title, name.author.as_deref()));
        rows.push((path, file, name, key, own));
    }
    let mut items = Vec::new();
    for (path, file, name, key, own) in &rows {
        let assigned = own.as_ref().filter(|key| groups[*key].id.is_some());
        let books: Vec<_> = groups
            .iter()
            .filter(|(candidate, group)| {
                assigned.map_or_else(
                    || *candidate == key || group.related(name),
                    |assigned| *candidate == assigned,
                )
            })
            .map(|(key, group)| ReadBook {
                key: key.clone(),
                title: group.title.clone(),
                author: group.author.clone(),
                existing: group.id.is_some() || !group.locations.is_empty(),
                locations: group
                    .locations
                    .iter()
                    .filter(|known| {
                        known.name.format == name.format
                            && !(local && known.branch == state.head && known.path == **path)
                    })
                    .map(|known| ReadLocation {
                        branch: known.branch.clone(),
                        path: known.path.clone(),
                        relation: relationship(name, file, known),
                    })
                    .collect(),
            })
            .collect();
        let exact = exact_key(&groups, name).filter(|key| !groups[key].locations.is_empty());
        let preferred_book = assigned
            .cloned()
            .or(exact)
            .or_else(|| (books.len() == 1).then(|| books[0].key.clone()));
        items.push(ReadItem {
            path: (*path).clone(),
            novel: Some(name.clone()),
            books,
            preferred_book,
            superseded_by: None,
            same_paths: Vec::new(),
        });
    }
    for i in 0..items.len() {
        let Some(book) = &items[i].preferred_book else {
            continue;
        };
        let (_, file, name, _, _) = &rows[i];
        let other = (0..items.len())
            .filter(|&j| j != i && items[j].preferred_book.as_ref() == Some(book))
            .filter(|&j| {
                let (_, other_file, other_name, _, _) = &rows[j];
                name.format == other_name.format
                    && (other_name.covers(name)
                        || (other_file.oid == file.oid
                            && !name.covers(other_name)
                            && (other_name.copy_rank(), j) < (name.copy_rank(), i)))
            })
            .reduce(|a, b| {
                let first = &rows[a].2;
                let second = &rows[b].2;
                if second.covers(first)
                    || (!first.covers(second) && (second.copy_rank(), b) < (first.copy_rank(), a))
                {
                    b
                } else {
                    a
                }
            });
        items[i].superseded_by = other.map(|j| items[j].path.clone());
    }
    (items, groups)
}

fn ordinary_preview(state: &State, inputs: &Files) -> Vec<ReadItem> {
    inputs
        .iter()
        .map(|(path, file)| ReadItem {
            path: path.clone(),
            novel: None,
            books: Vec::new(),
            preferred_book: None,
            superseded_by: None,
            same_paths: state
                .catalog
                .branches
                .iter()
                .filter_map(|(branch, info)| {
                    let existing = info.entries.get(path)?;
                    let relation = if existing.file == *file {
                        Relationship::Same
                    } else {
                        Relationship::Review
                    };
                    Some(ReadLocation {
                        path: path.clone(),
                        branch: branch.clone(),
                        relation,
                    })
                })
                .collect(),
        })
        .collect()
}

struct ReadInput<'a> {
    source: &'a Path,
    local: bool,
    files: &'a Files,
    items: &'a [ReadItem],
    groups: &'a BTreeMap<String, Group>,
}

fn register_group(catalog: &mut Catalog, group: &Group) -> Result<u64> {
    let id = group.id.unwrap_or_else(|| catalog.next_book_id());
    for (&existing, book) in &catalog.books {
        ensure!(
            existing == id || book.file_ids.is_disjoint(&group.members),
            "同一个文件已归入另一书目"
        );
    }
    catalog
        .books
        .entry(id)
        .or_insert_with(|| Book {
            title: group.title.clone(),
            author: group.author.clone(),
            file_ids: BTreeSet::new(),
        })
        .file_ids
        .extend(&group.members);
    Ok(id)
}

fn apply_read(
    repo: &Repository<'_>,
    lease: &WriteLease,
    mut state: State,
    read: &ReadInput<'_>,
    selections: &[ReadSelection],
) -> Result<LibraryView> {
    ensure!(!selections.is_empty(), "请勾选要应用的文件");
    let before = serde_json::to_vec(&state.catalog)?;
    let by_path: BTreeMap<_, _> = read
        .items
        .iter()
        .map(|item| (item.path.as_str(), item))
        .collect();
    let mut selected_paths = BTreeSet::new();
    let mut groups = BTreeMap::new();
    let mut removals = BTreeSet::<(&str, &str)>::new();
    let mut destinations = BTreeSet::new();
    let mut chosen = Vec::new();
    let mut updated = BTreeMap::new();

    for selection in selections {
        let path = selection.path.as_str();
        let branch = selection.branch.as_str();
        ensure!(selected_paths.insert(path), "重复选择来源文件 {path}");
        ensure!(
            !read.local || branch == state.head,
            "当前目录的去重只能修改当前分支"
        );
        let item = by_path.get(path).context("来源文件不在该次预览中")?;
        let original_branch = state.catalog.branch(branch)?;
        updated
            .entry(branch.to_owned())
            .or_insert_with(|| original_branch.clone());
        let (book, format, allowed) = if let Some(novel) = &item.novel {
            let key = selection.book.as_ref().context("请确认文件所属的书目")?;
            let choice = item
                .books
                .iter()
                .find(|choice| &choice.key == key)
                .context("书目不在该次预览中")?;
            let id = match groups.get(key) {
                Some(&id) => id,
                None => {
                    let id = register_group(&mut state.catalog, &read.groups[key])?;
                    groups.insert(key.clone(), id);
                    id
                }
            };
            (Some(id), Some(novel.format), &choice.locations)
        } else {
            ensure!(
                selection.book.is_none() && selection.action == ReadAction::Keep,
                "普通文件按照相对路径更新"
            );
            (None, None, &item.same_paths)
        };

        let destination = if read.local {
            path
        } else {
            &selection.destination
        };
        if selection.action == ReadAction::History {
            ensure!(selection.replaces.is_empty(), "归入旧版时无需替换当前文件");
            if read.local {
                ensure!(removals.insert((branch, path)), "文件重复设置为旧版 {path}");
            }
        } else {
            validate_destination(repo, path, destination)?;
            ensure!(
                destinations.insert((branch, destination)),
                "重复的目标文件 {destination}"
            );
            for replaced in &selection.replaces {
                ensure!(
                    allowed
                        .iter()
                        .any(|location| location.branch == branch && location.path == *replaced)
                        && (book.is_some() || replaced == destination),
                    "替换目标不属于所选书目和格式 {replaced}"
                );
                ensure!(
                    removals.insert((branch, replaced)),
                    "文件重复设置为旧版 {replaced}"
                );
            }
        }
        chosen.push((selection, book, format, destination));
    }

    for &(branch, path) in &removals {
        updated.get_mut(branch).unwrap().entries.remove(path);
    }
    // 先登记旧版，随后再发布当前文件；被替换内容始终有 Git 对象可供历史树引用。
    chosen.sort_by_key(|(selection, _, _, _)| selection.action == ReadAction::Keep);
    let mut needs_current = BTreeSet::new();
    for (selection, book, format, destination) in chosen {
        let branch = selection.branch.as_str();
        let path = selection.path.as_str();
        let file = &read.files[path];
        let target = updated.get_mut(branch).unwrap();
        let base = state.catalog.branch(branch)?;
        let old = base
            .entries
            .get(if read.local { path } else { destination })
            .cloned();
        if read.local {
            ensure!(old.is_some(), "请先保存外部改动 {path}");
        }

        if selection.action == ReadAction::Keep {
            ensure!(
                !read.local || !removals.contains(&(branch, path)),
                "已选择保留的文件也被设置为旧版 {path}"
            );
            let occupied = target.entries.get(destination);
            if let Some(existing) = occupied {
                let same_book =
                    book.is_none_or(|id| state.catalog.books[&id].file_ids.contains(&existing.id));
                ensure!(
                    existing.file == *file && (read.local || same_book),
                    "目标同名文件需要先确认替换或改名 {destination}"
                );
            } else {
                ensure_staging_paths(&removals, branch, destination)?;
                ensure_destination(repo, target, path, destination)?;
            }
        }

        let prior = old.map(|entry| entry.id);
        let reuse = prior.filter(|id| {
            read.local
                || book.is_none()
                || book.is_some_and(|book| state.catalog.books[&book].file_ids.contains(id))
        });
        let saved_name = if selection.action == ReadAction::Keep {
            destination
        } else {
            path
        };
        let entry = if selection.action == ReadAction::History && !read.local {
            let book_id = book.unwrap();
            let already_saved = state.catalog.books[&book_id]
                .file_ids
                .iter()
                .filter_map(|id| state.catalog.versions.get(id))
                .flatten()
                .any(|version| {
                    version.file.oid == file.oid
                        && version.path.as_deref().and_then(NovelFormat::of) == format
                });
            needs_current.insert((branch.to_owned(), book_id, format.unwrap()));
            if already_saved {
                continue;
            }
            state.catalog.file_entry(None, file.clone(), saved_name)
        } else {
            state.catalog.file_entry(reuse, file.clone(), saved_name)
        };
        if let (Some(book), Some(format)) = (book, format) {
            state
                .catalog
                .books
                .get_mut(&book)
                .unwrap()
                .file_ids
                .insert(entry.id);
            if selection.action == ReadAction::History {
                needs_current.insert((branch.to_owned(), book, format));
            }
        }
        if selection.action == ReadAction::Keep {
            target.entries.insert(destination.to_owned(), entry);
        }
    }
    for (branch, book, format) in needs_current {
        ensure!(
            updated[&branch]
                .entries
                .iter()
                .any(|(path, entry)| NovelFormat::of(path) == Some(format)
                    && state.catalog.books[&book].file_ids.contains(&entry.id)),
            "请在「{branch}」保留同书同格式的当前文件"
        );
    }
    if before == serde_json::to_vec(&state.catalog)?
        && updated
            .iter()
            .all(|(branch, new)| new.entries == state.catalog.branches[branch].entries)
    {
        return state.view(repo);
    }
    if !read.local {
        worktree::mark_for_gc(lease)?;
        for path in selected_paths {
            worktree::save_from(repo, read.source, path, &read.files[path])?;
        }
    }
    publish_branches(repo, lease, state, updated, "读取并整理文件")
}

fn ensure_staging_paths(
    removals: &BTreeSet<(&str, &str)>,
    branch: &str,
    destination: &str,
) -> Result<()> {
    let key = path_key(destination);
    for &(_, removed) in removals
        .iter()
        .filter(|(name, removed)| *name == branch && *removed != destination)
    {
        let old = path_key(removed);
        ensure!(
            old != key
                && !old
                    .strip_prefix(&key)
                    .is_some_and(|tail| tail.starts_with('/'))
                && !key
                    .strip_prefix(&old)
                    .is_some_and(|tail| tail.starts_with('/')),
            "新路径与正在移出的原路径重叠 {destination}（{removed}）"
        );
    }
    Ok(())
}
