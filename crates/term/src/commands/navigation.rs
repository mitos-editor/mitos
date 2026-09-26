//! Location jumps, jump history, and labeled word navigation.

use super::context::Context;
use crate::{
    commands::picker::PathStyleConfig,
    ui::{self, overlay::overlaid, Picker},
};
use editor_core::{
    chars::char_is_word,
    graphemes::{self, next_grapheme_boundary},
    line_ending::line_end_char_index,
    movement as core_movement,
    movement::{Direction, Movement},
    text_annotations::Overlay,
    Range, Selection, Tendril,
};
use std::{borrow::Cow, num::NonZeroUsize, path::Path};
use stdx::rope::RopeSliceExt;
use view::{
    document::Mode,
    editor::Action,
    quicklist::{QuicklistEntry, QuicklistPosition, QuicklistTarget},
    Document, DocumentId, Editor, View,
};

pub(super) fn goto_next_buffer(cx: &mut Context) {
    goto_buffer(cx.editor, Direction::Forward, cx.count());
}

pub(super) fn goto_previous_buffer(cx: &mut Context) {
    goto_buffer(cx.editor, Direction::Backward, cx.count());
}

pub(super) fn goto_buffer(editor: &mut Editor, direction: Direction, count: usize) {
    let current = view!(editor).doc;

    let id = match direction {
        Direction::Forward => {
            let iter = editor.documents.keys();
            // skip 'count' times past current buffer
            iter.cycle().skip_while(|id| *id != &current).nth(count)
        }
        Direction::Backward => {
            let iter = editor.documents.keys();
            // skip 'count' times past current buffer
            iter.rev()
                .cycle()
                .skip_while(|id| *id != &current)
                .nth(count)
        }
    }
    .unwrap();

    let id = *id;

    editor.switch(id, Action::Replace);
}

pub(super) fn goto_file_start(cx: &mut Context) {
    goto_file_start_impl(cx, Movement::Move);
}

pub(super) fn extend_to_file_start(cx: &mut Context) {
    goto_file_start_impl(cx, Movement::Extend);
}

fn goto_file_start_impl(cx: &mut Context, movement: Movement) {
    if cx.count.is_some() {
        goto_line_impl(cx, movement);
    } else {
        let (view, doc) = current!(cx.editor);
        let text = doc.text().slice(..);
        let selection = doc
            .selection(view.id)
            .clone()
            .transform(|range| range.put_cursor(text, 0, movement == Movement::Extend));
        push_jump(view, doc);
        doc.set_selection(view.id, selection);
    }
}

pub(super) fn goto_file_end(cx: &mut Context) {
    goto_file_end_impl(cx, Movement::Move);
}

pub(super) fn extend_to_file_end(cx: &mut Context) {
    goto_file_end_impl(cx, Movement::Extend)
}

fn goto_file_end_impl(cx: &mut Context, movement: Movement) {
    let (view, doc) = current!(cx.editor);
    let text = doc.text().slice(..);
    let pos = doc.text().len_chars();
    let selection = doc
        .selection(view.id)
        .clone()
        .transform(|range| range.put_cursor(text, pos, movement == Movement::Extend));
    push_jump(view, doc);
    doc.set_selection(view.id, selection);
}

// Store a jump on the jumplist.
pub(super) fn push_jump(view: &mut View, doc: &mut Document) {
    doc.append_changes_to_history(view);
    let jump = (doc.id(), doc.selection(view.id).clone());
    view.push_jump(doc, jump);
}

pub(super) fn goto_line(cx: &mut Context) {
    goto_line_impl(cx, Movement::Move);
}

fn goto_line_impl(cx: &mut Context, movement: Movement) {
    if cx.count.is_some() {
        let (view, doc) = current!(cx.editor);
        push_jump(view, doc);

        goto_line_without_jumplist(cx.editor, cx.count, movement);
    }
}

pub(super) fn goto_line_without_jumplist(
    editor: &mut Editor,
    count: Option<NonZeroUsize>,
    movement: Movement,
) {
    if let Some(count) = count {
        let (view, doc) = current!(editor);
        let text = doc.text().slice(..);
        let max_line = if text.line(text.len_lines() - 1).len_chars() == 0 {
            // If the last line is blank, don't jump to it.
            text.len_lines().saturating_sub(2)
        } else {
            text.len_lines() - 1
        };
        let line_idx = std::cmp::min(count.get() - 1, max_line);
        let pos = text.line_to_char(line_idx);
        let selection = doc
            .selection(view.id)
            .clone()
            .transform(|range| range.put_cursor(text, pos, movement == Movement::Extend));

        doc.set_selection(view.id, selection);
    }
}

pub(super) fn goto_last_line(cx: &mut Context) {
    goto_last_line_impl(cx, Movement::Move)
}

pub(super) fn extend_to_last_line(cx: &mut Context) {
    goto_last_line_impl(cx, Movement::Extend)
}

fn goto_last_line_impl(cx: &mut Context, movement: Movement) {
    let (view, doc) = current!(cx.editor);
    let text = doc.text().slice(..);
    let line_idx = if text.line(text.len_lines() - 1).len_chars() == 0 {
        // If the last line is blank, don't jump to it.
        text.len_lines().saturating_sub(2)
    } else {
        text.len_lines() - 1
    };
    let pos = text.line_to_char(line_idx);
    let selection = doc
        .selection(view.id)
        .clone()
        .transform(|range| range.put_cursor(text, pos, movement == Movement::Extend));

    push_jump(view, doc);
    doc.set_selection(view.id, selection);
}

pub(super) fn goto_column(cx: &mut Context) {
    goto_column_impl(cx, Movement::Move);
}

pub(super) fn extend_to_column(cx: &mut Context) {
    goto_column_impl(cx, Movement::Extend);
}

fn goto_column_impl(cx: &mut Context, movement: Movement) {
    let count = cx.count();
    let (view, doc) = current!(cx.editor);
    let text = doc.text().slice(..);
    let selection = doc.selection(view.id).clone().transform(|range| {
        let line = range.cursor_line(text);
        let line_start = text.line_to_char(line);
        let line_end = line_end_char_index(&text, line);
        let pos = graphemes::nth_next_grapheme_boundary(text, line_start, count - 1).min(line_end);
        range.put_cursor(text, pos, movement == Movement::Extend)
    });
    push_jump(view, doc);
    doc.set_selection(view.id, selection);
}

pub(super) fn goto_last_accessed_file(cx: &mut Context) {
    let view = view_mut!(cx.editor);
    if let Some(alt) = view.docs_access_history.pop() {
        cx.editor.switch(alt, Action::Replace);
    } else {
        cx.editor.set_error(|| "no last accessed buffer")
    }
}

pub(super) fn goto_last_modification(cx: &mut Context) {
    let (view, doc) = current!(cx.editor);
    let pos = doc.history.get_mut().last_edit_pos();
    let text = doc.text().slice(..);
    if let Some(pos) = pos {
        let selection = doc
            .selection(view.id)
            .clone()
            .transform(|range| range.put_cursor(text, pos, cx.editor.mode == Mode::Select));
        push_jump(view, doc);
        doc.set_selection(view.id, selection);
    }
}

pub(super) fn goto_last_modified_file(cx: &mut Context) {
    let view = view!(cx.editor);
    let alternate_file = view
        .last_modified_docs
        .into_iter()
        .flatten()
        .find(|&id| id != view.doc);
    if let Some(alt) = alternate_file {
        cx.editor.switch(alt, Action::Replace);
    } else {
        cx.editor.set_error(|| "no last modified buffer")
    }
}

pub(super) fn jump_forward(cx: &mut Context) {
    cx.editor.jump_forward(cx.editor.tree.focus, cx.count());
}

pub(super) fn jump_backward(cx: &mut Context) {
    cx.editor.jump_backward(cx.editor.tree.focus, cx.count());
}

pub(super) fn save_selection(cx: &mut Context) {
    let (view, doc) = current!(cx.editor);
    push_jump(view, doc);
    cx.editor.set_status("Selection saved to jumplist");
}

pub(super) fn jumplist_picker(cx: &mut Context) {
    struct JumpMeta<'a> {
        id: DocumentId,
        path: Option<Cow<'a, Path>>,
        selection: Selection,
        line_start: usize,
        text: String,
        is_current: bool,
    }

    for (view, _) in cx.editor.tree.views_mut() {
        for doc_id in view.jumps.iter().map(|e| e.0).collect::<Vec<_>>().iter() {
            let doc = doc_mut!(cx.editor, doc_id);
            view.sync_changes(doc);
        }
    }

    let new_meta = |view: &View, doc_id: DocumentId, selection: Selection| {
        let doc = doc!(cx.editor, &doc_id);
        let text = doc.text().slice(..);
        let contents = selection
            .fragments(text)
            .map(Cow::into_owned)
            .collect::<Vec<_>>()
            .join(" ");
        let line_start = selection.primary().cursor_line(text);

        JumpMeta {
            id: doc_id,
            path: doc
                .path()
                .map(ToOwned::to_owned)
                .map(stdx::path::get_relative_path),
            selection,
            line_start,
            text: contents,
            is_current: view.doc == doc_id,
        }
    };

    let columns = [
        ui::PickerColumn::new("id", |item: &JumpMeta, _| item.id.to_string().into()),
        ui::PickerColumn::new("path", |item: &JumpMeta, config: &PathStyleConfig| {
            config.stylize(item.path.as_deref(), Some(item.line_start))
        }),
        ui::PickerColumn::new("flags", |item: &JumpMeta, _| {
            let mut flags = Vec::new();
            if item.is_current {
                flags.push("*");
            }

            if flags.is_empty() {
                "".into()
            } else {
                format!(" ({})", flags.join("")).into()
            }
        }),
        ui::PickerColumn::new("contents", |item: &JumpMeta, _| item.text.as_str().into()),
    ];

    let picker = Picker::new(
        columns,
        1, // path
        cx.editor.tree.views().flat_map(|(view, _)| {
            view.jumps
                .iter()
                .rev()
                .map(|(doc_id, selection)| new_meta(view, *doc_id, selection.clone()))
        }),
        PathStyleConfig::new(cx.editor),
        |cx, meta, action| {
            cx.editor.switch(meta.id, action);
            let config = cx.editor.config();
            let (view, doc) = (view_mut!(cx.editor), doc_mut!(cx.editor, &meta.id));
            doc.set_selection(view.id, meta.selection.clone());
            if action.align_view(view, doc.id()) {
                view.ensure_cursor_in_view_center(doc, config.scrolloff);
            }
        },
    )
    .with_preview(|editor, meta| {
        let doc = &editor.documents.get(&meta.id)?;
        let line = meta.selection.primary().cursor_line(doc.text().slice(..));
        Some((meta.id.into(), Some((line, line))))
    })
    .with_quicklist(|_editor, meta| {
        Some(QuicklistEntry {
            target: QuicklistTarget::Document(meta.id),
            position: QuicklistPosition::Selection(meta.selection.clone()),
        })
    });
    cx.push_layer(Box::new(overlaid(picker)));
}

pub(super) fn goto_word(cx: &mut Context) {
    jump_to_word(cx, Movement::Move)
}

pub(super) fn extend_to_word(cx: &mut Context) {
    jump_to_word(cx, Movement::Extend)
}

fn jump_to_label(cx: &mut Context, labels: Vec<Range>, behaviour: Movement) {
    let doc = doc!(cx.editor);
    let alphabet = &cx.editor.config().jump_label_alphabet;
    if labels.is_empty() {
        return;
    }
    let alphabet_char = |i| {
        let mut res = Tendril::new();
        res.push(alphabet[i]);
        res
    };

    // Add label for each jump candidate to the View as virtual text.
    let text = doc.text().slice(..);
    let mut overlays: Vec<_> = labels
        .iter()
        .enumerate()
        .flat_map(|(i, range)| {
            [
                Overlay::new(range.from(), alphabet_char(i / alphabet.len())),
                Overlay::new(
                    graphemes::next_grapheme_boundary(text, range.from()),
                    alphabet_char(i % alphabet.len()),
                ),
            ]
        })
        .collect();
    overlays.sort_unstable_by_key(|overlay| overlay.char_idx);
    let (view, doc) = current!(cx.editor);
    doc.set_jump_labels(view.id, overlays);

    // Accept two characters matching a visible label. Jump to the candidate
    // for that label if it exists.
    let primary_selection = doc.selection(view.id).primary();
    let view_id = view.id;
    let doc = doc.id();
    cx.on_next_key(move |cx, event| {
        let alphabet = &cx.editor.config().jump_label_alphabet;
        let Some(i) = event
            .char()
            .filter(|_| event.modifiers.is_empty())
            .and_then(|ch| alphabet.iter().position(|&it| it == ch))
        else {
            doc_mut!(cx.editor, &doc).remove_jump_labels(view_id);
            return;
        };
        let outer = i * alphabet.len();
        // Bail if the given character cannot be a jump label.
        if outer > labels.len() {
            doc_mut!(cx.editor, &doc).remove_jump_labels(view_id);
            return;
        }
        cx.on_next_key(move |cx, event| {
            doc_mut!(cx.editor, &doc).remove_jump_labels(view_id);
            let alphabet = &cx.editor.config().jump_label_alphabet;
            let Some(inner) = event
                .char()
                .filter(|_| event.modifiers.is_empty())
                .and_then(|ch| alphabet.iter().position(|&it| it == ch))
            else {
                return;
            };
            if let Some(mut range) = labels.get(outer + inner).copied() {
                range = if behaviour == Movement::Extend {
                    let anchor = if range.anchor < range.head {
                        let from = primary_selection.from();
                        if range.anchor < from {
                            range.anchor
                        } else {
                            from
                        }
                    } else {
                        let to = primary_selection.to();
                        if range.anchor > to {
                            range.anchor
                        } else {
                            to
                        }
                    };
                    Range::new(anchor, range.head)
                } else {
                    range.with_direction(Direction::Forward)
                };
                let doc = doc_mut!(cx.editor, &doc);
                let view = view_mut!(cx.editor, view_id);
                push_jump(view, doc);
                doc.set_selection(view_id, range.into());
            }
        });
    });
}

fn jump_to_word(cx: &mut Context, behaviour: Movement) {
    // Calculate the jump candidates: ranges for any visible words with two or
    // more characters.
    let alphabet = &cx.editor.config().jump_label_alphabet;
    if alphabet.is_empty() {
        return;
    }

    let jump_label_limit = alphabet.len() * alphabet.len();
    let mut words = Vec::with_capacity(jump_label_limit);
    let (view, doc) = current_ref!(cx.editor);
    let text = doc.text().slice(..);

    // This is not necessarily exact if there is virtual text like soft wrap.
    // It's ok though because the extra jump labels will not be rendered.
    let start = text.line_to_char(text.char_to_line(doc.view_offset(view.id).anchor));
    let end = text.line_to_char(view.estimate_last_doc_line(doc) + 1);

    let primary_selection = doc.selection(view.id).primary();
    let cursor = primary_selection.cursor(text);
    let mut cursor_fwd = Range::point(cursor);
    let mut cursor_rev = Range::point(cursor);
    if text.get_char(cursor).is_some_and(|c| !c.is_whitespace()) {
        let cursor_word_end = core_movement::move_next_word_end(text, cursor_fwd, 1);
        //  single grapheme words need a special case
        if cursor_word_end.anchor == cursor {
            cursor_fwd = cursor_word_end;
        }
        let cursor_word_start = core_movement::move_prev_word_start(text, cursor_rev, 1);
        if cursor_word_start.anchor == next_grapheme_boundary(text, cursor) {
            cursor_rev = cursor_word_start;
        }
    }
    'outer: loop {
        let mut changed = false;
        while cursor_fwd.head < end {
            cursor_fwd = core_movement::move_next_word_end(text, cursor_fwd, 1);
            // The cursor is on a word that is atleast two graphemes long and
            // madeup of word characters. The latter condition is needed because
            // move_next_word_end simply treats a sequence of characters from
            // the same char class as a word so `=<` would also count as a word.
            let add_label = text
                .slice(..cursor_fwd.head)
                .graphemes_rev()
                .take(2)
                .take_while(|g| g.chars().all(char_is_word))
                .count()
                == 2;
            if !add_label {
                continue;
            }
            changed = true;
            // skip any leading whitespace
            cursor_fwd.anchor += text
                .chars_at(cursor_fwd.anchor)
                .take_while(|&c| !char_is_word(c))
                .count();
            words.push(cursor_fwd);
            if words.len() == jump_label_limit {
                break 'outer;
            }
            break;
        }
        while cursor_rev.head > start {
            cursor_rev = core_movement::move_prev_word_start(text, cursor_rev, 1);
            // The cursor is on a word that is atleast two graphemes long and
            // madeup of word characters. The latter condition is needed because
            // move_prev_word_start simply treats a sequence of characters from
            // the same char class as a word so `=<` would also count as a word.
            let add_label = text
                .slice(cursor_rev.head..)
                .graphemes()
                .take(2)
                .take_while(|g| g.chars().all(char_is_word))
                .count()
                == 2;
            if !add_label {
                continue;
            }
            changed = true;
            cursor_rev.anchor -= text
                .chars_at(cursor_rev.anchor)
                .reversed()
                .take_while(|&c| !char_is_word(c))
                .count();
            words.push(cursor_rev);
            if words.len() == jump_label_limit {
                break 'outer;
            }
            break;
        }
        if !changed {
            break;
        }
    }
    jump_to_label(cx, words, behaviour)
}

pub(super) mod typed {
    //! Typable navigation commands.

    use crate::{commands::navigation::goto_line_without_jumplist, compositor, ui::PromptEvent};
    use ::command_line::Args;
    use editor_core::movement::Movement;
    use std::num::NonZeroUsize;
    use view::document::Mode;

    fn abort_goto_line_number_preview(cx: &mut compositor::Context) {
        if let Some(last_selection) = cx.editor.last_selection.take() {
            let scrolloff = cx.editor.config().scrolloff;

            let (view, doc) = current!(cx.editor);
            doc.set_selection(view.id, last_selection);
            view.ensure_cursor_in_view(doc, scrolloff);
        }
    }

    fn update_goto_line_number_preview(
        cx: &mut compositor::Context,
        args: Args,
    ) -> anyhow::Result<()> {
        cx.editor.last_selection.get_or_insert_with(|| {
            let (view, doc) = current!(cx.editor);
            doc.selection(view.id).clone()
        });

        let scrolloff = cx.editor.config().scrolloff;
        let line = args[0].parse::<usize>()?;
        goto_line_without_jumplist(
            cx.editor,
            NonZeroUsize::new(line),
            if cx.editor.mode == Mode::Select {
                Movement::Extend
            } else {
                Movement::Move
            },
        );

        let (view, doc) = current!(cx.editor);
        view.ensure_cursor_in_view(doc, scrolloff);

        Ok(())
    }

    #[cold]
    pub(in crate::commands) fn goto_line_number(
        cx: &mut compositor::Context,
        args: Args,
        event: PromptEvent,
    ) -> anyhow::Result<()> {
        match event {
            PromptEvent::Abort => abort_goto_line_number_preview(cx),
            PromptEvent::Validate => {
                // If we are invoked directly via a keybinding, Validate is
                // sent without any prior Update events. Ensure the cursor
                // is moved to the appropriate location.
                update_goto_line_number_preview(cx, args)?;

                let last_selection = cx
                    .editor
                    .last_selection
                    .take()
                    .expect("update_goto_line_number_preview should always set last_selection");

                let (view, doc) = current!(cx.editor);
                view.push_jump(doc, (doc.id(), last_selection));
            }

            // When a user hits backspace and there are no numbers left,
            // we can bring them back to their original selection. If they
            // begin typing numbers again, we'll start a new preview session.
            PromptEvent::Update if args.is_empty() => abort_goto_line_number_preview(cx),
            PromptEvent::Update => update_goto_line_number_preview(cx, args)?,
        }

        Ok(())
    }
}
