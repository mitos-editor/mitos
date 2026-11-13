use std::{
    borrow::Cow,
    fs,
    path::{Path, PathBuf},
    str::FromStr as _,
    sync::Arc,
};
use stdx::rope::RopeExt as _;

use crate::{document::SavePoint, features::completion::ResponseContext, Document};
use editor_core::{Selection, Transaction};
use event::TaskHandle;
use stdx::path::{self, canonicalize, fold_home_dir, get_path_suffix};
use stdx::Url;

use super::{CompletionItems, CompletionResponse, TextCompletion};

pub(crate) fn path_completion(
    selection: Selection,
    doc: &Document,
    handle: TaskHandle,
    savepoint: Arc<SavePoint>,
) -> Option<impl FnOnce() -> CompletionResponse + use<>> {
    if !doc.path_completion_enabled() {
        return None;
    }

    let text = doc.text().clone();
    let cursor = selection.primary().cursor(text.slice(..));
    let cur_line = text.char_to_line_idx(cursor);
    let start = text
        .line_to_char_idx(cur_line)
        .max(cursor.saturating_sub(1000));
    let line_until_cursor = text.slice_chars(start..cursor);

    let (dir_path, typed_file_name) =
        get_path_suffix(line_until_cursor, false).and_then(|matched_path| {
            let matched_path = Cow::from(matched_path);
            let path: Cow<_> = if matched_path.starts_with("file://") {
                Url::from_str(&matched_path)
                    .ok()
                    .and_then(|url| url.to_file_path().ok())?
                    .into()
            } else {
                Path::new(&*matched_path).into()
            };
            let path = path::expand(&path);
            let parent_dir = doc.path().and_then(|dp| dp.parent());
            let path = match parent_dir {
                Some(parent_dir) if path.is_relative() => parent_dir.join(&path),
                _ => path.into_owned(),
            };
            #[cfg(windows)]
            let ends_with_slash = matches!(matched_path.as_bytes().last(), Some(b'/' | b'\\'));
            #[cfg(not(windows))]
            let ends_with_slash = matches!(matched_path.as_bytes().last(), Some(b'/'));

            if ends_with_slash {
                Some((PathBuf::from(path.as_path()), None))
            } else {
                path.parent().map(|parent_path| {
                    (
                        PathBuf::from(parent_path),
                        path.file_name().and_then(|f| f.to_str().map(String::from)),
                    )
                })
            }
        })?;

    let typed_file_name = typed_file_name.as_deref().unwrap_or_default();
    let edit_diff = typed_file_name.chars().count();
    // Every cursor must be at the same filename prefix. A matching suffix of a
    // longer filename is insufficient, and an empty prefix still needs a slash.
    if !selection.iter().all(|range| {
        let cursor = range.cursor(text.slice(..));
        let Some(start) = cursor.checked_sub(edit_diff).filter(|&start| start > 0) else {
            return false;
        };
        let separator = text.char_at(start - 1);
        (separator == '/' || cfg!(windows) && separator == '\\')
            && text
                .slice_chars(start..cursor)
                .chars()
                .eq(typed_file_name.chars())
    }) {
        return None;
    }

    if handle.is_canceled() {
        return None;
    }

    // TODO: handle properly in the future
    const PRIORITY: i8 = 1;
    let future = move || {
        let Ok(read_dir) = std::fs::read_dir(&dir_path) else {
            return CompletionResponse {
                items: CompletionItems::Path(Vec::new()),
                context: ResponseContext {
                    is_incomplete: false,
                    priority: PRIORITY,
                    savepoint,
                },
            };
        };

        let res: Vec<_> = read_dir
            .filter_map(Result::ok)
            .filter_map(|dir_entry| {
                dir_entry
                    .metadata()
                    .ok()
                    .and_then(|md| Some((dir_entry.file_name().into_string().ok()?, md)))
            })
            .map_while(|(file_name, md)| {
                if handle.is_canceled() {
                    return None;
                }

                let kind = path_kind(&md);
                let documentation = path_documentation(&md, &dir_path.join(&file_name), kind);

                let transaction = Transaction::change_by_selection(&text, &selection, |range| {
                    let cursor = range.cursor(text.slice(..));
                    (cursor - edit_diff, cursor, Some((&file_name).into()))
                });

                Some(TextCompletion {
                    kind: Cow::Borrowed(kind),
                    label: file_name.into(),
                    transaction,
                    documentation: Some(documentation),
                })
            })
            .collect();
        CompletionResponse {
            items: CompletionItems::Path(res),
            context: ResponseContext {
                is_incomplete: false,
                priority: PRIORITY,
                savepoint,
            },
        }
    };

    Some(future)
}

fn path_documentation(md: &fs::Metadata, full_path: &Path, kind: &str) -> String {
    let full_path = fold_home_dir(canonicalize(full_path));
    let full_path_name = full_path.to_string_lossy();
    let mut documentation = format!("type: `{kind}`\n");
    #[cfg(unix)]
    {
        use std::os::unix::prelude::PermissionsExt;
        let mode = md.permissions().mode();

        let perms = [
            (libc::S_IRUSR, 'r'),
            (libc::S_IWUSR, 'w'),
            (libc::S_IXUSR, 'x'),
            (libc::S_IRGRP, 'r'),
            (libc::S_IWGRP, 'w'),
            (libc::S_IXGRP, 'x'),
            (libc::S_IROTH, 'r'),
            (libc::S_IWOTH, 'w'),
            (libc::S_IXOTH, 'x'),
        ]
        .into_iter()
        .fold(String::with_capacity(9), |mut acc, (p, s)| {
            // This cast is necessary on some platforms such as macos as `mode_t` is u16 there
            #[allow(clippy::unnecessary_cast)]
            acc.push(if mode & (p as u32) > 0 { s } else { '-' });
            acc
        });

        documentation.push_str(&format!("permissions: `[{perms}]`\n"));
    }
    #[cfg(not(unix))]
    let _ = md;
    documentation.push_str(&format!("full path: `{full_path_name}`"));
    documentation
}

#[cfg(unix)]
fn path_kind(md: &fs::Metadata) -> &'static str {
    if md.is_symlink() {
        "link"
    } else if md.is_dir() {
        "folder"
    } else {
        use std::os::unix::fs::FileTypeExt;
        if md.file_type().is_block_device() {
            "block"
        } else if md.file_type().is_socket() {
            "socket"
        } else if md.file_type().is_char_device() {
            "char_device"
        } else if md.file_type().is_fifo() {
            "fifo"
        } else {
            "file"
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use editor_core::{Range, Rope};

    fn complete(
        text: &str,
        cursors: &[usize],
        primary: usize,
        filename: &str,
    ) -> Option<Vec<TextCompletion>> {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join(filename), "").unwrap();
        let mut doc = crate::document::test_document(text);
        doc.set_path(Some(&dir.path().join("buffer.txt")));
        assert!(doc.path_completion_enabled());
        let view = crate::ViewId::default();
        doc.ensure_view_init(view);
        let selection =
            Selection::new(cursors.iter().copied().map(Range::point).collect(), primary);
        doc.set_selection(view, selection.clone());
        let savepoint = doc.savepoint(view);
        let mut controller = event::TaskController::new();
        let response = path_completion(selection, &doc, controller.restart(), savepoint)?();
        let CompletionItems::Path(items) = response.items else {
            panic!("expected path completions")
        };
        Some(items)
    }

    #[test]
    fn secondary_cursor_before_prefix_cannot_underflow() {
        assert!(complete("./ap\n", &[0, 4], 1, "apple.txt").is_none());
    }

    #[test]
    fn incompatible_filename_prefixes_are_declined() {
        for (text, cursors) in [
            ("./a\n./ap\n", [3, 8]),
            ("hello\n./ap\n", [5, 10]),
            ("./map\n./ap\n", [5, 10]),
        ] {
            assert!(complete(text, &cursors, 1, "apple.txt").is_none(), "{text}");
        }
    }

    #[test]
    fn empty_prefixes_require_a_path_separator_at_every_cursor() {
        assert!(complete("./\nplain\n", &[2, 8], 0, "apple.txt").is_none());
    }

    #[test]
    fn matching_prefixes_replace_all_cursors() {
        for (input, cursors) in [("./ap\n./ap\n", [4, 9]), ("./\n./\n", [2, 5])] {
            let items = complete(input, &cursors, 0, "apple.txt").unwrap();
            assert_eq!(items.len(), 1);
            let mut text = Rope::from(input);
            assert!(items[0].transaction.apply(&mut text));
            assert_eq!(text, "./apple.txt\n./apple.txt\n");
        }
    }

    #[test]
    fn matching_unicode_prefixes_use_character_offsets() {
        let input = "😀 ./é亜\n😀 ./é亜\n";
        let items = complete(input, &[6, 13], 1, "é亜clair.txt").unwrap();
        let mut text = Rope::from(input);
        assert!(items[0].transaction.apply(&mut text));
        assert_eq!(text, "😀 ./é亜clair.txt\n😀 ./é亜clair.txt\n");
    }
}

#[cfg(not(unix))]
fn path_kind(md: &fs::Metadata) -> &'static str {
    if md.is_symlink() {
        "link"
    } else if md.is_dir() {
        "folder"
    } else {
        "file"
    }
}
