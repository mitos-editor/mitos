//! Partial index edits are built from source snapshots, never from displayed review rows.

use std::{io::Read, ops::Range, path::Path};

use anyhow::{ensure, Context, Result};
use gix::bstr::ByteSlice;
use ropey::Rope;

use super::{FileOperation, GitRepository, Operation, PreparedOperation};
use crate::MAX_REVIEW_BYTES;

/// Zero-based, end-exclusive source lines. Deletions use their following line as an anchor.
#[derive(Clone, Debug)]
pub enum PartialSelection {
    Hunks(Vec<Range<u32>>),
    Lines(Vec<Range<u32>>),
    /// Independent removed/added rows selected in a live diff, in transformation coordinates.
    ReviewLines {
        before: Vec<Range<u32>>,
        after: Vec<Range<u32>>,
    },
}

impl PartialSelection {
    fn ranges(&self) -> &[Range<u32>] {
        match self {
            Self::Hunks(ranges) | Self::Lines(ranges) => ranges,
            Self::ReviewLines { before, after } => {
                if after.is_empty() {
                    before
                } else {
                    after
                }
            }
        }
    }
    fn contains(&self, line: u32) -> bool {
        self.ranges().iter().any(|range| range.contains(&line))
    }
}

/// Staging a buffer explicitly includes its unsaved UTF-8 text without saving the file.
/// Index coordinates are used by staged review buffers when unstaging.
#[derive(Clone, Debug)]
pub enum PartialSource {
    Buffer(String),
    Worktree,
    Index,
    IndexSnapshot(String),
    /// Exact live-review snapshots. A changed index invalidates the action.
    Snapshot {
        index: String,
        text: String,
        buffer: bool,
    },
}

impl GitRepository {
    pub fn prepare_partial(
        &self,
        path: &Path,
        kind: FileOperation,
        source: &PartialSource,
        mut selection: PartialSelection,
        trusted: bool,
    ) -> Result<PreparedOperation> {
        ensure!(
            trusted,
            "Git actions require workspace trust; run :workspace-trust first"
        );
        let repo = self.open(true)?;
        let relative = self.relative(path)?;
        let path_bytes =
            gix::path::to_unix_separators_on_windows(gix::path::try_into_bstr(&relative)?);
        let index = repo.index_or_empty()?;
        ensure!(
            index
                .entries()
                .iter()
                .filter(|entry| entry.path(&index) == path_bytes.as_bstr())
                .all(|entry| entry.stage() == gix::index::entry::Stage::Unconflicted),
            "Resolve file conflicts before partial staging"
        );
        let entry = index
            .entry_by_path_and_stage(path_bytes.as_bstr(), gix::index::entry::Stage::Unconflicted);
        if entry.is_none() && kind == FileOperation::Stage {
            let mut excludes = repo.excludes(&index, None, Default::default())?;
            ensure!(
                !excludes.at_path(&relative, None)?.is_excluded(),
                "This file is ignored by Git; partial staging does not force-add ignored files"
            );
        }
        let status = std::cell::RefCell::new(None);
        let canonical_path = self.root.join(&relative);
        crate::git::for_each_file_status(
            &crate::ChangedFileScope::Repository(self.root.clone()),
            true,
            |_, result| match result {
                Ok(found) if found.path() == canonical_path => {
                    *status.borrow_mut() = Some(Ok(found));
                    false
                }
                Err(error) => {
                    *status.borrow_mut() = Some(Err(error));
                    false
                }
                _ => true,
            },
        )?;
        let status = status.into_inner().transpose()?;
        ensure!(
            !status.as_ref().is_some_and(|status| matches!(
                status.unstaged,
                Some(crate::FileChange::Renamed { .. })
            )),
            "Stage or unstage this rename as a whole file before selecting text changes"
        );
        let mode = entry.map(|entry| entry.mode);
        ensure!(
            mode.is_none_or(|mode| matches!(
                mode,
                gix::index::entry::Mode::FILE | gix::index::entry::Mode::FILE_EXECUTABLE
            )),
            "Partial staging requires a regular text file"
        );
        ensure!(
            !std::fs::symlink_metadata(path).is_ok_and(|m| !m.is_file()),
            "Partial staging requires a regular text file"
        );
        let mut watched = crate::git::repository_watched_paths(&repo)?;
        watched.push(path.to_owned());
        watched.extend(
            path.ancestors()
                .skip(1)
                .map(|parent| parent.join(".gitattributes")),
        );
        let expected = self.stamp(watched);
        let indexed = match entry {
            Some(entry) => {
                ensure!(
                    repo.find_header(entry.id)?.size() <= MAX_REVIEW_BYTES as u64,
                    "File too large for partial staging"
                );
                let data = repo.find_object(entry.id)?.detach().data;
                ensure!(
                    data.len() <= MAX_REVIEW_BYTES,
                    "File too large for partial staging"
                );
                data
            }
            None => Vec::new(),
        };
        let worktree_index = match entry {
            Some(entry) => crate::review::read_blob(
                &repo,
                entry.id,
                gix::objs::tree::EntryKind::Blob,
                &relative,
            )?,
            None => Vec::new(),
        };
        let index_text = text(&worktree_index)?;
        ensure!(
            line_count(index_text) == line_count(text(&indexed)?),
            "This file's Git filters change line positions; use whole-file staging"
        );
        let input = match source {
            PartialSource::Buffer(text) => text.as_bytes().to_owned(),
            PartialSource::Worktree => read_worktree(path)?,
            PartialSource::Index => worktree_index.clone(),
            PartialSource::IndexSnapshot(expected) => {
                ensure!(
                    expected.trim_start_matches('\u{feff}')
                        == index_text.trim_start_matches('\u{feff}'),
                    "Git index changed since this review; refresh and try again"
                );
                worktree_index.clone()
            }
            PartialSource::Snapshot {
                index,
                text,
                buffer,
            } => {
                ensure!(
                    index.trim_start_matches('\u{feff}')
                        == index_text.trim_start_matches('\u{feff}'),
                    "Git index changed since this review; refresh and try again"
                );
                if !buffer {
                    let current = read_worktree(path)?;
                    ensure!(
                        self::text(&current)?.trim_start_matches('\u{feff}')
                            == text.trim_start_matches('\u{feff}'),
                        "Source file changed since this review; refresh and try again"
                    );
                    current
                } else {
                    text.as_bytes().to_owned()
                }
            }
        };
        let input = text(&input)?;
        match kind {
            FileOperation::Stage => {
                ensure!(
                    !matches!(
                        source,
                        PartialSource::Index | PartialSource::IndexSnapshot(_)
                    ),
                    "Choose a buffer or disk source to stage"
                );
                let (mut pipeline, _) = repo.filter_pipeline(None)?;
                let mut cleaned = Vec::new();
                pipeline
                    .convert_to_git(input.as_bytes(), &relative, &index)?
                    .take(MAX_REVIEW_BYTES as u64 + 1)
                    .read_to_end(&mut cleaned)?;
                let cleaned = text(&cleaned)?;
                ensure!(
                    line_count(input) == line_count(cleaned),
                    "This file's Git filters change line positions; use whole-file staging"
                );
                let mut changed = select_changes(
                    text(&indexed)?.trim_start_matches('\u{feff}'),
                    cleaned.trim_start_matches('\u{feff}'),
                    &selection,
                    Side::After,
                )?;
                let delete_file = matches!(
                    source,
                    PartialSource::Worktree | PartialSource::Snapshot { buffer: false, .. }
                ) && std::fs::symlink_metadata(path)
                    .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound);
                // Text actions preserve an existing index BOM; new files use their source BOM.
                let has_bom = if entry.is_none() {
                    cleaned.starts_with('\u{feff}')
                } else {
                    indexed.starts_with(b"\xef\xbb\xbf")
                };
                if has_bom && !(delete_file && changed.is_empty()) {
                    changed.insert(0, '\u{feff}');
                }
                let remaining = hunks(
                    changed.trim_start_matches('\u{feff}'),
                    cleaned.trim_start_matches('\u{feff}'),
                );
                let cursor = selection.ranges().first().map_or(0, |range| range.start);
                let next_line = remaining
                    .iter()
                    .find(|hunk| hunk.after.start > cursor)
                    .or_else(|| remaining.first())
                    .map(|hunk| hunk.after.start);
                let mut operation = self.partial_result(
                    relative,
                    kind,
                    indexed,
                    changed,
                    delete_file,
                    mode,
                    new_mode(&repo, path),
                    expected,
                )?;
                if let Operation::Partial {
                    next_line: next, ..
                } = &mut operation.operation
                {
                    *next = next_line;
                }
                Ok(operation)
            }
            FileOperation::Unstage => {
                if !matches!(
                    source,
                    PartialSource::Index | PartialSource::IndexSnapshot(_)
                ) {
                    selection = map_to_index(
                        index_text.trim_start_matches('\u{feff}'),
                        input.trim_start_matches('\u{feff}'),
                        selection,
                    );
                }
                let head = if repo.head()?.is_unborn() {
                    None
                } else {
                    let head_path = match status.as_ref().and_then(|status| status.staged.as_ref())
                    {
                        Some(crate::FileChange::Renamed { from_path, .. }) => {
                            self.relative(from_path)?
                        }
                        _ => relative.clone(),
                    };
                    repo.head_commit()?
                        .tree()?
                        .lookup_entry_by_path(&head_path)?
                };
                let remove = head.is_none();
                let restored_mode = if head.as_ref().is_some_and(|entry| {
                    entry.mode().kind() == gix::objs::tree::EntryKind::BlobExecutable
                }) {
                    gix::index::entry::Mode::FILE_EXECUTABLE
                } else {
                    gix::index::entry::Mode::FILE
                };
                let original = match head {
                    Some(entry) => {
                        ensure!(
                            matches!(
                                entry.mode().kind(),
                                gix::objs::tree::EntryKind::Blob
                                    | gix::objs::tree::EntryKind::BlobExecutable
                            ),
                            "Partial unstaging requires a regular text file"
                        );
                        ensure!(
                            repo.find_header(entry.object_id())?.size() <= MAX_REVIEW_BYTES as u64,
                            "File too large for partial staging"
                        );
                        repo.find_object(entry.object_id())?.detach().data
                    }
                    None => Vec::new(),
                };
                let restored = text(&original)?.to_owned();
                let mut changed = select_changes(
                    text(&indexed)?.trim_start_matches('\u{feff}'),
                    restored.trim_start_matches('\u{feff}'),
                    &selection,
                    Side::Before,
                )?;
                let has_bom = if entry.is_none() {
                    restored.starts_with('\u{feff}')
                } else {
                    indexed.starts_with(b"\xef\xbb\xbf")
                };
                if has_bom && !(remove && changed.is_empty()) {
                    changed.insert(0, '\u{feff}');
                }
                self.partial_result(
                    relative,
                    kind,
                    indexed,
                    changed,
                    remove,
                    mode,
                    restored_mode,
                    expected,
                )
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn partial_result(
        &self,
        relative: std::path::PathBuf,
        kind: FileOperation,
        indexed: Vec<u8>,
        changed: String,
        remove: bool,
        mode: Option<gix::index::entry::Mode>,
        new_mode: gix::index::entry::Mode,
        expected: Vec<(std::path::PathBuf, Option<(std::time::SystemTime, u64)>)>,
    ) -> Result<PreparedOperation> {
        let cleaned = changed.into_bytes();
        ensure!(
            !indexed.contains(&0) && !cleaned.contains(&0),
            "Binary files require whole-file staging"
        );
        ensure!(
            indexed != cleaned || (remove && cleaned.is_empty()),
            "No selected text changes"
        );
        let patch = patch(
            &relative,
            &indexed,
            &cleaned,
            mode,
            new_mode,
            remove && cleaned.is_empty(),
        )?;
        Ok(PreparedOperation {
            repository: self.clone(),
            operation: Operation::Partial {
                kind,
                path: relative,
                patch,
                next_line: None,
            },
            expected,
        })
    }
}

fn text(bytes: &[u8]) -> Result<&str> {
    ensure!(
        bytes.len() <= MAX_REVIEW_BYTES,
        "File too large for partial staging"
    );
    ensure!(!bytes.contains(&0), "Partial staging requires UTF-8 text");
    let text = std::str::from_utf8(bytes).context(
        "Partial staging requires UTF-8 text; use whole-file staging for other encodings",
    )?;
    ensure!(
        line_count(text) <= text_diff::MAX_REVIEW_LINES,
        "Too many lines for partial staging (limit 200,000)"
    );
    Ok(text)
}

fn read_worktree(path: &Path) -> Result<Vec<u8>> {
    let mut data = Vec::new();
    match std::fs::File::open(path) {
        Ok(file) => {
            file.take(MAX_REVIEW_BYTES as u64 + 1)
                .read_to_end(&mut data)?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    Ok(data)
}

fn line_count(text: &str) -> usize {
    text.split_inclusive('\n').count()
}

fn hunks(before: &str, after: &str) -> Vec<crate::Hunk> {
    let old = line_count(before) as u32;
    let new = line_count(after) as u32;
    text_diff::LineDiff::new(Rope::from(before), Rope::from(after))
        .hunks()
        .map(|mut hunk| {
            // Rope line iterators include a virtual final empty line; Git patches do not.
            hunk.before = hunk.before.start.min(old)..hunk.before.end.min(old);
            hunk.after = hunk.after.start.min(new)..hunk.after.end.min(new);
            hunk
        })
        .filter(|hunk| !hunk.before.is_empty() || !hunk.after.is_empty())
        .collect()
}

#[derive(Clone, Copy)]
enum Side {
    Before,
    After,
}

/// Pair replacement lines by position; surplus removals/additions accompany a fully selected hunk.
fn select_changes(
    before: &str,
    after: &str,
    selection: &PartialSelection,
    side: Side,
) -> Result<String> {
    let old: Vec<_> = before.split_inclusive('\n').collect();
    let new: Vec<_> = after.split_inclusive('\n').collect();
    let mut result = String::new();
    let mut previous = 0;
    let mut selected = false;
    for hunk in hunks(before, after) {
        if let PartialSelection::ReviewLines { before, after } = selection {
            result.extend(old[previous..hunk.before.start as usize].iter().copied());
            for line in hunk.before.clone() {
                if before.iter().any(|range| range.contains(&line)) {
                    selected = true;
                } else {
                    result.push_str(old[line as usize]);
                }
            }
            for line in hunk.after.clone() {
                if after.iter().any(|range| range.contains(&line)) {
                    result.push_str(new[line as usize]);
                    selected = true;
                }
            }
            previous = hunk.before.end as usize;
            continue;
        }
        let range = match side {
            Side::Before => &hunk.before,
            Side::After => &hunk.after,
        };
        let intersects = if range.is_empty() {
            selection.contains(range.start)
        } else {
            range.clone().any(|line| selection.contains(line))
        };
        result.extend(old[previous..hunk.before.start as usize].iter().copied());
        if !intersects {
            result.extend(
                old[hunk.before.start as usize..hunk.before.end as usize]
                    .iter()
                    .copied(),
            );
        } else if matches!(selection, PartialSelection::Hunks(_))
            || range.is_empty()
            || range.clone().all(|line| selection.contains(line))
        {
            result.extend(
                new[hunk.after.start as usize..hunk.after.end as usize]
                    .iter()
                    .copied(),
            );
            selected = true;
        } else {
            for offset in 0..hunk.before.len().max(hunk.after.len()) {
                let line = range.start + offset as u32;
                let take = offset < range.len() && selection.contains(line);
                let old_line =
                    (offset < hunk.before.len()).then(|| old[hunk.before.start as usize + offset]);
                let new_line =
                    (offset < hunk.after.len()).then(|| new[hunk.after.start as usize + offset]);
                match (take, old_line, new_line) {
                    (true, _, Some(line)) | (false, Some(line), _) => result.push_str(line),
                    _ => {}
                }
                selected |= take;
            }
        }
        previous = hunk.before.end as usize;
    }
    result.extend(old[previous..].iter().copied());
    ensure!(selected, "No selected text changes");
    Ok(result)
}

fn map_to_index(index: &str, input: &str, selection: PartialSelection) -> PartialSelection {
    let hunks = hunks(index, input);
    let mut mapped = Vec::new();
    for range in selection.ranges() {
        for line in range.clone() {
            let mut shift = 0i64;
            let mut mapped_line = None;
            for hunk in &hunks {
                if line < hunk.after.start {
                    break;
                }
                if hunk.after.contains(&line) {
                    let offset = line - hunk.after.start;
                    mapped_line =
                        (offset < hunk.before.len() as u32).then_some(hunk.before.start + offset);
                    shift = i64::MIN;
                    break;
                }
                shift = i64::from(hunk.before.end) - i64::from(hunk.after.end);
            }
            if shift != i64::MIN {
                mapped_line = u32::try_from(i64::from(line) + shift).ok();
            }
            if let Some(line) = mapped_line {
                mapped.push(line..line.saturating_add(1));
            }
        }
    }
    match selection {
        PartialSelection::Hunks(_) => PartialSelection::Hunks(mapped),
        PartialSelection::Lines(_) | PartialSelection::ReviewLines { .. } => {
            PartialSelection::Lines(mapped)
        }
    }
}

fn new_mode(repo: &gix::Repository, path: &Path) -> gix::index::entry::Mode {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if repo
            .config_snapshot()
            .boolean("core.filemode")
            .unwrap_or(true)
            && std::fs::metadata(path)
                .is_ok_and(|metadata| metadata.permissions().mode() & 0o111 != 0)
        {
            return gix::index::entry::Mode::FILE_EXECUTABLE;
        }
    }
    #[cfg(not(unix))]
    let _ = (repo, path);
    gix::index::entry::Mode::FILE
}

fn patch(
    path: &Path,
    before: &[u8],
    after: &[u8],
    mode: Option<gix::index::entry::Mode>,
    new_mode: gix::index::entry::Mode,
    remove: bool,
) -> Result<Vec<u8>> {
    let bytes = gix::path::to_unix_separators_on_windows(gix::path::try_into_bstr(path)?);
    let a = quoted_path(b"a/", bytes.as_ref());
    let b = quoted_path(b"b/", bytes.as_ref());
    let mut patch = format!("diff --git {a} {b}\n");
    let mode_str = if mode.unwrap_or(new_mode) == gix::index::entry::Mode::FILE_EXECUTABLE {
        "100755"
    } else {
        "100644"
    };
    if mode.is_none() {
        patch.push_str(&format!("new file mode {mode_str}\n"));
    }
    if remove {
        patch.push_str(&format!("deleted file mode {mode_str}\n"));
    }
    patch.push_str(&format!(
        "--- {}\n+++ {}\n",
        if mode.is_none() { "/dev/null" } else { &a },
        if remove { "/dev/null" } else { &b }
    ));
    let old: Vec<_> = before.split_inclusive(|byte| *byte == b'\n').collect();
    let new: Vec<_> = after.split_inclusive(|byte| *byte == b'\n').collect();
    patch.push_str(&format!(
        "@@ -{},{} +{},{} @@\n",
        usize::from(!old.is_empty()),
        old.len(),
        usize::from(!new.is_empty()),
        new.len()
    ));
    let mut patch = patch.into_bytes();
    for (marker, lines) in [(b'-', old), (b'+', new)] {
        for line in lines {
            patch.push(marker);
            patch.extend_from_slice(line);
            if !line.ends_with(b"\n") {
                patch.extend_from_slice(b"\n\\ No newline at end of file\n");
            }
        }
    }
    Ok(patch)
}

fn quoted_path(prefix: &[u8], path: &[u8]) -> String {
    let mut result = String::from("\"");
    for byte in prefix.iter().chain(path) {
        match byte {
            b' '..=b'~' if !matches!(byte, b'"' | b'\\') => result.push(char::from(*byte)),
            _ => result.push_str(&format!("\\{byte:03o}")),
        }
    }
    result.push('"');
    result
}
