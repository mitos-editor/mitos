//! Text conflict markers. Ranges are UTF-8 byte offsets in the captured input.

use std::ops::Range;

use anyhow::{bail, ensure, Result};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Conflict {
    pub whole: Range<usize>,
    pub current: Range<usize>,
    pub base: Option<Range<usize>>,
    pub incoming: Range<usize>,
    pub current_label: String,
    pub incoming_label: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Resolution {
    Current,
    Incoming,
    Both,
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
    let repo = crate::git::open_repo(parent, trusted)?.to_thread_local();
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
    let bytes = gix::path::to_unix_separators_on_windows(gix::path::try_into_bstr(relative)?);
    let index = repo.index()?;
    ensure!(
        index
            .entries()
            .iter()
            .any(|entry| entry.stage() != Stage::Unconflicted
                && entry.path(&index) == bytes.as_ref()),
        "File has no unresolved index stages"
    );
    let rebasing = repo.git_dir().join("rebase-merge").is_dir()
        || repo.git_dir().join("rebase-apply").is_dir();
    let role = match (side, rebasing) {
        ("current", true) => "current (new base and already replayed commits)",
        ("incoming", true) => "incoming (commit being replayed)",
        ("current", false) => "current (index stage 2)",
        ("incoming", false) => "incoming (index stage 3)",
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
    let content =
        crate::review::read_blob(&repo, entry.id, gix::objs::tree::EntryKind::Blob, relative)?;
    ensure!(
        !content.contains(&0),
        "Binary conflict sources cannot be shown as text"
    );
    Ok((role.into(), content))
}

impl Conflict {
    pub fn replacement(&self, text: &str, choice: Resolution) -> String {
        match choice {
            Resolution::Current => text[self.current.clone()].into(),
            Resolution::Incoming => text[self.incoming.clone()].into(),
            Resolution::Both => format!(
                "{}{}",
                &text[self.current.clone()],
                &text[self.incoming.clone()]
            ),
        }
    }
}

/// Reject incomplete/nested markers instead of offering destructive resolution of an
/// ambiguous region. Marker widths follow Git's configurable conflict-marker-size.
pub fn parse(text: &str) -> Result<Vec<Conflict>> {
    struct Open {
        start: usize,
        current: Range<usize>,
        base: Option<Range<usize>>,
        incoming: Option<usize>,
        width: usize,
        label: String,
    }
    let mut result = Vec::new();
    let mut open: Option<Open> = None;
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        let content = line.trim_end_matches(['\r', '\n']);
        let marker = content
            .as_bytes()
            .first()
            .copied()
            .filter(|c| matches!(c, b'<' | b'|' | b'=' | b'>'));
        let width = marker.map_or(0, |c| content.bytes().take_while(|b| *b == c).count());
        let suffix = &content[width..];
        let is_marker = width >= 7
            && (suffix.is_empty() || suffix.starts_with(' '))
            && (marker != Some(b'=') || suffix.is_empty());
        if is_marker {
            match marker.unwrap() {
                b'<' => {
                    ensure!(
                        open.is_none(),
                        "Nested conflict markers; resolve this region manually"
                    );
                    open = Some(Open {
                        start: offset,
                        current: offset + line.len()..0,
                        base: None,
                        incoming: None,
                        width,
                        label: suffix.trim().into(),
                    });
                }
                b'|' | b'=' | b'>' => {
                    let Some(region) = open.as_mut() else {
                        bail!("Unmatched conflict marker; resolve this region manually")
                    };
                    ensure!(width == region.width, "Mismatched conflict marker widths");
                    match marker.unwrap() {
                        b'|' => {
                            ensure!(
                                region.base.is_none() && region.incoming.is_none(),
                                "Invalid base conflict marker"
                            );
                            region.current.end = offset;
                            region.base = Some(offset + line.len()..0);
                        }
                        b'=' => {
                            ensure!(region.incoming.is_none(), "Duplicate conflict separator");
                            if let Some(base) = &mut region.base {
                                base.end = offset;
                            } else {
                                region.current.end = offset;
                            }
                            region.incoming = Some(offset + line.len());
                        }
                        b'>' => {
                            let region = open.take().unwrap();
                            let incoming = region
                                .incoming
                                .ok_or_else(|| anyhow::anyhow!("Conflict has no separator"))?;
                            result.push(Conflict {
                                whole: region.start..offset + line.len(),
                                current: region.current,
                                base: region.base,
                                incoming: incoming..offset,
                                current_label: region.label,
                                incoming_label: suffix.trim().into(),
                            });
                        }
                        _ => unreachable!(),
                    }
                }
                _ => unreachable!(),
            }
        }
        offset += line.len();
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

    #[test]
    fn parses_diff3_unicode_crlf_and_custom_width_without_changing_text() -> Result<()> {
        let text = "prefix\r\n<<<<<<<< HEAD\r\nžluť\r\n|||||||| base\r\nold\r\n========\r\nblue\r\n>>>>>>>> feature";
        let conflicts = parse(text)?;
        let c = &conflicts[0];
        assert_eq!(&text[c.whole.clone()], &text[8..]);
        assert_eq!(&text[c.base.clone().unwrap()], "old\r\n");
        assert_eq!(c.replacement(text, Resolution::Both), "žluť\r\nblue\r\n");
        assert_eq!(c.current_label, "HEAD");
        assert_eq!(c.incoming_label, "feature");
        Ok(())
    }

    #[test]
    fn rejects_malformed_regions_and_accepts_empty_sides() -> Result<()> {
        for text in [
            "<<<<<<< a\na\n",
            "=======\n",
            "<<<<<<< a\n<<<<<<< b\n",
            "<<<<<<< a\n=======\n>>>>>>>> b\n",
        ] {
            assert!(parse(text).is_err(), "{text}");
        }
        let text = "<<<<<<< a\n=======\n>>>>>>> b\n";
        assert_eq!(parse(text)?[0].replacement(text, Resolution::Both), "");
        assert!(parse("<<<<<<<<comparison\n=======not a marker\n")?.is_empty());
        Ok(())
    }
}
