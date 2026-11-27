//! Text conflict markers. Ranges are UTF-8 byte offsets in the captured input.
//!
//! Git writes two sides around a `=======` separator, with an optional `|||||||`
//! base between them. Jujutsu's snapshot style writes any number of `+++++++`
//! sides alternating with `-------` bases.

use std::ops::Range;

use anyhow::{bail, ensure, Result};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SectionKind {
    Side,
    Base,
}

/// One version of the conflicted text and the marker line that introduces it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Section {
    pub kind: SectionKind,
    /// Exact marker line, including its original line ending. Git introduces
    /// its first side with the opening marker.
    pub marker: Range<usize>,
    pub content: Range<usize>,
    // Jujutsu uses this line ending to separate the text from the next marker.
    separator_len: usize,
}

impl Section {
    fn resolution_range(&self) -> Range<usize> {
        self.content.start..self.content.end - self.separator_len
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Conflict {
    pub whole: Range<usize>,
    /// The `<<<<<<<` line, including its original line ending.
    pub opening: Range<usize>,
    /// The `>>>>>>>` line, including its original line ending.
    pub closing: Range<usize>,
    /// In file order. At least one is a side.
    pub sections: Vec<Section>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Resolution {
    /// The first side.
    Current,
    /// The last side.
    Incoming,
    Base,
    /// Every side, in file order.
    Both,
    Section(usize),
}

impl Conflict {
    pub fn sides(&self) -> impl DoubleEndedIterator<Item = &Section> {
        self.sections
            .iter()
            .filter(|section| section.kind == SectionKind::Side)
    }

    pub fn base(&self) -> Option<&Section> {
        self.sections
            .iter()
            .find(|section| section.kind == SectionKind::Base)
    }

    /// Git's first side starts at the opening marker; Jujutsu gives every section its own.
    fn separator(&self) -> Option<&Range<usize>> {
        let (last, first) = (self.sections.last()?, self.sections.first()?);
        (self.sections.len() > 1 && first.marker == self.opening).then_some(&last.marker)
    }

    /// The section whose text or marker contains `position`. The closing marker
    /// belongs to the last section; Git's separator belongs to neither side.
    pub fn section_at(&self, position: usize) -> Option<usize> {
        if self.closing.contains(&position) {
            return Some(self.sections.len() - 1);
        }
        if self
            .separator()
            .is_some_and(|marker| marker.contains(&position))
        {
            return None;
        }
        self.sections.iter().position(|section| {
            section.marker.contains(&position) || section.content.contains(&position)
        })
    }

    /// Content ranges that replace the conflict, or `None` if the choice is absent.
    pub fn resolution(&self, choice: Resolution) -> Option<Vec<Range<usize>>> {
        let section = match choice {
            Resolution::Current => self.sides().next(),
            Resolution::Incoming => self.sides().next_back(),
            Resolution::Base => self.base(),
            Resolution::Section(index) => self.sections.get(index),
            Resolution::Both => return Some(self.sides().map(Section::resolution_range).collect()),
        };
        Some(vec![section?.resolution_range()])
    }
}

/// Read an index conflict stage on a blocking worker. Missing stages are meaningful
/// in add/add and modify/delete conflicts, and are represented by empty content.
#[cfg(feature = "git")]
pub fn read_source(path: &std::path::Path, side: &str, trusted: bool) -> Result<(String, Vec<u8>)> {
    use gix::index::entry::Stage;
    let stage = match side {
        "base" => Stage::Base,
        "current" => Stage::Ours,
        "incoming" => Stage::Theirs,
        _ => bail!("Expected base, current, or incoming"),
    };
    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("File has no parent"))?;
    let directory = parent
        .ancestors()
        .find(|path| path.is_dir())
        .ok_or_else(|| anyhow::anyhow!("File has no existing parent directory"))?;
    let repo = crate::repository::open_repo(directory, trusted)?.to_thread_local();
    let root = stdx::path::canonicalize_existing(
        repo.workdir()
            .ok_or_else(|| anyhow::anyhow!("Repository has no working tree"))?,
    );
    // Match physical repository identity on Windows (including verbatim paths)
    // and through ancestor aliases, without following a symlink leaf.
    let path = stdx::path::canonicalize_existing(parent).join(
        path.file_name()
            .ok_or_else(|| anyhow::anyhow!("File has no name"))?,
    );
    let relative = path.strip_prefix(&root)?;
    let bytes = gix::path::to_unix_separators_on_windows(gix::path::into_bstr(relative)?);
    let index = repo.index()?;
    ensure!(
        index
            .entries()
            .iter()
            .any(|entry| entry.stage() != Stage::Unconflicted
                && entry.path(&index) == bytes.as_ref()),
        "File has no unresolved Git conflict"
    );
    let rebasing = repo.git_dir().join("rebase-merge").is_dir()
        || repo.git_dir().join("rebase-apply").is_dir();
    let role = match (side, rebasing) {
        ("current", true) => "current (new base and already replayed commits)",
        ("incoming", true) => "incoming (commit being replayed)",
        ("current", false) => "current",
        ("incoming", false) => "incoming",
        _ => "base (common ancestor)",
    };
    let Some(entry) = index.entry_by_path_and_stage(bytes.as_ref(), stage) else {
        return Ok((format!("{role}, absent"), Vec::new()));
    };
    ensure!(
        matches!(
            entry.mode,
            gix::index::entry::Mode::FILE | gix::index::entry::Mode::FILE_EXECUTABLE
        ),
        "This conflict source is not a regular text file"
    );
    Ok((
        role.into(),
        crate::review::read_blob(&repo, entry.id, gix::objs::tree::EntryKind::Blob, relative)?,
    ))
}

fn is_jujutsu_label(suffix: &str) -> bool {
    suffix
        .strip_prefix(" conflict ")
        .or_else(|| suffix.strip_prefix(" Conflict "))
        .and_then(|label| label.split_once(" of "))
        .is_some_and(|(index, count)| {
            matches!((index.parse::<usize>(), count.parse::<usize>()),
                (Ok(index), Ok(count)) if (1..=count).contains(&index))
        })
}

/// Reject incomplete/nested markers instead of offering destructive resolution of an
/// ambiguous region. Marker widths follow Git's configurable conflict-marker-size.
pub fn parse(text: &str) -> Result<Vec<Conflict>> {
    struct Open<'a> {
        opening: Range<usize>,
        width: usize,
        jujutsu_label: Option<&'a str>,
        separated: bool,
        sections: Vec<Section>,
        section: Option<(SectionKind, Range<usize>)>,
    }
    impl Open<'_> {
        fn start(&mut self, kind: SectionKind, marker: Range<usize>) {
            if let Some((kind, previous)) = self.section.replace((kind, marker.clone())) {
                self.sections.push(Section {
                    kind,
                    content: previous.end..marker.start,
                    marker: previous,
                    separator_len: 0,
                });
            }
        }
    }
    let mut result = Vec::new();
    let mut open: Option<Open<'_>> = None;
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        let content = line.trim_end_matches(['\r', '\n']);
        let marker = content
            .as_bytes()
            .first()
            .copied()
            .filter(|c| matches!(c, b'<' | b'|' | b'=' | b'>' | b'+' | b'-' | b'%'));
        let width = marker.map_or(0, |c| content.bytes().take_while(|b| *b == c).count());
        let suffix = &content[width..];
        let marker = marker.filter(|marker| {
            width >= 7
                && (suffix.is_empty() || suffix.starts_with(' '))
                && (*marker != b'=' || suffix.is_empty())
        });
        let range = offset..offset + line.len();
        offset = range.end;
        let Some(marker) = marker else {
            continue;
        };
        let Some(region) = open.as_mut() else {
            match marker {
                b'<' => {
                    let jujutsu_label = is_jujutsu_label(suffix).then_some(suffix);
                    open = Some(Open {
                        opening: range.clone(),
                        width,
                        jujutsu_label,
                        separated: false,
                        sections: Vec::new(),
                        section: jujutsu_label
                            .is_none()
                            .then_some((SectionKind::Side, range)),
                    })
                }
                b'|' | b'=' | b'>' => {
                    bail!("Unmatched conflict marker; resolve this region manually")
                }
                _ => {}
            }
            continue;
        };
        let jujutsu = region.jujutsu_label.is_some();
        // Jujutsu uses wider markers when the content contains marker-like text.
        if jujutsu && width < region.width {
            continue;
        }
        if matches!(marker, b'+' | b'-' | b'%') {
            if !jujutsu || width != region.width {
                continue;
            }
            ensure!(
                region.section.is_some() || range.start == region.opening.end,
                "Expected a Jujutsu section after the opening marker"
            );
            ensure!(
                marker != b'%',
                "Jujutsu diff-style conflict; resolve it manually or set \
                 ui.conflict-marker-style to \"snapshot\""
            );
            let kind = if marker == b'+' {
                SectionKind::Side
            } else {
                SectionKind::Base
            };
            region.start(kind, range);
            continue;
        }
        ensure!(
            marker != b'<',
            "Nested conflict markers; resolve this region manually"
        );
        // Jujutsu separates sections with its own markers only.
        if jujutsu && matches!(marker, b'|' | b'=') {
            continue;
        }
        ensure!(width == region.width, "Mismatched conflict marker widths");
        match marker {
            b'|' => {
                ensure!(
                    !region.separated && region.sections.is_empty(),
                    "Invalid base conflict marker"
                );
                region.start(SectionKind::Base, range);
            }
            b'=' => {
                ensure!(!region.separated, "Duplicate conflict separator");
                region.separated = true;
                region.start(SectionKind::Side, range);
            }
            _ => {
                let mut region = open.take().unwrap();
                ensure!(jujutsu || region.separated, "Conflict has no separator");
                // The closing marker ends the last section without starting another.
                region.start(SectionKind::Side, range.clone());
                if let Some(label) = region.jujutsu_label {
                    ensure!(
                        suffix.strip_suffix(" ends") == Some(label),
                        "Mismatched Jujutsu conflict labels"
                    );
                    // Without an ending on the closing marker, Jujutsu adds a
                    // separator to each section. The opening marker records its style.
                    if !line.ends_with('\n') {
                        let crlf = text[region.opening.clone()].ends_with("\r\n");
                        for section in &mut region.sections {
                            if let Some(content) = text[section.content.clone()].strip_suffix('\n')
                            {
                                section.separator_len =
                                    1 + usize::from(crlf && content.ends_with('\r'));
                            }
                        }
                    }
                }
                ensure!(
                    region
                        .sections
                        .iter()
                        .any(|section| section.kind == SectionKind::Side),
                    "Conflict has no side; resolve this region manually"
                );
                result.push(Conflict {
                    whole: region.opening.start..range.end,
                    opening: region.opening,
                    closing: range,
                    sections: region.sections,
                });
            }
        }
    }
    ensure!(
        open.is_none(),
        "Incomplete conflict markers; resolve this region manually"
    );
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "git")]
    #[test]
    fn reads_all_index_stages_and_labels_rebase_roles() -> Result<()> {
        use crate::git::test::{create_commit_with_message, empty_git_repo, exec_git_cmd};
        let dir = empty_git_repo();
        let root = dir.path().canonicalize()?;
        let path = root.join("file.txt");
        std::fs::write(root.join(".gitattributes"), "* text eol=lf\n")?;
        std::fs::write(&path, "base\n")?;
        create_commit_with_message(&root, true, "initial");
        exec_git_cmd(&["checkout", "-b", "topic"], &root);
        std::fs::write(&path, "incoming\n")?;
        create_commit_with_message(&root, true, "topic");
        exec_git_cmd(&["checkout", "main"], &root);
        std::fs::write(&path, "current\n")?;
        create_commit_with_message(&root, true, "current");
        let status = std::process::Command::new("git")
            .arg("-C")
            .arg(&root)
            .args(["merge", "topic"])
            .env_remove("GIT_DIR")
            .output()?;
        assert!(!status.status.success());
        assert_eq!(read_source(&path, "base", false)?.1, b"base\n");
        assert_eq!(read_source(&path, "current", false)?.1, b"current\n");
        assert_eq!(read_source(&path, "incoming", false)?.1, b"incoming\n");
        std::fs::create_dir(root.join(".git/rebase-merge"))?;
        assert!(read_source(&path, "current", false)?.0.contains("new base"));
        assert!(read_source(&path, "incoming", false)?
            .0
            .contains("being replayed"));
        Ok(())
    }

    #[cfg(feature = "git")]
    #[test]
    fn missing_stages_remain_inspectable_after_removing_the_parent_directory() -> Result<()> {
        use crate::git::test::{create_commit_with_message, empty_git_repo, exec_git_cmd};
        let dir = empty_git_repo();
        let root = dir.path().canonicalize()?;
        std::fs::write(root.join(".gitattributes"), "* text eol=lf\n")?;
        let parent = root.join("nested");
        let path = parent.join("file.txt");
        std::fs::create_dir(&parent)?;
        std::fs::write(&path, "base\n")?;
        create_commit_with_message(&root, true, "base");
        exec_git_cmd(&["checkout", "-b", "topic"], &root);
        std::fs::write(&path, "incoming\n")?;
        create_commit_with_message(&root, true, "incoming");
        exec_git_cmd(&["checkout", "main"], &root);
        exec_git_cmd(&["rm", "nested/file.txt"], &root);
        create_commit_with_message(&root, false, "delete current");
        let merge = std::process::Command::new("git")
            .arg("-C")
            .arg(&root)
            .args(["merge", "topic"])
            .env_remove("GIT_DIR")
            .output()?;
        assert_eq!(merge.status.code(), Some(1));
        // Choose deletion in the worktree, keeping unresolved stages for inspection.
        std::fs::remove_file(&path)?;
        std::fs::remove_dir(&parent)?;
        assert_eq!(read_source(&path, "base", false)?.1, b"base\n");
        let (label, current) = read_source(&path, "current", false)?;
        assert!(label.contains("absent"));
        assert!(current.is_empty());
        assert_eq!(read_source(&path, "incoming", false)?.1, b"incoming\n");
        Ok(())
    }

    fn sections<'a>(text: &'a str, conflict: &Conflict) -> Vec<(SectionKind, &'a str, &'a str)> {
        conflict
            .sections
            .iter()
            .map(|s| (s.kind, &text[s.marker.clone()], &text[s.content.clone()]))
            .collect()
    }

    #[test]
    fn parses_diff3_unicode_crlf_and_custom_width_without_changing_text() -> Result<()> {
        let text = "prefix\r\n<<<<<<<< HEAD\r\nžluť\r\n|||||||| base\r\nold\r\n========\r\nblue\r\n>>>>>>>> feature";
        let conflicts = parse(text)?;
        let c = &conflicts[0];
        assert_eq!(&text[c.whole.clone()], &text[8..]);
        assert_eq!(&text[c.opening.clone()], "<<<<<<<< HEAD\r\n");
        assert_eq!(&text[c.closing.clone()], ">>>>>>>> feature");
        assert_eq!(
            sections(text, c),
            [
                (SectionKind::Side, "<<<<<<<< HEAD\r\n", "žluť\r\n"),
                (SectionKind::Base, "|||||||| base\r\n", "old\r\n"),
                (SectionKind::Side, "========\r\n", "blue\r\n"),
            ]
        );
        Ok(())
    }

    #[test]
    fn rejects_malformed_regions_and_accepts_empty_sides() -> Result<()> {
        for text in [
            "<<<<<<< a\na\n",
            "=======\n",
            "<<<<<<< a\n<<<<<<< b\n",
            "<<<<<<< a\n=======\n>>>>>>>> b\n",
            "<<<<<<< a\n=======\n=======\n>>>>>>> b\n",
            "<<<<<<< a\n=======\n||||||| base\n>>>>>>> b\n",
            "<<<<<<< a\nkept\n>>>>>>> b\n",
            "<<<<<<< a\nours\n||||||| base\nold\n>>>>>>> b\n",
        ] {
            assert!(parse(text).is_err(), "{text}");
        }
        let text = "<<<<<<< a\n=======\n>>>>>>> b\n";
        let conflicts = parse(text)?;
        assert!(conflicts[0].sections.iter().all(|s| s.content.is_empty()));
        assert!(parse("<<<<<<<<comparison\n=======not a marker\n")?.is_empty());
        Ok(())
    }

    #[test]
    fn parses_jujutsu_snapshots_with_any_number_of_sides() -> Result<()> {
        let text = "\
<<<<<<< conflict 1 of 1
+++++++ side #1
one
------- base #1
=======
+++++++ side #2
------- base #2
two
+++++++ side #3
three
>>>>>>> conflict 1 of 1 ends
";
        let conflict = &parse(text)?[0];
        assert_eq!(&text[conflict.opening.clone()], "<<<<<<< conflict 1 of 1\n");
        assert_eq!(
            sections(text, conflict),
            [
                (SectionKind::Side, "+++++++ side #1\n", "one\n"),
                // Git's markers are ordinary text between Jujutsu's.
                (SectionKind::Base, "------- base #1\n", "=======\n"),
                (SectionKind::Side, "+++++++ side #2\n", ""),
                (SectionKind::Base, "------- base #2\n", "two\n"),
                (SectionKind::Side, "+++++++ side #3\n", "three\n"),
            ]
        );
        let contents = |choice| {
            conflict.resolution(choice).map(|ranges| {
                ranges
                    .into_iter()
                    .map(|range| &text[range])
                    .collect::<String>()
            })
        };
        assert_eq!(contents(Resolution::Current).unwrap(), "one\n");
        assert_eq!(contents(Resolution::Incoming).unwrap(), "three\n");
        assert_eq!(contents(Resolution::Base).unwrap(), "=======\n");
        assert_eq!(contents(Resolution::Both).unwrap(), "one\nthree\n");
        assert_eq!(contents(Resolution::Section(3)).unwrap(), "two\n");
        assert_eq!(contents(Resolution::Section(5)), None);
        Ok(())
    }

    #[test]
    fn jujutsu_markers_are_text_in_git_conflicts_and_diff_style_is_rejected() -> Result<()> {
        let text = "<<<<<<< HEAD\nTitle\n-------\n+++++++ added\n=======\ntheirs\n>>>>>>> topic\n";
        let conflict = &parse(text)?[0];
        assert_eq!(
            sections(text, conflict),
            [
                (
                    SectionKind::Side,
                    "<<<<<<< HEAD\n",
                    "Title\n-------\n+++++++ added\n"
                ),
                (SectionKind::Side, "=======\n", "theirs\n"),
            ]
        );
        assert_eq!(conflict.resolution(Resolution::Base), None);
        // A longer or shorter run is not one of this conflict's markers.
        let text = "<<<<<<< conflict 1 of 1\n+++++++ one\n--------\n>>>>>>> conflict 1 of 1 ends\n";
        assert_eq!(parse(text)?[0].sections.len(), 1);
        let conflict = &parse(text)?[0];
        assert_eq!(&text[conflict.sections[0].content.clone()], "--------\n");
        for text in [
            "<<<<<<< conflict 1 of 1\n%%%%%%% diff\n-a\n+b\n+++++++ side\nc\n>>>>>>> conflict 1 of 1 ends\n",
            "<<<<<<< conflict 1 of 1\n+++++++ side\nc\n%%%%%%% diff\n-a\n+b\n>>>>>>> conflict 1 of 1 ends\n",
        ] {
            assert!(parse(text)
                .unwrap_err()
                .to_string()
                .contains("Jujutsu diff-style conflict"));
        }
        assert!(parse(
            "<<<<<<< conflict 1 of 1\n------- base\nold\n>>>>>>> conflict 1 of 1 ends\n"
        )
        .is_err());
        Ok(())
    }

    #[test]
    fn git_conflict_can_start_with_literal_jujutsu_section_markers() -> Result<()> {
        for literal in ["+++++++ literal", "-------", "%%%%%%% literal"] {
            let ours = format!("{literal}\nours\n");
            let text =
                format!("<<<<<<< HEAD\n{ours}||||||| base\nold\n=======\ntheirs\n>>>>>>> topic\n");
            assert_eq!(
                sections(&text, &parse(&text)?[0]),
                [
                    (SectionKind::Side, "<<<<<<< HEAD\n", ours.as_str()),
                    (SectionKind::Base, "||||||| base\n", "old\n"),
                    (SectionKind::Side, "=======\n", "theirs\n"),
                ]
            );
        }
        Ok(())
    }

    #[test]
    fn jujutsu_resolution_preserves_trailing_line_endings() -> Result<()> {
        for eol in ["\n", "\r\n"] {
            // The CR at the end of the current side is part of the original text.
            let text = format!(
                "<<<<<<< Conflict 1 of 1{eol}+++++++ side #1{eol}ours\r{eol}------- base{eol}{eol}+++++++ side #2{eol}theirs{eol}{eol}>>>>>>> Conflict 1 of 1 ends"
            );
            let conflict = &parse(&text)?[0];
            let incoming = format!("theirs{eol}");
            let both = format!("ours\r{incoming}");
            for (choice, expected) in [
                (Resolution::Current, "ours\r"),
                (Resolution::Base, ""),
                (Resolution::Incoming, incoming.as_str()),
                (Resolution::Both, both.as_str()),
                (Resolution::Section(0), "ours\r"),
            ] {
                let replacement: String = conflict
                    .resolution(choice)
                    .unwrap()
                    .into_iter()
                    .map(|range| &text[range])
                    .collect();
                assert_eq!(replacement, expected, "{choice:?} with {eol:?}");
            }
        }
        Ok(())
    }

    #[test]
    fn long_jujutsu_markers_allow_shorter_literal_markers_in_sections() -> Result<()> {
        let text = concat!(
            "<<<<<<<<<<< conflict 1 of 1\n",
            "+++++++++++ side #1\n",
            "<<<<<<< literal\n",
            "----------- base\n",
            "old\n",
            "+++++++++++ side #2\n",
            ">>>>>>> literal\n",
            ">>>>>>>>>>> conflict 1 of 1 ends\n",
        );
        assert_eq!(
            sections(text, &parse(text)?[0]),
            [
                (
                    SectionKind::Side,
                    "+++++++++++ side #1\n",
                    "<<<<<<< literal\n"
                ),
                (SectionKind::Base, "----------- base\n", "old\n"),
                (
                    SectionKind::Side,
                    "+++++++++++ side #2\n",
                    ">>>>>>> literal\n"
                ),
            ]
        );
        Ok(())
    }

    #[test]
    fn positions_map_to_sections_except_on_the_git_separator() -> Result<()> {
        let text = "<<<<<<< HEAD\nours\n||||||| base\nold\n=======\ntheirs\n>>>>>>> topic\nafter\n";
        let conflict = &parse(text)?[0];
        let at = |needle: &str| conflict.section_at(text.find(needle).unwrap());
        assert_eq!(at("<<<<<<<"), Some(0));
        assert_eq!(at("ours"), Some(0));
        assert_eq!(at("|||||||"), Some(1));
        assert_eq!(at("old"), Some(1));
        assert_eq!(at("======="), None);
        assert_eq!(at("theirs"), Some(2));
        assert_eq!(at(">>>>>>>"), Some(2));
        assert_eq!(at("after"), None);

        let text = "<<<<<<< conflict 1 of 1\n+++++++ one\na\n------- base\nb\n+++++++ two\nc\n>>>>>>> conflict 1 of 1 ends\n";
        let conflict = &parse(text)?[0];
        let at = |needle: &str| conflict.section_at(text.find(needle).unwrap());
        assert_eq!(at("<<<<<<<"), None);
        assert_eq!(at("+++++++ one"), Some(0));
        assert_eq!(at("------- base"), Some(1));
        assert_eq!(at("+++++++ two"), Some(2));
        assert_eq!(at(">>>>>>>"), Some(2));
        Ok(())
    }
}
