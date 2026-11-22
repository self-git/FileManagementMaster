use std::sync::LazyLock;

use icu_normalizer::ComposingNormalizer;
use regex::Regex;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "lowercase")]
pub enum NovelFormat {
    Epub,
    Txt,
    Mobi,
    Azw3,
    Pdf,
    Azw,
    Kfx,
    Prc,
    Fb2,
    #[serde(rename = "fb2.zip")]
    Fb2Zip,
    Kepub,
}

impl NovelFormat {
    pub(super) fn of(path: &str) -> Option<Self> {
        let name = path.rsplit('/').next()?.to_ascii_lowercase();
        if name.ends_with(".fb2.zip") {
            return Some(Self::Fb2Zip);
        }
        let (stem, extension) = name.rsplit_once('.')?;
        if stem.is_empty() {
            return None;
        }
        Some(match extension {
            "epub" => Self::Epub,
            "txt" => Self::Txt,
            "mobi" => Self::Mobi,
            "azw3" => Self::Azw3,
            "pdf" => Self::Pdf,
            "azw" => Self::Azw,
            "kfx" => Self::Kfx,
            "prc" => Self::Prc,
            "fb2" => Self::Fb2,
            "kepub" => Self::Kepub,
            _ => return None,
        })
    }

    fn suffix_len(self, leaf: &str) -> usize {
        if self == Self::Fb2Zip {
            ".fb2.zip".len()
        } else {
            leaf.len() - leaf.rfind('.').unwrap()
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Coverage {
    Chapters(u32, u32),
    Volumes(u32, u32),
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct NovelName {
    pub(super) title: String,
    pub(super) author: Option<String>,
    pub(super) format: NovelFormat,
    progress: Option<String>,
    completed: bool,
    revised: bool,
    supplemental: bool,
    copy: Option<u32>,
    #[serde(skip)]
    coverage: Option<Coverage>,
}

static COPY: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s*\((\d+)\)\s*$").unwrap());
static AUTHOR: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"作\s*者\s*[:：]?\s*").unwrap());
static RANGE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:第\s*)?([0-9]+)\s*(?:章\s*)?[-–—~至到]\s*(?:第\s*)?([0-9]+)\s*(?:[章节話话])?")
        .unwrap()
});
static THROUGH: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:更新|连载|連載|累计|累計|收录|收錄)(?:至|到)|截至|更新[:：]").unwrap()
});
static VOLUME: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:第\s*)?([0-9]+|[一二三四五六七八九十两]+)\s*[卷部]|[卷部]\s*([0-9]+|[一二三四五六七八九十两]+)").unwrap()
});
static VOLUME_RANGE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"1\s*[-–—~至到]\s*(?:第\s*)?([0-9]+|[一二三四五六七八九十两]+)\s*卷").unwrap()
});
static CHAPTER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?:第\s*)?([0-9]+)\s*[章节話话]").unwrap());
static NOTE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)未完[结結本]?|已完[结結本]?|连载中|連載中|完[结結本]|全本|全集|排版|修[改订訂正]|重修|加料|无删减|無刪減|合订|合訂|番外|续更|續更|单卷|單卷|补档|補檔").unwrap()
});

impl NovelName {
    pub(super) fn parse(path: &str) -> Option<Self> {
        let format = NovelFormat::of(path)?;
        let leaf = path.rsplit('/').next()?;
        let stem = ComposingNormalizer::new_nfkc()
            .normalize(&leaf[..leaf.len() - format.suffix_len(leaf)]);
        let stem = stem.trim();
        let copy = COPY.captures(stem);
        let copy_number = copy.as_ref().and_then(|capture| number(capture, 1));
        let stem = copy
            .as_ref()
            .and_then(|capture| capture.get(0))
            .map_or(stem, |suffix| &stem[..suffix.start()]);
        let (before, author, after) = split_author(stem);
        let (title, note) = split_title(before);
        let notes = format!("{note} {after}");
        let completed = ["完结", "完結", "完本", "全本", "全集"]
            .iter()
            .any(|marker| notes.contains(marker))
            && !["未完", "连载中", "連載中"]
                .iter()
                .any(|marker| notes.contains(marker));
        let revised = [
            "修改",
            "修订",
            "修訂",
            "修正",
            "自修",
            "重修",
            "加料",
            "增补",
            "合订",
            "合訂",
            "无删减",
            "無刪減",
            "改版",
            "校对",
        ]
        .iter()
        .any(|marker| notes.contains(marker));
        let supplemental = [
            "番外", "续更", "續更", "补档", "補檔", "单卷", "單卷", "补更",
        ]
        .iter()
        .any(|marker| notes.contains(marker));
        let (progress, coverage) = progress(&notes);
        Some(Self {
            title: if title.is_empty() {
                before.trim().to_owned()
            } else {
                title.to_owned()
            },
            author,
            format,
            progress,
            completed,
            revised,
            supplemental,
            copy: copy_number,
            coverage,
        })
    }

    pub(super) fn covers(&self, previous: &Self) -> bool {
        if self.format != previous.format
            || self.revised
            || previous.revised
            || self.supplemental
            || previous.supplemental
            || (previous.completed && !self.completed)
        {
            return false;
        }
        match (self.coverage, previous.coverage) {
            (Some(Coverage::Chapters(a, b)), Some(Coverage::Chapters(c, d))) => {
                a <= c && b >= d && (a < c || b > d)
            }
            (Some(Coverage::Volumes(a, b)), Some(Coverage::Volumes(c, d))) => (a, b) > (c, d),
            _ => false,
        }
    }

    pub(super) fn copy_rank(&self) -> (bool, u32) {
        (self.copy.is_some(), self.copy.unwrap_or(0))
    }
}

pub(super) fn identity(text: &str) -> String {
    ComposingNormalizer::new_nfkc()
        .normalize(text)
        .chars()
        .filter(|c| !c.is_whitespace())
        .flat_map(char::to_lowercase)
        .collect()
}

// 只用文件名明确写出的别名作候选，不根据相似度自动合书。
pub(super) fn possibly_same(a: &str, b: &str) -> bool {
    let a = identity(a);
    let b = identity(b);
    a == b || aliases(&a).any(|alias| alias == b) || aliases(&b).any(|alias| alias == a)
}

fn aliases(value: &str) -> impl Iterator<Item = &str> {
    value
        .split("又名")
        .flat_map(|part| part.split(['(', ')', '[', ']', '/', '、']))
        .filter(|part| !part.is_empty())
}

fn clean(text: &str) -> &str {
    text.trim_matches(|c: char| c.is_whitespace() || "_-+()[]【】".contains(c))
}

fn wrapped_title(stem: &str) -> Option<(&str, usize)> {
    for (open, close) in [('《', '》'), ('【', '】')] {
        if let Some(start) = stem.find(open) {
            let prefix = stem[..start].trim();
            if prefix.is_empty() || (prefix.starts_with('[') && prefix.ends_with(']')) {
                if let Some(end) = stem[start + open.len_utf8()..].find(close) {
                    let start = start + open.len_utf8();
                    return Some((
                        stem[start..start + end].trim(),
                        start + end + close.len_utf8(),
                    ));
                }
            }
        }
    }
    None
}

fn split_title(stem: &str) -> (&str, &str) {
    let stem = stem.trim();
    if let Some((title, end)) = wrapped_title(stem) {
        return (title, &stem[end..]);
    }
    let end = note_start(stem).unwrap_or(stem.len());
    (clean(&stem[..end]), &stem[end..])
}

fn split_author(stem: &str) -> (&str, Option<String>, &str) {
    let title_end = wrapped_title(stem).map_or(0, |(_, end)| end);
    let Some(label) = AUTHOR.find_at(stem, title_end) else {
        return (stem, None, "");
    };
    let rest = &stem[label.end()..];
    let end = note_start(rest).unwrap_or(rest.len());
    let author = clean(&rest[..end]);
    (
        &stem[..label.start()],
        (!author.is_empty()).then(|| author.to_owned()),
        &rest[end..],
    )
}

fn note_start(text: &str) -> Option<usize> {
    let range = chapter_ranges(text)
        .next()
        .and_then(|cap| cap.get(0))
        .map(|m| m.start());
    let volume = VOLUME_RANGE
        .find(text)
        .or_else(|| VOLUME.find(text))
        .map(|m| m.start());
    let through = THROUGH.find(text).map(|m| m.start());
    let note = NOTE.find(text).map(|m| m.start());
    let standalone_chapter = CHAPTER
        .find(text)
        .filter(|m| {
            text[..m.start()]
                .chars()
                .next_back()
                .is_some_and(|c| c.is_whitespace() || "([{【_".contains(c))
        })
        .map(|m| m.start());
    [range, volume, through, note, standalone_chapter]
        .into_iter()
        .flatten()
        .min()
}

fn chapter_ranges(text: &str) -> impl Iterator<Item = regex::Captures<'_>> {
    RANGE.captures_iter(text).filter(|cap| {
        let part = cap.get(0).unwrap();
        let before = text[..part.start()].chars().next_back();
        let after = text[part.end()..].chars().next();
        before != Some('.') && after != Some('.') && after != Some('卷')
    })
}

fn progress(note: &str) -> (Option<String>, Option<Coverage>) {
    let range = chapter_ranges(note).collect::<Vec<_>>();
    let volume_range = VOLUME_RANGE.captures(note);
    let cumulative_range = volume_range.is_some();
    if let Some(volume) = volume_range.or_else(|| VOLUME.captures(note)) {
        let location = volume.get(0).unwrap();
        let prefix = note[..location.start()].trim_end();
        let volume = volume
            .get(1)
            .or_else(|| volume.get(2))
            .and_then(|m| volume_number(m.as_str()));
        let chapter = CHAPTER
            .captures(&note[location.end()..])
            .and_then(|cap| number(&cap, 1));
        let label = volume.map(|vol| {
            chapter.map_or_else(
                || format!("第 {vol} 卷"),
                |ch| format!("第 {vol} 卷第 {ch} 章"),
            )
        });
        let cumulative = cumulative_range
            || THROUGH.is_match(prefix)
            || ["1-", "1~", "1—", "1至"]
                .iter()
                .any(|start| prefix.ends_with(start));
        return (
            label,
            if cumulative {
                volume.map(|vol| Coverage::Volumes(vol, chapter.unwrap_or(0)))
            } else {
                None
            },
        );
    }
    if range.len() == 1 {
        if let (Some(start), Some(end)) = (number(&range[0], 1), number(&range[0], 2)) {
            if start > 0 && start <= end {
                return (
                    Some(format!("第 {start}–{end} 章")),
                    Some(Coverage::Chapters(start, end)),
                );
            }
        }
    } else if range.len() > 1 {
        return (Some("多段章节".to_owned()), None);
    }
    if let Some(chapter) = CHAPTER.captures(note).and_then(|cap| number(&cap, 1)) {
        return (
            Some(format!("第 {chapter} 章")),
            THROUGH
                .is_match(note)
                .then_some(Coverage::Chapters(1, chapter)),
        );
    }
    (None, None)
}

fn number(capture: &regex::Captures<'_>, index: usize) -> Option<u32> {
    capture.get(index)?.as_str().parse().ok()
}

fn volume_number(text: &str) -> Option<u32> {
    let digit = |c| {
        "〇一二三四五六七八九"
            .chars()
            .position(|item| item == c)
            .map(|n| n as u32)
    };
    if let Ok(value) = text.parse() {
        return Some(value);
    }
    if text == "两" {
        return Some(2);
    }
    if let Some((tens, ones)) = text.split_once('十') {
        return Some(
            tens.chars().next().map_or(Some(1), digit)? * 10
                + ones.chars().next().map_or(Some(0), digit)?,
        );
    }
    text.chars().next().and_then(digit)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_wrapped_names_chapters_authors_and_copies() {
        let name =
            NovelName::parse("0911/2026/《海图2》 １-１１１未完结 作者：舟（1）.TXT").unwrap();
        assert_eq!(name.title, "海图2");
        assert_eq!(name.author.as_deref(), Some("舟"));
        assert_eq!(name.coverage, Some(Coverage::Chapters(1, 111)));
        assert_eq!(name.copy, Some(1));
        assert!(!name.completed);
        let finished = NovelName::parse("《海图2》作者：舟[1-116章][已完结].txt").unwrap();
        assert_eq!(finished.author.as_deref(), Some("舟"));
        assert!(finished.completed && finished.covers(&name));
        assert_eq!(
            NovelName::parse("《作者的传说》.epub").unwrap().title,
            "作者的传说"
        );
        assert_eq!(
            NovelFormat::of("0911/海图.Fb2.ZIP"),
            Some(NovelFormat::Fb2Zip)
        );
        assert_eq!(NovelFormat::of("0911/海图.zip"), None);
    }

    #[test]
    fn recognises_all_supported_suffixes_case_insensitively() {
        for (suffix, expected) in [
            ("EPUB", NovelFormat::Epub),
            ("TXT", NovelFormat::Txt),
            ("MOBI", NovelFormat::Mobi),
            ("AZW3", NovelFormat::Azw3),
            ("PDF", NovelFormat::Pdf),
            ("AZW", NovelFormat::Azw),
            ("KFX", NovelFormat::Kfx),
            ("PRC", NovelFormat::Prc),
            ("FB2", NovelFormat::Fb2),
            ("FB2.ZIP", NovelFormat::Fb2Zip),
            ("KEPUB", NovelFormat::Kepub),
        ] {
            assert_eq!(
                NovelFormat::of(&format!("0911/海图.{suffix}")),
                Some(expected)
            );
        }
    }

    #[test]
    fn only_comparable_chapters_and_cumulative_volumes_are_ranked() {
        let parsed = |name| NovelName::parse(name).unwrap();
        let old = parsed("海图2 1-116 作者舟.TXT");
        for uncertain in [
            "海图2 全本 作者舟.txt",
            "海图2 1-130 修改版 作者舟.txt",
            "海图2 1-130 番外 作者舟.txt",
            "海图2 117-130 续更 作者舟.txt",
        ] {
            assert!(!parsed(uncertain).covers(&old), "{uncertain}");
        }
        let first = parsed("海图2 1-2卷 第30章 作者舟.txt");
        let next = parsed("海图2 更新至第3卷 第2章 作者舟.txt");
        assert_eq!(first.title, "海图2");
        assert_eq!(first.coverage, Some(Coverage::Volumes(2, 30)));
        assert!(next.covers(&first));
        assert!(!parsed("海图2 第3卷 第10章 作者舟.txt").covers(&first));
        assert_eq!(parsed("海图2 116章 作者舟.txt").title, "海图2");
    }
}
