//! Shared completion replacements and snippet rendering.

use editor_core::{chars, Rope, RopeSlice, Selection, Tendril, Transaction};
use snippets::{RenderedSnippet, Snippet, SnippetElement, SnippetRenderCtx};
use stdx::rope::RopeExt as _;

/// Extends a derived completion range backwards over a prefix of the insert text that
/// the user has already typed. A server that omits `textEdit` expects the client to
/// replace the word before the cursor, but an insert text like `$foo` (PHP, Svelte
/// runes), `@param` (docblocks) or `str/trim` (namespaces, paths) also carries
/// characters we do not count as part of a word, so the walk stops short of them and
/// they end up duplicated.
///
/// Whatever this consumes is re-inserted verbatim at the same offset, so the result is
/// the document you would get had the prefix not been typed by hand.
fn extend_range_over_typed_prefix(text: RopeSlice, start: usize, insert_text: &str) -> usize {
    // A match has to span the non-word character that stopped the word walk, so an
    // insert text made purely of word characters can never extend the range.
    if !insert_text.chars().any(|ch| !chars::char_is_word(ch)) {
        return start;
    }

    // Reach back at most as far as the insert text is long, and never across
    // whitespace, so a completion cannot swallow the word in front of it.
    let insert_len = insert_text.chars().count();
    let mut max_len = 0;
    for ch in text.chars_at_char(start).reversed() {
        if max_len == insert_len || ch.is_whitespace() {
            break;
        }
        max_len += 1;
    }

    // Longest match wins: for `ab/ab/c` and an insert text of `ab/ab/cd`, stopping at
    // the first match would leave `ab/ab/ab/cd`.
    for len in (1..=max_len).rev() {
        if text
            .slice_chars(start - len..start)
            .chars()
            .eq(insert_text.chars().take(len))
        {
            return start - len;
        }
    }
    start
}

/// If a provider did not supply a range for the completion or the range of the
/// primary cursor can not be used for the secondary cursor, this function
/// can be used to find the completion range for a cursor
fn find_completion_range(
    text: RopeSlice,
    replace_mode: bool,
    cursor: usize,
    insert_text: &str,
) -> (usize, usize) {
    let start = cursor
        - text
            .chars_at_char(cursor)
            .reversed()
            .take_while(|ch| chars::char_is_word(*ch))
            .count();
    let start = extend_range_over_typed_prefix(text, start, insert_text);
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
    insert_text: &str,
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
        None => find_completion_range(text, replace_mode, cursor, insert_text),
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
        Some(new_text.as_str().into())
    };

    let text = doc.slice(..);
    let (removed_start, removed_end) = completion_range(
        text,
        edit_offset,
        replace_mode,
        selection.primary().cursor(text),
        &new_text,
    )
    .expect("transaction must be valid for primary selection");
    let removed_text = text.slice_chars(removed_start..removed_end);

    let (transaction, mut selection) = Transaction::change_by_selection_ignore_overlapping(
        doc,
        selection,
        |range| {
            let cursor = range.cursor(text);
            completion_range(text, edit_offset, replace_mode, cursor, &new_text)
                .filter(|(start, end)| text.slice_chars(start..end) == removed_text)
                .unwrap_or_else(|| find_completion_range(text, replace_mode, cursor, &new_text))
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
    let insert_text = match snippet.elements().first() {
        Some(SnippetElement::Text(text)) => text.as_str(),
        _ => "",
    };
    let (removed_start, removed_end) = completion_range(
        text,
        edit_offset,
        replace_mode,
        selection.primary().cursor(text),
        insert_text,
    )
    .expect("transaction must be valid for primary selection");
    let removed_text = text.slice_chars(removed_start..removed_end);
    let (transaction, mapped_selection, snippet) = snippet.render(
        doc,
        selection,
        |range| {
            let cursor = range.cursor(text);
            completion_range(text, edit_offset, replace_mode, cursor, insert_text)
                .filter(|(start, end)| text.slice_chars(start..end) == removed_text)
                .unwrap_or_else(|| find_completion_range(text, replace_mode, cursor, insert_text))
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
    let (word_start, end) = find_completion_range(text, replace_mode, cursor, "");
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

#[cfg(test)]
mod lsp_prefix_tests {
    use super::*;
    use editor_core::indent::IndentStyle;
    /// Completes `new_text` with a single cursor at `cursor`.
    fn complete_with(
        doc: &str,
        cursor: usize,
        new_text: &str,
        edit_offset: Option<(i128, i128)>,
        replace_mode: bool,
    ) -> Rope {
        let mut doc = Rope::from(doc);
        let selection = Selection::point(cursor);
        let transaction = generate_transaction_from_completion_edit(
            &doc,
            &selection,
            edit_offset,
            replace_mode,
            new_text.to_string(),
        );
        assert!(transaction.apply(&mut doc));
        doc
    }

    /// Completes `new_text` with a single cursor at `cursor` and no server-supplied range.
    fn complete(doc: &str, cursor: usize, new_text: &str) -> Rope {
        complete_with(doc, cursor, new_text, None, false)
    }

    /// As [`complete`], but replacing the whole word around the cursor rather than only the
    /// part before it.
    fn complete_replace(doc: &str, cursor: usize, new_text: &str) -> Rope {
        complete_with(doc, cursor, new_text, None, true)
    }

    #[test]
    fn completion_range_covers_leading_sigils() {
        // Regression for issue #12544: a server that omits `textEdit` leaves the client to
        // derive the replaced range. `$` and `@` are not word characters, so the derived
        // range started after the sigil the user typed and the insert text duplicated it.
        assert_eq!(complete("$", 1, "$foo"), "$foo");
        assert_eq!(complete("$fo", 3, "$foo"), "$foo");
        assert_eq!(complete("@par", 4, "@param"), "@param");
        assert_eq!(complete("foo.", 4, ".bar"), "foo.bar");
    }

    #[test]
    fn completion_range_covers_typed_prefix_past_the_word() {
        // The non-word character need not be leading. Without this the derived range starts
        // at the last word boundary and everything before it is re-inserted.
        assert_eq!(
            complete("example/str/tr", 14, "str/trim"),
            "example/str/trim"
        );
        assert_eq!(complete("str/tr", 6, "str/trim"), "str/trim");
        assert_eq!(complete("界/名", 3, "界/名前"), "界/名前");
        assert_eq!(complete("foo.bar.ba", 10, "bar.baz"), "foo.bar.baz");
        // Nothing to match against, so only the word before the cursor is replaced.
        assert_eq!(complete("example/tr", 10, "str/trim"), "example/str/trim");
        // Longest match wins: stopping at the first would give `ab/ab/ab/cd`.
        assert_eq!(complete("ab/ab/c", 7, "ab/ab/cd"), "ab/ab/cd");
    }

    #[test]
    fn completion_range_does_not_over_consume() {
        // At most as many characters as the insert text is long, and the word walk still
        // stops at `:`.
        assert_eq!(complete("$$", 2, "$foo"), "$$foo");
        assert_eq!(complete("Foo::$b", 7, "$bar"), "Foo::$bar");
        assert_eq!(complete("fo", 2, "foo"), "foo");
        assert_eq!(complete("", 0, "$foo"), "$foo");
        // An empty insert text has no prefix to match and deletes the word before the cursor.
        assert_eq!(complete("fo", 2, ""), "");
    }

    #[test]
    fn completion_range_does_not_reach_across_whitespace() {
        // The range stops at whitespace so a completion cannot swallow the word in front of
        // it, even where reaching further would deduplicate more.
        assert_eq!(complete("a/b a/b", 7, "a/bc"), "a/b a/bc");
        assert_eq!(complete("a b", 3, "a bc"), "a a bc");
    }

    #[test]
    fn completion_range_covers_leading_sigils_in_replace_mode() {
        // Replace mode extends the range forwards over the rest of the word, which is
        // independent of the backwards extension over the sigil run.
        assert_eq!(complete_replace("$foo", 2, "$bar"), "$bar");
        assert_eq!(complete_replace("@param", 4, "@returns"), "@returns");
        // The sigil run still bounds how far back the range reaches.
        assert_eq!(complete_replace("$$foo", 3, "$bar"), "$$bar");
    }

    #[test]
    fn server_supplied_range_is_left_alone() {
        // A range from the server is used verbatim: one that already covers the sigil
        // completes cleanly, and one that does not is not silently widened.
        assert_eq!(
            complete_with("$fo", 3, "$foo", Some((-3, 0)), false),
            "$foo"
        );
        assert_eq!(
            complete_with("$fo", 3, "$foo", Some((-2, 0)), false),
            "$$foo"
        );
        assert_eq!(complete_with("$", 1, "$foo", Some((0, 0)), false), "$$foo");
    }

    #[test]
    fn completion_range_covers_sigils_for_every_cursor() {
        let mut doc = Rope::from("$ $");
        let selection = Selection::new(
            vec![editor_core::Range::point(1), editor_core::Range::point(3)].into(),
            0,
        );
        let transaction = generate_transaction_from_completion_edit(
            &doc,
            &selection,
            None,
            false,
            "$foo".to_string(),
        );
        assert!(transaction.apply(&mut doc));
        assert_eq!(doc, "$foo $foo");
    }

    #[test]
    fn snippet_completion_range_covers_leading_sigils() {
        // The snippet path reads its prefix from the parsed snippet, since a leading `$` in
        // raw snippet syntax is a variable marker rather than literal text.
        let mut doc = Rope::from("$");
        let selection = Selection::point(1);
        let snippet = Snippet::parse(r"\$foo($1)").unwrap();
        let mut ctx = SnippetRenderCtx {
            resolve_var: Box::new(|_, _| None),
            tab_width: 4,
            indent_style: IndentStyle::Spaces(4),
            line_ending: "\n",
        };
        let (transaction, _) =
            generate_transaction_from_snippet(&doc, &selection, None, false, &snippet, &mut ctx);
        assert!(transaction.apply(&mut doc));
        assert_eq!(doc, "$foo()");
    }
}
