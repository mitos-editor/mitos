//! Text-object selection and its interactive prompts.

use crate::commands::{context::Context, spelling::spelling_ranges, vcs::hunk_range};
use arc_swap::access::DynAccess;
use editor_core::{
    graphemes::next_grapheme_boundary, textobject, Range, RopeSlice, Selection, SmallVec, Syntax,
};
use view::{info::Info, Editor};

pub(super) fn select_textobject_around(cx: &mut Context) {
    select_textobject(cx, textobject::TextObject::Around);
}

pub(super) fn select_textobject_inner(cx: &mut Context) {
    select_textobject(cx, textobject::TextObject::Inside);
}

fn select_all_treesitter_textobject_ranges(
    text: RopeSlice,
    range: Range,
    objtype: textobject::TextObject,
    obj_name: &str,
    syntax: Option<&Syntax>,
    loader: &editor_core::syntax::Loader,
) -> SmallVec<[Range; 1]> {
    let Some(syntax) = syntax else {
        return SmallVec::new();
    };

    let from = text.char_to_byte(range.from()) as u32;
    let to = text.char_to_byte(range.to()) as u32;
    // Syntax layer ranges use an inclusive end while editor ranges use an exclusive end.
    let end = if to > from { to - 1 } else { to };
    let layer = syntax.layer_for_byte_range(from, end);
    let root = syntax.tree_for_byte_range(from, end).root_node();
    let Some(textobject_query) = loader.textobject_query(syntax.layer(layer).language) else {
        return SmallVec::new();
    };

    let capture_name = format!("{obj_name}.{objtype}");
    // TODO(perf): Limit the query cursor to `from..to` once textobject queries support ranges.
    let Some(nodes) = textobject_query.capture_nodes(&capture_name, &root, text) else {
        return SmallVec::new();
    };

    nodes
        .filter_map(|node| {
            let start_byte = node.start_byte();
            let end_byte = node.end_byte();
            if start_byte > text.len_bytes() || end_byte > text.len_bytes() {
                return None;
            }

            let object_range =
                Range::new(text.byte_to_char(start_byte), text.byte_to_char(end_byte))
                    .with_direction(range.direction());

            range.contains_range(&object_range).then_some(object_range)
        })
        .collect()
}

fn select_all_word_textobject_ranges(
    text: RopeSlice,
    range: Range,
    objtype: textobject::TextObject,
    long: bool,
) -> SmallVec<[Range; 1]> {
    let mut ranges = SmallVec::new();
    let mut pos = range.from();

    while pos < range.to() {
        let object_range = textobject::textobject_word(text, Range::point(pos), objtype, 1, long)
            .with_direction(range.direction());

        if !object_range.is_empty() && range.contains_range(&object_range) {
            ranges.push(object_range);
            pos = object_range.to();
        } else {
            pos = next_grapheme_boundary(text, pos);
        }
    }

    ranges
}

fn select_all_paragraph_textobject_ranges(
    text: RopeSlice,
    range: Range,
    objtype: textobject::TextObject,
) -> SmallVec<[Range; 1]> {
    let mut ranges = SmallVec::new();
    let mut pos = range.from();

    while pos < range.to() {
        let object_range = textobject::textobject_paragraph(text, Range::point(pos), objtype, 1)
            .with_direction(range.direction());

        if !object_range.is_empty() && range.contains_range(&object_range) {
            ranges.push(object_range);
            pos = object_range.to();
        } else {
            pos = next_grapheme_boundary(text, pos);
        }
    }

    ranges
}

fn select_all_textobjects_ranges(
    editor: &mut Editor,
    objtype: textobject::TextObject,
    ch: char,
) -> Result<Option<Selection>, ()> {
    let (view, doc) = current!(editor);
    let loader = editor.syn_loader.load();
    let text = doc.text().slice(..);
    let selection = doc.selection(view.id).clone();
    let mut ranges = SmallVec::new();

    for range in selection {
        match ch {
            'w' => ranges.extend(select_all_word_textobject_ranges(
                text, range, objtype, false,
            )),
            'W' => ranges.extend(select_all_word_textobject_ranges(
                text, range, objtype, true,
            )),
            'p' => ranges.extend(select_all_paragraph_textobject_ranges(text, range, objtype)),
            't' => ranges.extend(select_all_treesitter_textobject_ranges(
                text,
                range,
                objtype,
                "class",
                doc.syntax(),
                &loader,
            )),
            'f' => ranges.extend(select_all_treesitter_textobject_ranges(
                text,
                range,
                objtype,
                "function",
                doc.syntax(),
                &loader,
            )),
            'a' => ranges.extend(select_all_treesitter_textobject_ranges(
                text,
                range,
                objtype,
                "parameter",
                doc.syntax(),
                &loader,
            )),
            'c' => ranges.extend(select_all_treesitter_textobject_ranges(
                text,
                range,
                objtype,
                "comment",
                doc.syntax(),
                &loader,
            )),
            'T' => ranges.extend(select_all_treesitter_textobject_ranges(
                text,
                range,
                objtype,
                "test",
                doc.syntax(),
                &loader,
            )),
            'e' => ranges.extend(select_all_treesitter_textobject_ranges(
                text,
                range,
                objtype,
                "entry",
                doc.syntax(),
                &loader,
            )),
            'x' => ranges.extend(select_all_treesitter_textobject_ranges(
                text,
                range,
                objtype,
                "xml-element",
                doc.syntax(),
                &loader,
            )),
            'g' => {
                let Some(diff_handle) = doc.diff_handle() else {
                    editor.set_status("Diff is not available in current buffer");
                    return Err(());
                };
                let diff = diff_handle.load();
                ranges.extend((0..diff.len()).filter_map(|idx| {
                    let hunk_range =
                        hunk_range(diff.nth_hunk(idx), text).with_direction(range.direction());
                    range.contains_range(&hunk_range).then_some(hunk_range)
                }));
            }
            'd' => {
                ranges.extend(doc.diagnostics().iter().filter_map(|diagnostic| {
                    let diagnostic_range = Range::new(diagnostic.range.start, diagnostic.range.end)
                        .with_direction(range.direction());
                    range
                        .contains_range(&diagnostic_range)
                        .then_some(diagnostic_range)
                }));
            }
            's' => ranges.extend(
                spelling_ranges(doc)
                    .filter(|finding| range.contains_range(finding))
                    .map(|finding| finding.with_direction(range.direction())),
            ),
            _ => {}
        }
    }

    Ok((!ranges.is_empty()).then(|| Selection::new(ranges, 0)))
}

fn select_all_textobjects(cx: &mut Context, objtype: textobject::TextObject) {
    cx.on_next_key(move |cx, event| {
        cx.editor.autoinfo = None;
        let Some(ch) = event.char() else {
            return;
        };

        match select_all_textobjects_ranges(cx.editor, objtype, ch) {
            Ok(Some(selection)) => {
                let (view, doc) = current!(cx.editor);
                doc.set_selection(view.id, selection);
            }
            Ok(None) => cx.editor.set_error(|| "nothing selected"),
            Err(()) => {}
        }
    });

    let title = match objtype {
        textobject::TextObject::Inside => "Select all inside",
        textobject::TextObject::Around => "Select all around",
        _ => return,
    };
    let help_text = [
        ("w", "Word"),
        ("W", "WORD"),
        ("p", "Paragraph"),
        ("t", "Type definition (tree-sitter)"),
        ("f", "Function (tree-sitter)"),
        ("a", "Argument/parameter (tree-sitter)"),
        ("c", "Comment (tree-sitter)"),
        ("T", "Test (tree-sitter)"),
        ("e", "Data structure entry (tree-sitter)"),
        ("g", "Change"),
        ("d", "Diagnostic"),
        ("s", "Spelling finding"),
        ("x", "(X)HTML element (tree-sitter)"),
    ];
    cx.editor.autoinfo = Some(Info::new(title, &help_text));
}

pub(super) fn select_all_textobjects_around(cx: &mut Context) {
    select_all_textobjects(cx, textobject::TextObject::Around);
}

pub(super) fn select_all_textobjects_inner(cx: &mut Context) {
    select_all_textobjects(cx, textobject::TextObject::Inside);
}

fn select_textobject(cx: &mut Context, objtype: textobject::TextObject) {
    let count = cx.count();

    cx.on_next_key(move |cx, event| {
        cx.editor.autoinfo = None;
        if let Some(ch) = event.char() {
            let textobject = move |editor: &mut Editor| {
                let (view, doc) = current!(editor);
                let loader = editor.syn_loader.load();
                let text = doc.text().slice(..);

                let textobject_treesitter = |obj_name: &str, range: Range| -> Range {
                    let Some(syntax) = doc.syntax() else {
                        return range;
                    };
                    textobject::textobject_treesitter(
                        text, range, objtype, obj_name, syntax, &loader, count,
                    )
                };

                if ch == 'g' && doc.diff_handle().is_none() {
                    editor.set_status("Diff is not available in current buffer");
                    return;
                }

                let textobject_change = |range: Range| -> Range {
                    let diff_handle = doc.diff_handle().unwrap();
                    let diff = diff_handle.load();
                    let line = range.cursor_line(text);
                    let hunk_idx = if let Some(hunk_idx) = diff.hunk_at(line as u32, false) {
                        hunk_idx
                    } else {
                        return range;
                    };
                    let hunk = diff.nth_hunk(hunk_idx).after;

                    let start = text.line_to_char(hunk.start as usize);
                    let end = text.line_to_char(hunk.end as usize);
                    Range::new(start, end).with_direction(range.direction())
                };

                let selection = doc.selection(view.id).clone().transform(|range| {
                    match ch {
                        'w' => textobject::textobject_word(text, range, objtype, count, false),
                        'W' => textobject::textobject_word(text, range, objtype, count, true),
                        't' => textobject_treesitter("class", range),
                        'f' => textobject_treesitter("function", range),
                        'a' => textobject_treesitter("parameter", range),
                        'c' => textobject_treesitter("comment", range),
                        'T' => textobject_treesitter("test", range),
                        'e' => textobject_treesitter("entry", range),
                        'x' => textobject_treesitter("xml-element", range),
                        'p' => textobject::textobject_paragraph(text, range, objtype, count),
                        'm' => textobject::textobject_pair_surround_closest(
                            doc.syntax(),
                            text,
                            range,
                            objtype,
                            count,
                        ),
                        'g' => textobject_change(range),
                        's' => spelling_ranges(doc)
                            .find(|finding| finding.contains(range.cursor(text)))
                            .map_or(range, |finding| finding.with_direction(range.direction())),
                        // TODO: cancel new ranges if inconsistent surround matches across lines
                        ch if !ch.is_ascii_alphanumeric() => textobject::textobject_pair_surround(
                            doc.syntax(),
                            text,
                            range,
                            objtype,
                            ch,
                            count,
                        ),
                        _ => range,
                    }
                });
                doc.set_selection(view.id, selection);
            };
            cx.editor.apply_motion(textobject);
        }
    });

    let title = match objtype {
        textobject::TextObject::Inside => "Match inside",
        textobject::TextObject::Around => "Match around",
        _ => return,
    };
    let help_text = [
        ("w", "Word"),
        ("W", "WORD"),
        ("p", "Paragraph"),
        ("t", "Type definition (tree-sitter)"),
        ("f", "Function (tree-sitter)"),
        ("a", "Argument/parameter (tree-sitter)"),
        ("c", "Comment (tree-sitter)"),
        ("T", "Test (tree-sitter)"),
        ("e", "Data structure entry (tree-sitter)"),
        ("m", "Closest surrounding pair (tree-sitter)"),
        ("g", "Change"),
        ("s", "Spelling finding"),
        ("x", "(X)HTML element (tree-sitter)"),
        (" ", "... or any character acting as a pair"),
    ];

    cx.editor.autoinfo = Some(Info::new(title, &help_text));
}
