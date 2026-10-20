//! Language-aware joining and comment operations over documents.

use crate::{editing::EditError, Document, ViewId};
use editor_core::{
    comment::{self, BlockCommentToken},
    line_ending::line_end_char_index,
    Range, Rope, RopeSlice, Selection, SmallVec, Tendril, Transaction,
};
use stdx::rope::RopeExt as _;

/// Reorder selected contents while retaining primary-selection and undo bookkeeping.
pub fn reorder_selection_contents(
    doc: &mut Document,
    view_id: ViewId,
    strategy: editor_core::transform::ReorderStrategy,
    count: usize,
) -> Result<(), EditError> {
    if let Some((transaction, selection)) = editor_core::transform::reorder_selection_contents(
        doc.text(),
        doc.selection(view_id),
        strategy,
        count,
    ) {
        let selection = selection.map(transaction.changes());
        super::try_apply(doc, &transaction.with_selection(selection), view_id)?;
    }
    Ok(())
}

/// Join selected lines, continuing comments across matching language layers.
pub fn join_selections(
    doc: &mut Document,
    view_id: ViewId,
    select_space: bool,
) -> Result<(), EditError> {
    use editor_core::movement::skip_while;
    let text = doc.text();
    let slice = text.slice(..);

    let mut changes = Vec::new();

    for selection in doc.selection(view_id) {
        let (start, mut end) = selection.line_range(slice);
        if start == end {
            end = (end + 1).min(text.len_lines(stdx::rope::LINE_TYPE) - 1);
        }
        let lines = start..end;

        changes.reserve(lines.len());

        let first_line_idx = slice.line_to_char_idx(start);
        let first_line_idx = skip_while(slice, first_line_idx, |ch| matches!(ch, ' ' | '\t'))
            .unwrap_or(first_line_idx);
        let mut current_comment_token =
            continued_line_comment_token(doc, slice, start, slice.char_to_byte_idx(first_line_idx));

        for line in lines {
            let start = line_end_char_index(&slice, line);
            let mut end = text.line_to_char_idx(line + 1);
            end = skip_while(slice, end, |ch| matches!(ch, ' ' | '\t')).unwrap_or(end);
            if let Some(token) =
                continued_line_comment_token(doc, slice, line + 1, slice.char_to_byte_idx(end))
            {
                if Some(token) == current_comment_token {
                    end += token.chars().count();
                    end = skip_while(slice, end, |ch| matches!(ch, ' ' | '\t')).unwrap_or(end);
                } else {
                    // update current token, but don't delete this one.
                    current_comment_token = Some(token);
                }
            }

            let separator = if end == line_end_char_index(&slice, line + 1) {
                // the joining line contains only space-characters => don't include a whitespace when joining
                None
            } else {
                Some(Tendril::from(" "))
            };
            changes.push((start, end, separator));
        }
    }

    // nothing to do, bail out early to avoid crashes later
    if changes.is_empty() {
        return Ok(());
    }

    changes.sort_unstable_by_key(|(from, _to, _text)| *from);
    changes.dedup();

    // select inserted spaces
    let transaction = if select_space {
        let mut offset: usize = 0;
        let ranges: SmallVec<_> = changes
            .iter()
            .filter_map(|change| {
                if change.2.is_some() {
                    let range = Range::point(change.0 - offset);
                    offset += change.1 - change.0 - 1; // -1 adjusts for the replacement of the range by a space
                    Some(range)
                } else {
                    offset += change.1 - change.0;
                    None
                }
            })
            .collect();
        let t = Transaction::change(text, changes.into_iter());
        if ranges.is_empty() {
            t
        } else {
            let selection = Selection::new(ranges, 0);
            t.with_selection(selection)
        }
    } else {
        Transaction::change(text, changes.into_iter())
    };

    super::try_apply(doc, &transaction, view_id)
}

// comments
type CommentTransactionFn = fn(
    line_token: Option<&str>,
    block_tokens: Option<&[BlockCommentToken]>,
    doc: &Rope,
    selection: &Selection,
) -> Transaction;

fn toggle_comments_impl(
    doc: &mut Document,
    view_id: ViewId,
    comment_transaction: CommentTransactionFn,
) -> Result<(), EditError> {
    let loader = doc.syntax_loader();
    let cursor = doc
        .selection(view_id)
        .primary()
        .cursor(doc.text().slice(..));
    let byte_pos = doc.text().char_to_byte_idx(cursor);
    // Resolve the comment tokens from the enclosing injection layer that owns the comment,
    // not the innermost layer at the cursor. Prefer the innermost layer that defines
    // *line* comment tokens, falling back to the innermost layer with block tokens.
    let mut line_layer = None;
    let mut block_layer = None;
    if let Some(syntax) = doc.syntax() {
        for layer in syntax.layers_for_byte_range(byte_pos as u32, byte_pos as u32) {
            let language = syntax.layer(layer).language;
            let config = loader.language(language).config();
            if config.comment_tokens.is_some() {
                line_layer = Some(language);
            }
            if config.block_comment_tokens.is_some() {
                block_layer = Some(language);
            }
        }
    }
    let lang_config = line_layer
        .or(block_layer)
        .map(|language| &**loader.language(language).config())
        .or_else(|| doc.language_config());

    // Pick the token the cursor's line is already commented with (longest match, so `///` wins over `//`).
    // If the line isn't commented yet, fall back to the primary token for adding a comment.
    let cursor_line = doc.text().char_to_line_idx(cursor);
    let line_token: Option<&str> = lang_config
        .and_then(|lc| lc.comment_tokens.as_ref())
        .and_then(|tokens| {
            comment::get_comment_token(doc.text().slice(..), tokens, cursor_line)
                .or_else(|| tokens.first().map(|token| token.as_str()))
        });
    let block_tokens: Option<&[BlockCommentToken]> = lang_config
        .and_then(|lc| lc.block_comment_tokens.as_ref())
        .map(|tc| &tc[..]);

    let transaction =
        comment_transaction(line_token, block_tokens, doc.text(), doc.selection(view_id));

    super::try_apply(doc, &transaction, view_id)
}

/// commenting behavior:
/// 1. only line comment tokens -> line comment
/// 2. each line block commented -> uncomment all lines
/// 3. whole selection block commented -> uncomment selection
/// 4. all lines not commented and block tokens -> comment uncommented lines
/// 5. no comment tokens and not block commented -> line comment
///
/// Toggle the appropriate line or block comment form for the selected language.
pub fn toggle_comments(doc: &mut Document, view_id: ViewId) -> Result<(), EditError> {
    toggle_comments_impl(doc, view_id, |line_token, block_tokens, doc, selection| {
        let text = doc.slice(..);

        // only have line comment tokens
        if line_token.is_some() && block_tokens.is_none() {
            return comment::toggle_line_comments(doc, selection, line_token);
        }

        let split_lines = comment::split_lines_of_selection(text, selection);

        let default_block_tokens = &[BlockCommentToken::default()];
        let block_comment_tokens = block_tokens.unwrap_or(default_block_tokens);

        let (line_commented, line_comment_changes) =
            comment::find_block_comments(block_comment_tokens, text, &split_lines);

        // block commented by line would also be block commented so check this first
        if line_commented {
            return comment::create_block_comment_transaction(
                doc,
                &split_lines,
                line_commented,
                line_comment_changes,
            )
            .0;
        }

        let (block_commented, comment_changes) =
            comment::find_block_comments(block_comment_tokens, text, selection);

        // check if selection has block comments
        if block_commented {
            return comment::create_block_comment_transaction(
                doc,
                selection,
                block_commented,
                comment_changes,
            )
            .0;
        }

        // not commented and only have block comment tokens
        if line_token.is_none() && block_tokens.is_some() {
            return comment::create_block_comment_transaction(
                doc,
                &split_lines,
                line_commented,
                line_comment_changes,
            )
            .0;
        }

        // not block commented at all and don't have any tokens
        comment::toggle_line_comments(doc, selection, line_token)
    })
}

/// Toggle line comments, falling back to blocks when the language requires them.
pub fn toggle_line_comments(doc: &mut Document, view_id: ViewId) -> Result<(), EditError> {
    toggle_comments_impl(doc, view_id, |line_token, block_tokens, doc, selection| {
        if line_token.is_none() && block_tokens.is_some() {
            let default_block_tokens = &[BlockCommentToken::default()];
            let block_comment_tokens = block_tokens.unwrap_or(default_block_tokens);
            comment::toggle_block_comments(
                doc,
                &comment::split_lines_of_selection(doc.slice(..), selection),
                block_comment_tokens,
            )
        } else {
            comment::toggle_line_comments(doc, selection, line_token)
        }
    })
}

/// Toggle block comments, falling back to lines when the language requires them.
pub fn toggle_block_comments(doc: &mut Document, view_id: ViewId) -> Result<(), EditError> {
    toggle_comments_impl(doc, view_id, |line_token, block_tokens, doc, selection| {
        if line_token.is_some() && block_tokens.is_none() {
            comment::toggle_line_comments(doc, selection, line_token)
        } else {
            let default_block_tokens = &[BlockCommentToken::default()];
            let block_comment_tokens = block_tokens.unwrap_or(default_block_tokens);
            comment::toggle_block_comments(doc, selection, block_comment_tokens)
        }
    })
}

/// Find the existing comment token in the language layer containing this position.
pub fn continued_line_comment_token<'a>(
    doc: &'a Document,
    text: RopeSlice,
    line_num: usize,
    byte_pos: usize,
) -> Option<&'a str> {
    let loader = doc.syntax_loader();
    if let Some(syntax) = doc.syntax() {
        let mut token = None;
        for layer in syntax.layers_for_byte_range(byte_pos as u32, byte_pos as u32) {
            let config = loader.language(syntax.layer(layer).language).config();
            if let Some(tokens) = config.comment_tokens.as_ref() {
                token = comment::get_comment_token(text, tokens, line_num).or(token);
            }
        }
        token
    } else {
        doc.language_config()
            .and_then(|config| config.comment_tokens.as_ref())
            .and_then(|tokens| comment::get_comment_token(text, tokens, line_num))
    }
}
