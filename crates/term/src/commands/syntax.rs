//! Syntax traversal, inspection, and symbol pickers.
use super::Context;
use crate::ui::{
    overlay::overlaid,
    picker::{Injector, PathOrId},
    Picker, PickerColumn,
};
use editor_core::{
    movement::{Direction, Movement},
    Selection,
};
use std::{ops::ControlFlow, sync::Arc};
use view::{
    align_view,
    document::{Mode, SCRATCH_BUFFER_NAME},
    features::syntax_symbols::{document_symbols, Symbol, Target, WorkspaceSymbols},
    Align, Editor,
};

fn target_path(target: &Target) -> Option<PathOrId<'_>> {
    match target {
        Target::Uri(uri) => uri.as_path().map(PathOrId::Path),
        Target::Document(id) => Some(PathOrId::Id(*id)),
    }
}

pub fn syntax_symbol_picker(cx: &mut Context) {
    let doc_id = doc!(cx.editor).id();
    cx.editor.ensure_syntax(doc_id);
    let doc = doc!(cx.editor);
    if doc.syntax().is_none() {
        cx.editor
            .set_error(|| "Syntax tree is not available on this buffer");
        return;
    }
    let tags = document_symbols(cx.editor, doc_id);
    let mut version = doc.version();

    let columns = vec![
        PickerColumn::new("kind", |tag: &Symbol, _| tag.tag.kind.as_str().into()),
        PickerColumn::new("name", |tag: &Symbol, _| tag.tag.name.as_str().into()),
    ];

    let picker = Picker::new(
        cx.editor.redraw().clone(),
        columns,
        1, // name
        tags,
        (),
        move |cx, tag, action| {
            cx.editor.switch(doc_id, action);
            let view = view_mut!(cx.editor);
            let doc = doc_mut!(cx.editor, &doc_id);
            doc.set_selection(view.id, Selection::single(tag.tag.start, tag.tag.end));
            if action.align_view(view, doc.id()) {
                align_view(doc, view, Align::Center)
            }
        },
    )
    .with_title("Document Symbols")
    .with_item_key(Symbol::key)
    .with_editor_updates(move |cx| {
        let Some(doc) = cx.editor.document(doc_id) else {
            return Some(Vec::new());
        };
        if doc.version() == version || doc.is_syntax_pending() {
            return None;
        }
        version = doc.version();
        Some(document_symbols(cx.editor, doc_id))
    })
    .with_preview(|_editor, tag| {
        Some((
            target_path(&tag.target)?,
            Some((tag.tag.start_line, tag.tag.end_line)),
        ))
    })
    .with_quicklist(|_editor, tag| tag.quicklist_entry())
    .truncate_start(false);

    cx.push_layer(Box::new(overlaid(cx.editor, picker)));
}

pub fn syntax_workspace_symbol_picker(cx: &mut Context) {
    let state = WorkspaceSymbols::new(
        cx.editor,
        doc!(cx.editor).id(),
        crate::files::picker_options(&cx.editor.config().file_picker),
    );
    let reg = cx.register.unwrap_or('/');
    cx.editor.registers.last_search_register = reg;
    let columns = vec![
        PickerColumn::new("kind", |tag: &Symbol, _| tag.tag.kind.as_str().into()),
        PickerColumn::new("name", |tag: &Symbol, _| tag.tag.name.as_str().into())
            .without_filtering(),
        PickerColumn::new("path", |tag: &Symbol, state: &WorkspaceSymbols| {
            match &tag.target {
                Target::Uri(uri) => {
                    if let Some(path) = uri.as_path() {
                        let path = if let Ok(stripped) = path.strip_prefix(state.root()) {
                            stripped
                        } else {
                            path
                        };
                        path.to_string_lossy().into()
                    } else {
                        uri.to_string().into()
                    }
                }
                // This picker only uses `Id` for scratch buffers for better display.
                Target::Document(_) => SCRATCH_BUFFER_NAME.into(),
            }
        }),
    ];

    let get_tags = |query: &str,
                    editor: &mut Editor,
                    state: Arc<WorkspaceSymbols>,
                    injector: &Injector<_, _>| {
        let sink = injector.clone();
        let cancel = injector.clone();
        state.query(
            editor,
            query,
            move |symbol| {
                if sink.push(symbol).is_ok() {
                    ControlFlow::Continue(())
                } else {
                    ControlFlow::Break(())
                }
            },
            move || cancel.is_shutdown(),
        )
    };
    let picker = Picker::new(cx.editor.redraw().clone(),
        columns,
        1, // name
        [],
        state,
        move |cx, tag, action| {
            let doc_id = match &tag.target {
                Target::Document(id) => *id,
                Target::Uri(uri) => match cx.editor.open(uri.as_path().expect(""), action) {
                    Ok(id) => id,
                    Err(e) => {
                        cx.editor
                            .set_error(|| format!("Failed to open file '{uri:?}': {e}"));
                        return;
                    }
                }
            };
            let doc = doc_mut!(cx.editor, &doc_id);
            let view = view_mut!(cx.editor);
            let len_chars = doc.text().len_chars();
            if tag.tag.start >= len_chars || tag.tag.end > len_chars {
                cx.editor.set_error(|| "The location you jumped to does not exist anymore because the file has changed.");
                return;
            }
            doc.set_selection(view.id, Selection::single(tag.tag.start, tag.tag.end));
            if action.align_view(view, doc.id()) {
                align_view(doc, view, Align::Center)
            }
        },
    )
    .with_title("Workspace Symbols")
    .with_dynamic_query(get_tags, Some(275), cx.jobs)
    .with_item_key(Symbol::key)
    .with_query_updates(|editor, state| state.refresh(editor))
    .with_preview(move |_editor, tag| {
        Some((
            target_path(&tag.target)?,
            Some((tag.tag.start_line, tag.tag.end_line)),
        ))
    })
    .with_quicklist(|_editor, tag| tag.quicklist_entry())
    .with_history_register(Some(reg))
    .truncate_start(false);
    cx.push_layer(Box::new(overlaid(cx.editor, picker)));
}

fn structural_selection(cx: &mut Context, action: view::editing::syntax::SelectionAction) {
    cx.editor.apply_motion(move |editor: &mut Editor| {
        let (view, doc) = current!(editor);
        view::editing::syntax::select(doc, view.id, action);
    });
}

pub(super) fn expand_selection(cx: &mut Context) {
    structural_selection(cx, view::editing::syntax::SelectionAction::Expand);
}

pub(super) fn shrink_selection(cx: &mut Context) {
    structural_selection(cx, view::editing::syntax::SelectionAction::Shrink);
}

pub(super) fn select_next_sibling(cx: &mut Context) {
    structural_selection(cx, view::editing::syntax::SelectionAction::NextSibling);
}

pub(super) fn select_prev_sibling(cx: &mut Context) {
    structural_selection(cx, view::editing::syntax::SelectionAction::PreviousSibling);
}

fn move_node_bound_impl(cx: &mut Context, dir: Direction, movement: Movement) {
    cx.editor.apply_motion(move |editor: &mut Editor| {
        let (view, doc) = current!(editor);
        view::editing::syntax::move_parent_boundary(doc, view.id, dir, movement);
    });
}

pub fn move_parent_node_end(cx: &mut Context) {
    move_node_bound_impl(cx, Direction::Forward, Movement::Move)
}

pub fn move_parent_node_start(cx: &mut Context) {
    move_node_bound_impl(cx, Direction::Backward, Movement::Move)
}

pub fn extend_parent_node_end(cx: &mut Context) {
    move_node_bound_impl(cx, Direction::Forward, Movement::Extend)
}

pub fn extend_parent_node_start(cx: &mut Context) {
    move_node_bound_impl(cx, Direction::Backward, Movement::Extend)
}

pub(super) fn select_all_siblings(cx: &mut Context) {
    structural_selection(cx, view::editing::syntax::SelectionAction::AllSiblings);
}

pub(super) fn select_all_children(cx: &mut Context) {
    structural_selection(cx, view::editing::syntax::SelectionAction::AllChildren);
}

pub(super) fn match_brackets(cx: &mut Context) {
    let movement = if cx.editor.mode() == Mode::Select {
        Movement::Extend
    } else {
        Movement::Move
    };
    let (view, doc) = current!(cx.editor);
    view::editing::syntax::match_brackets(doc, view.id, movement);
}

fn goto_ts_object_impl(cx: &mut Context, object: &'static str, direction: Direction) {
    let count = cx.count();
    cx.editor.apply_motion(move |editor: &mut Editor| {
        let movement = if editor.mode() == Mode::Select {
            Movement::Extend
        } else {
            Movement::Move
        };
        let (view, doc) = current!(editor);
        if !view::editing::syntax::goto_object(doc, view, object, direction, count, movement) {
            editor.set_status("Syntax-tree is not available in current buffer");
        }
    });
}

pub(super) fn goto_next_function(cx: &mut Context) {
    goto_ts_object_impl(cx, "function", Direction::Forward)
}

pub(super) fn goto_prev_function(cx: &mut Context) {
    goto_ts_object_impl(cx, "function", Direction::Backward)
}

pub(super) fn goto_next_class(cx: &mut Context) {
    goto_ts_object_impl(cx, "class", Direction::Forward)
}

pub(super) fn goto_prev_class(cx: &mut Context) {
    goto_ts_object_impl(cx, "class", Direction::Backward)
}

pub(super) fn goto_next_parameter(cx: &mut Context) {
    goto_ts_object_impl(cx, "parameter", Direction::Forward)
}

pub(super) fn goto_prev_parameter(cx: &mut Context) {
    goto_ts_object_impl(cx, "parameter", Direction::Backward)
}

pub(super) fn goto_next_comment(cx: &mut Context) {
    goto_ts_object_impl(cx, "comment", Direction::Forward)
}

pub(super) fn goto_prev_comment(cx: &mut Context) {
    goto_ts_object_impl(cx, "comment", Direction::Backward)
}

pub(super) fn goto_next_test(cx: &mut Context) {
    goto_ts_object_impl(cx, "test", Direction::Forward)
}

pub(super) fn goto_prev_test(cx: &mut Context) {
    goto_ts_object_impl(cx, "test", Direction::Backward)
}

pub(super) fn goto_next_xml_element(cx: &mut Context) {
    goto_ts_object_impl(cx, "xml-element", Direction::Forward)
}

pub(super) fn goto_prev_xml_element(cx: &mut Context) {
    goto_ts_object_impl(cx, "xml-element", Direction::Backward)
}

pub(super) fn goto_next_entry(cx: &mut Context) {
    goto_ts_object_impl(cx, "entry", Direction::Forward)
}

pub(super) fn goto_prev_entry(cx: &mut Context) {
    goto_ts_object_impl(cx, "entry", Direction::Backward)
}

pub(super) mod typed {
    //! Typable syntax commands.

    use crate::{
        compositor::{self, Compositor},
        job::{self, Callback},
        ui::{self, Popup, PromptEvent},
    };
    use ::command_line::Args;
    use anyhow::bail;
    use arc_swap::access::DynAccess;
    use stdx::rope::RopeExt;
    use view::Editor;

    #[cold]
    pub(in crate::commands) fn tree_sitter_scopes(
        cx: &mut compositor::Context,
        _args: Args,
        event: PromptEvent,
    ) -> anyhow::Result<()> {
        if event != PromptEvent::Validate {
            return Ok(());
        }

        let (view, doc) = current!(cx.editor);
        let text = doc.text().slice(..);

        let pos = doc.selection(view.id).primary().cursor(text);
        let scopes = language::indent::get_scopes(doc.syntax(), text, pos);

        let contents = format!("```json\n{:?}\n````", scopes);

        let callback = async move {
            let call: job::Callback = Callback::EditorCompositor(Box::new(
                move |editor: &mut Editor, compositor: &mut Compositor| {
                    let contents = ui::Markdown::new(contents, editor.syn_loader.clone());
                    let popup = Popup::new("hover", contents).auto_close(true);
                    compositor.replace_or_push("hover", popup);
                },
            ));
            Ok(call)
        };

        cx.jobs.callback(callback);

        Ok(())
    }

    #[cold]
    pub(in crate::commands) fn tree_sitter_highlight_name(
        cx: &mut compositor::Context,
        _args: Args,
        event: PromptEvent,
    ) -> anyhow::Result<()> {
        if event != PromptEvent::Validate {
            return Ok(());
        }

        let (view, doc) = current_ref!(cx.editor);
        let Some(syntax) = doc.syntax() else {
            return Ok(());
        };
        let text = doc.text().slice(..);
        let cursor = doc.selection(view.id).primary().cursor(text);
        let byte = text.char_to_byte_idx(cursor) as u32;
        // Query the same range as the one used in syntax highlighting.
        let range = {
            // Calculate viewport byte ranges:
            let row = text.char_to_line_idx(doc.view_offset(view.id).anchor.min(text.len_chars()));
            // Saturating subs to make it inclusive zero indexing.
            let last_line = text.len_lines(stdx::rope::LINE_TYPE).saturating_sub(1);
            let height = view.inner_area(doc).height;
            let last_visible_line = (row + height as usize).saturating_sub(1).min(last_line);
            let start = text.line_to_byte_idx(row.min(last_line), stdx::rope::LINE_TYPE) as u32;
            let end = text.line_to_byte_idx(last_visible_line + 1, stdx::rope::LINE_TYPE) as u32;

            start..end
        };

        let loader = cx.editor.syn_loader.load();
        let mut highlighter = syntax.highlighter(text, &loader, range);
        let mut highlights = Vec::new();

        while highlighter.next_event_offset() <= byte {
            let (event, new_highlights) = highlighter.advance();
            if event == editor_core::highlight::HighlightEvent::Refresh {
                highlights.clear();
            }
            highlights.extend(new_highlights);
        }

        let content = highlights
            .into_iter()
            .fold(String::new(), |mut acc, highlight| {
                if !acc.is_empty() {
                    acc.push_str(", ");
                }
                acc.push_str(cx.editor.theme.scope(highlight));
                acc
            });

        let callback = async move {
            let call: job::Callback = Callback::EditorCompositor(Box::new(
                move |editor: &mut Editor, compositor: &mut Compositor| {
                    let content = ui::Markdown::new(content, editor.syn_loader.clone());
                    let popup = Popup::new("hover", content).auto_close(true);
                    compositor.replace_or_push("hover", popup);
                },
            ));
            Ok(call)
        };

        cx.jobs.callback(callback);

        Ok(())
    }

    #[cold]
    pub(in crate::commands) fn tree_sitter_layers(
        cx: &mut compositor::Context,
        _args: Args,
        event: PromptEvent,
    ) -> anyhow::Result<()> {
        if event != PromptEvent::Validate {
            return Ok(());
        }

        let (view, doc) = current_ref!(cx.editor);
        let Some(syntax) = doc.syntax() else {
            bail!("Syntax information is not available");
        };

        let loader: &language::syntax::Loader = &cx.editor.syn_loader.load();
        let text = doc.text().slice(..);
        let cursor = doc.selection(view.id).primary().cursor(text);
        let byte = text.char_to_byte_idx(cursor) as u32;
        let languages =
            syntax
                .layers_for_byte_range(byte, byte)
                .fold(String::new(), |mut acc, layer| {
                    if !acc.is_empty() {
                        acc.push_str(", ");
                    }
                    acc.push_str(
                        &loader
                            .language(syntax.layer(layer).language)
                            .config()
                            .language_id,
                    );
                    acc
                });

        let callback = async move {
            let call: job::Callback = Callback::EditorCompositor(Box::new(
                move |editor: &mut Editor, compositor: &mut Compositor| {
                    let content = ui::Markdown::new(languages, editor.syn_loader.clone());
                    let popup = Popup::new("hover", content).auto_close(true);
                    compositor.replace_or_push("hover", popup);
                },
            ));
            Ok(call)
        };

        cx.jobs.callback(callback);

        Ok(())
    }

    #[cold]
    pub(in crate::commands) fn tree_sitter_subtree(
        cx: &mut compositor::Context,
        _args: Args,
        event: PromptEvent,
    ) -> anyhow::Result<()> {
        if event != PromptEvent::Validate {
            return Ok(());
        }

        let (view, doc) = current!(cx.editor);

        if let Some(syntax) = doc.syntax() {
            let primary_selection = doc.selection(view.id).primary();
            let text = doc.text();
            let from = text.char_to_byte_idx(primary_selection.from()) as u32;
            let to = text.char_to_byte_idx(primary_selection.to()) as u32;
            if let Some(selected_node) = syntax.descendant_for_byte_range(from, to) {
                let mut contents = String::from("```tsq\n");
                language::syntax::pretty_print_tree(&mut contents, selected_node)?;
                contents.push_str("\n```");

                let callback = async move {
                    let call: job::Callback = Callback::EditorCompositor(Box::new(
                        move |editor: &mut Editor, compositor: &mut Compositor| {
                            let contents = ui::Markdown::new(contents, editor.syn_loader.clone());
                            let popup = Popup::new("hover", contents).auto_close(true);
                            compositor.replace_or_push("hover", popup);
                        },
                    ));
                    Ok(call)
                };

                cx.jobs.callback(callback);
            }
        }

        Ok(())
    }
}
