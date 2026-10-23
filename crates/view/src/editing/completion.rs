//! Shared completion replacements and snippet rendering.

use editor_core::{chars, Rope, RopeSlice, Selection, Tendril, Transaction};
use snippets::{RenderedSnippet, Snippet, SnippetRenderCtx};
use stdx::rope::RopeExt as _;

/// If a provider did not supply a range for the completion or the range of the
/// primary cursor can not be used for the secondary cursor, this function
/// can be used to find the completion range for a cursor
fn find_completion_range(text: RopeSlice, replace_mode: bool, cursor: usize) -> (usize, usize) {
    let start = cursor
        - text
            .chars_at_char(cursor)
            .reversed()
            .take_while(|ch| chars::char_is_word(*ch))
            .count();
    let mut end = cursor;
    if replace_mode {
        end += text
            .chars_at_char(cursor)
            .take_while(|ch| chars::char_is_word(*ch))
            .count();
    }
    (start, end)
}
fn completion_range(
    text: RopeSlice,
    edit_offset: Option<(i128, i128)>,
    replace_mode: bool,
    cursor: usize,
) -> Option<(usize, usize)> {
    let res = match edit_offset {
        Some((start_offset, end_offset)) => {
            let start_offset = cursor as i128 + start_offset;
            if start_offset < 0 {
                return None;
            }
            let end_offset = cursor as i128 + end_offset;
            if end_offset > text.len_chars() as i128 {
                return None;
            }
            (start_offset as usize, end_offset as usize)
        }
        None => find_completion_range(text, replace_mode, cursor),
    };
    Some(res)
}

/// Creates a [Transaction] for a completion replacement.
/// The transaction applies the edit to all cursors.
pub fn generate_transaction_from_completion_edit(
    doc: &Rope,
    selection: &Selection,
    edit_offset: Option<(i128, i128)>,
    replace_mode: bool,
    new_text: String,
) -> Transaction {
    let replacement: Option<Tendril> = if new_text.is_empty() {
        None
    } else {
        Some(new_text.into())
    };

    let text = doc.slice(..);
    let (removed_start, removed_end) = completion_range(
        text,
        edit_offset,
        replace_mode,
        selection.primary().cursor(text),
    )
    .expect("transaction must be valid for primary selection");
    let removed_text = text.slice_chars(removed_start..removed_end);

    let (transaction, mut selection) = Transaction::change_by_selection_ignore_overlapping(
        doc,
        selection,
        |range| {
            let cursor = range.cursor(text);
            completion_range(text, edit_offset, replace_mode, cursor)
                .filter(|(start, end)| text.slice_chars(start..end) == removed_text)
                .unwrap_or_else(|| find_completion_range(text, replace_mode, cursor))
        },
        |_, _| replacement.clone(),
    );
    if transaction.changes().is_empty() {
        return transaction;
    }
    selection = selection.map(transaction.changes());
    transaction.with_selection(selection)
}

/// Creates a [Transaction] from the [Snippet] in a completion response.
/// The transaction applies the edit to all cursors.
pub fn generate_transaction_from_snippet(
    doc: &Rope,
    selection: &Selection,
    edit_offset: Option<(i128, i128)>,
    replace_mode: bool,
    snippet: &Snippet,
    cx: &mut SnippetRenderCtx,
) -> (Transaction, RenderedSnippet) {
    let text = doc.slice(..);
    let (removed_start, removed_end) = completion_range(
        text,
        edit_offset,
        replace_mode,
        selection.primary().cursor(text),
    )
    .expect("transaction must be valid for primary selection");
    let removed_text = text.slice_chars(removed_start..removed_end);
    let (transaction, mapped_selection, snippet) = snippet.render(
        doc,
        selection,
        |range| {
            let cursor = range.cursor(text);
            completion_range(text, edit_offset, replace_mode, cursor)
                .filter(|(start, end)| text.slice_chars(start..end) == removed_text)
                .unwrap_or_else(|| find_completion_range(text, replace_mode, cursor))
        },
        cx,
    );
    let transaction = transaction.with_selection(snippet.first_selection(
        // we keep the direction of the old primary selection in case it changed during mapping
        // but use the primary idx from the mapped selection in case ranges had to be merged
        selection.primary().direction(),
        mapped_selection.primary_index(),
    ));
    (transaction, snippet)
}

/// Render a native snippet, replacing the matching prefix independently at each cursor.
/// Punctuation in a trigger belongs to the replacement, rather than its preceding text.
pub fn generate_transaction_from_user_snippet(
    doc: &Rope,
    selection: &Selection,
    prefix: &str,
    replace_mode: bool,
    snippet: &Snippet,
    context: &mut SnippetRenderCtx,
) -> (Transaction, RenderedSnippet) {
    let text = doc.slice(..);
    let (transaction, mapped, rendered) = snippet.render(
        doc,
        selection,
        |range| user_snippet_range(text, range.cursor(text), prefix, replace_mode),
        context,
    );
    let transaction = transaction.with_selection(
        rendered.first_selection(selection.primary().direction(), mapped.primary_index()),
    );
    (transaction, rendered)
}

fn user_snippet_range(
    text: RopeSlice<'_>,
    cursor: usize,
    prefix: &str,
    replace_mode: bool,
) -> (usize, usize) {
    let (word_start, end) = find_completion_range(text, replace_mode, cursor);
    let boundaries: Vec<_> = prefix
        .char_indices()
        .map(|(index, character)| index + character.len_utf8())
        .collect();
    for count in (cursor - word_start + 1..=cursor.min(boundaries.len())).rev() {
        let start = cursor - count;
        if text.slice_chars(start..cursor) == prefix[..boundaries[count - 1]]
            && (start == 0
                || !prefix.starts_with(chars::char_is_word)
                || !chars::char_is_word(text.char_at(start - 1)))
        {
            return (start, end);
        }
    }
    (word_start, end)
}

#[cfg(test)]
mod native_prefix_tests {
    use super::*;
    #[test]
    fn punctuation_prefixes_use_character_coordinates_and_respect_word_boundaries() {
        for (source, cursor, prefix, replace, expected) in [
            ("for-c", 5, "for-const", false, (0, 5)),
            ("for!", 4, "for!", false, (0, 4)),
            ("!界-名", 4, "!界-名前", false, (0, 4)),
            ("beforefor!", 10, "for!", false, (10, 10)),
            ("for-consuffix", 6, "for-const", false, (0, 6)),
            ("for-consuffix", 6, "for-const", true, (0, 13)),
        ] {
            let text = Rope::from_str(source);
            assert_eq!(
                user_snippet_range(text.slice(..), cursor, prefix, replace),
                expected,
                "{source}"
            );
        }
    }
}
