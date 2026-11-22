use std::io;

use icu_normalizer::DecomposingNormalizer;
use tempfile::TempDir;

use super::*;

#[cfg(test)]
mod tests;

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum MoveSelection {
    Book { id: u64 },
    Files { paths: Vec<String> },
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum MoveConfirmation {
    SaveExternal {
        token: String,
    },
    Move {
        token: String,
        destinations: BTreeMap<String, String>,
    },
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MoveRequest {
    pub source_branch: String,
    pub target_branch: String,
    pub selection: MoveSelection,
    pub confirmation: Option<MoveConfirmation>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MoveItem {
    pub path: String,
    pub conflict: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum MoveStatus {
    SaveExternal {
        token: String,
        changes: Vec<ExternalChange>,
    },
    Preview {
        token: String,
        items: Vec<MoveItem>,
    },
    SavedExternal,
    Moved,
}

#[derive(Debug, Serialize)]
pub struct MoveTransition {
    pub library: LibraryView,
    pub status: MoveStatus,
}

impl LibraryEngine {
    pub fn create_book(
        &self,
        directory: &Path,
        source_branch: &str,
        title: &str,
        paths: &[String],
    ) -> Result<LibraryView> {
        let repo = self.repository(directory)?;
        let lease = open(&repo)?;
        let mut state = State::load(&repo)?;
        ensure!(source_branch == state.head, "原分支已变化，请重新选择");
        let title = title.trim();
        ensure!(
            !title.is_empty() && !paths.is_empty(),
            "请填写书目名并选择文件"
        );
        let branch = state.catalog.branch(source_branch)?;
        let file_ids = paths
            .iter()
            .map(|path| {
                branch
                    .entries
                    .get(path)
                    .map(|entry| entry.id)
                    .with_context(|| format!("文件不在当前分支 {path}"))
            })
            .collect::<Result<BTreeSet<_>>>()?;
        ensure!(
            state
                .catalog
                .books
                .values()
                .all(|book| book.file_ids.is_disjoint(&file_ids)),
            "所选文件已经归入书目"
        );

        let commit = branch.commit.clone();
        state.catalog.books.insert(
            state.catalog.next_book_id(),
            Book {
                title: title.to_owned(),
                author: None,
                file_ids,
            },
        );
        let next = state.catalog.snapshot(&repo, "建立书目")?;
        worktree::apply_operation(
            &repo,
            &lease,
            vec![RefEdit::replace(STATE_REF, &state.oid, &next)],
            WorktreePlan::index(Some(commit.clone()), commit),
            None,
        )?;
        State::load(&repo)?.view(&repo)
    }

    pub fn move_files(&self, directory: &Path, request: &MoveRequest) -> Result<MoveTransition> {
        let repo = self.repository(directory)?;
        let lease = open(&repo)?;
        let mut state = State::load(&repo)?;
        ensure!(
            request.source_branch == state.head,
            "原分支已变化，请重新选择"
        );
        ensure!(request.target_branch != state.head, "请选择另一个分支");
        let source = state.catalog.branch(&state.head)?;
        let target = state.catalog.branch(&request.target_branch)?;
        let selected = selected_paths(&state.catalog, source, &request.selection)?;
        let observed = worktree::scan(&repo)?;
        // 确认票据绑定书库状态、两个分支、选择和工作目录字节。
        let token = serde_json::to_string(&(
            &state.oid,
            &state.head,
            &request.target_branch,
            &request.selection,
            &observed,
        ))?;
        let changes = external_changes(&source.entries, &observed);
        if !changes.is_empty() {
            let status = match &request.confirmation {
                Some(MoveConfirmation::SaveExternal { token: confirmed })
                    if confirmed == &token =>
                {
                    save_external(&repo, &lease, &mut state, observed, &changes)?;
                    MoveStatus::SavedExternal
                }
                _ => MoveStatus::SaveExternal { token, changes },
            };
            return Ok(MoveTransition {
                library: state.view(&repo)?,
                status,
            });
        }

        let items = preview_paths(source, target, selected);
        let destinations = match &request.confirmation {
            Some(MoveConfirmation::Move {
                token: confirmed,
                destinations,
            }) if confirmed == &token => destinations,
            _ => {
                return Ok(MoveTransition {
                    library: state.view(&repo)?,
                    status: MoveStatus::Preview { token, items },
                })
            }
        };

        let paths = resolve_destinations(&repo, source, target, &items, destinations)?;
        let (edits, plan, stage) =
            prepare_move(&repo, &lease, state, &request.target_branch, &paths)?;
        worktree::apply_operation(&repo, &lease, edits, plan, Some(stage))?;
        Ok(MoveTransition {
            library: State::load(&repo)?.view(&repo)?,
            status: MoveStatus::Moved,
        })
    }
}

fn selected_paths<'a>(
    catalog: &Catalog,
    source: &'a Branch,
    selection: &MoveSelection,
) -> Result<BTreeSet<&'a str>> {
    let selected = match selection {
        MoveSelection::Book { id } => {
            let book = catalog.books.get(id).context("找不到此书目")?;
            source
                .entries
                .iter()
                .filter(|(_, entry)| book.file_ids.contains(&entry.id))
                .map(|(path, _)| path.as_str())
                .collect()
        }
        MoveSelection::Files { paths } => paths
            .iter()
            .map(|path| {
                source
                    .entries
                    .get_key_value(path)
                    .map(|(key, _)| key.as_str())
                    .with_context(|| format!("文件不在当前分支 {path}"))
            })
            .collect::<Result<BTreeSet<_>>>()?,
    };
    ensure!(!selected.is_empty(), "请选择当前分支中的书目或文件");
    Ok(selected)
}

// 同一份路径索引识别文件、目录与目录拼写冲突，也用于校验多项改名。
struct PathSlot<'a> {
    spelling: &'a str,
    file_path: &'a str,
    file: Option<&'a BlobFile>,
}

#[derive(Default)]
pub(super) struct TargetPaths<'a>(BTreeMap<String, PathSlot<'a>>);

impl<'a> TargetPaths<'a> {
    pub(super) fn from_branch(branch: &'a Branch) -> Self {
        let mut paths = Self::default();
        for (path, entry) in &branch.entries {
            paths.insert(path, &entry.file);
        }
        paths
    }

    fn insert(&mut self, path: &'a str, file: &'a BlobFile) {
        for prefix in prefixes(path) {
            self.0.entry(path_key(prefix)).or_insert(PathSlot {
                spelling: prefix,
                file_path: path,
                file: (prefix == path).then_some(file),
            });
        }
    }

    pub(super) fn conflict(&self, path: &str, shared: Option<&BlobFile>) -> Option<&'a str> {
        for prefix in prefixes(path) {
            let Some(slot) = self.0.get(&path_key(prefix)) else {
                continue;
            };
            let occupied = if prefix == path {
                shared.is_none() || slot.file != shared
            } else {
                slot.file.is_some()
            };
            if slot.spelling != prefix || occupied {
                return Some(slot.file_path);
            }
        }
        None
    }
}

pub(super) fn path_key(path: &str) -> String {
    // Git 中的不同拼写在桌面文件系统上可能指向同一个路径。
    DecomposingNormalizer::new_nfd()
        .normalize(&path.to_uppercase())
        .into_owned()
}

fn prefixes(path: &str) -> impl Iterator<Item = &str> {
    path.match_indices('/')
        .map(|(index, _)| &path[..index])
        .chain(std::iter::once(path))
}

fn preview_paths(source: &Branch, target: &Branch, selected: BTreeSet<&str>) -> Vec<MoveItem> {
    let mut destinations = TargetPaths::from_branch(target);
    selected
        .into_iter()
        .map(|path| {
            let file = &source.entries[path].file;
            let conflict = destinations.conflict(path, Some(file)).map(str::to_owned);
            if conflict.is_none() {
                destinations.insert(path, file);
            }
            MoveItem {
                path: path.to_owned(),
                conflict,
            }
        })
        .collect()
}

fn resolve_destinations(
    repo: &Repository<'_>,
    source: &Branch,
    target: &Branch,
    items: &[MoveItem],
    renames: &BTreeMap<String, String>,
) -> Result<BTreeMap<String, String>> {
    ensure!(
        renames.len() == items.iter().filter(|item| item.conflict.is_some()).count(),
        "请为每个冲突填写新目标路径"
    );
    let mut destinations = TargetPaths::from_branch(target);
    let mut moves = BTreeMap::new();
    for item in items.iter().filter(|item| item.conflict.is_none()) {
        destinations.insert(&item.path, &source.entries[&item.path].file);
        moves.insert(item.path.clone(), item.path.clone());
    }
    for item in items.iter().filter(|item| item.conflict.is_some()) {
        let path = renames
            .get(&item.path)
            .with_context(|| format!("请为冲突文件填写新目标路径 {}", item.path))?;
        validate_destination(repo, &item.path, path)?;
        if let Some(existing) = destinations.conflict(path, None) {
            anyhow::bail!("新目标路径仍有冲突 {path}（{existing}）");
        }
        destinations.insert(path, &source.entries[&item.path].file);
        moves.insert(item.path.clone(), path.clone());
    }
    Ok(moves)
}

pub(super) fn validate_destination(
    repo: &Repository<'_>,
    original: &str,
    destination: &str,
) -> Result<()> {
    ensure!(
        destination.split('/').all(|part| {
            !part.is_empty()
                && part != "."
                && part != ".."
                && !part.eq_ignore_ascii_case(".git")
                && part != ".DS_Store"
                && !part.contains(['\\', '\0'])
                && (!cfg!(windows) || valid_windows_component(part))
        }),
        "无效目标相对路径 {destination}"
    );
    ensure!(
        suffix(original).eq_ignore_ascii_case(suffix(destination)),
        "新目标路径须保留原文件后缀 {original}"
    );
    let mut path = repo.root().to_owned();
    for part in destination.split('/') {
        path.push(part);
        match fs::symlink_metadata(&path) {
            Ok(info) => {
                ensure!(
                    !worktree::is_link(&info),
                    "目标路径经过链接 {}",
                    path.display()
                );
                if info.is_file() {
                    // 当前分支的普通文件会在切换时更新，目标树的占用从目录记录判断。
                    break;
                }
                ensure!(
                    info.is_dir() && !path.join(".git").try_exists()?,
                    "目标路径被其他书库或文件占用 {}",
                    path.display()
                );
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => break,
            Err(error) => {
                return Err(error).with_context(|| format!("检查目标路径 {}", path.display()))
            }
        }
    }
    Ok(())
}

fn valid_windows_component(name: &str) -> bool {
    if name.ends_with(['.', ' '])
        || name.contains(['<', '>', ':', '"', '|', '?', '*'])
        || name.chars().any(|character| character < ' ')
        || name.eq_ignore_ascii_case("desktop.ini")
        || name.eq_ignore_ascii_case("Thumbs.db")
    {
        return false;
    }
    let stem = name
        .split('.')
        .next()
        .unwrap_or_default()
        .trim_end()
        .to_ascii_uppercase();
    !matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        && !stem
            .strip_prefix("COM")
            .or_else(|| stem.strip_prefix("LPT"))
            .is_some_and(|number| {
                matches!(
                    number,
                    "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
                )
            })
}

fn suffix(path: &str) -> &str {
    let name = path.rsplit('/').next().unwrap_or_default();
    if let Some(suffix) = name.get(name.len().saturating_sub(8)..) {
        if suffix.eq_ignore_ascii_case(".fb2.zip") {
            return suffix;
        }
    }
    Path::new(name)
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
}

fn prepare_move(
    repo: &Repository<'_>,
    lease: &WriteLease,
    mut state: State,
    target: &str,
    paths: &BTreeMap<String, String>,
) -> Result<(Vec<RefEdit>, WorktreePlan, TempDir)> {
    let mut source_branch = state.catalog.branch(&state.head)?.clone();
    let mut target_branch = state.catalog.branch(target)?.clone();
    let old_source = source_branch.commit.clone();
    let old_target = target_branch.commit.clone();
    let original_files = source_branch.files();
    for (from, to) in paths {
        let entry = source_branch
            .entries
            .remove(from)
            .with_context(|| format!("找不到原文件 {from}"))?;
        // 跨分支沿用文件 ID 和 blob，旧版及书目仍引用同一个文件身份。
        target_branch.entries.insert(to.clone(), entry);
    }

    let operation = state.catalog.revision + 1;
    source_branch.commit = snapshot_branch(
        repo,
        &source_branch.entries,
        &format!("跨分支移动 {operation} 移出文件"),
    )?;
    target_branch.commit = snapshot_branch(
        repo,
        &target_branch.entries,
        &format!("跨分支移动 {operation} 加入文件"),
    )?;
    ensure!(
        worktree::snapshot_files(repo, &target_branch.commit)? == target_branch.files(),
        "目标分支有无法写入 Git 的路径"
    );
    let (plan, stage) = WorktreePlan::move_out(
        repo,
        lease,
        (&old_source, &source_branch.commit),
        &original_files,
        &source_branch.files(),
    )?;
    let mut edits = vec![
        RefEdit::replace(&refname(&state.head), &old_source, &source_branch.commit),
        RefEdit::replace(&refname(target), &old_target, &target_branch.commit),
    ];
    state.catalog.branches.insert(state.head, source_branch);
    state
        .catalog
        .branches
        .insert(target.to_owned(), target_branch);
    let next = state
        .catalog
        .snapshot(repo, &format!("跨分支移动 {operation} 更新书库"))?;
    edits.push(RefEdit::replace(STATE_REF, &state.oid, &next));
    Ok((edits, plan, stage))
}
