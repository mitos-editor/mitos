//! Terminal gutter glyphs, styles, and marker precedence.

use std::fmt::Write;
use stdx::rope::RopeExt as _;
use view::gutter::width;

use editor_config::GutterType;
use language_config::LanguageServerFeature;
use theme::icons::ICONS;
use theme::Theme;
use view::{
    graphics::{Style, StyleExt as _, UnderlineStyle},
    Document, Editor, View,
};

type GutterFn<'doc> = Box<dyn FnMut(usize, bool, bool, &mut String) -> Option<Style> + 'doc>;
pub(super) fn style<'doc>(
    gutter: GutterType,
    editor: &'doc Editor,
    doc: &'doc Document,
    view: &View,
    theme: &Theme,
    is_focused: bool,
) -> GutterFn<'doc> {
    match gutter {
        GutterType::Diagnostics => diagnostics_or_breakpoints(editor, doc, view, theme, is_focused),
        GutterType::LineNumbers => line_numbers(editor, doc, view, theme, is_focused),
        GutterType::Spacer => padding(editor, doc, view, theme, is_focused),
        GutterType::Diff => diff(editor, doc, view, theme, is_focused),
        GutterType::CodeActionHint => code_action_hint(editor, doc, view, theme, is_focused),
    }
}

fn diagnostic<'doc>(
    editor: &'doc Editor,
    doc: &'doc Document,
    _view: &View,
    theme: &Theme,
    _is_focused: bool,
) -> GutterFn<'doc> {
    let warning = theme.get("warning");
    let error = theme.get("error");
    let info = theme.get("info");
    let hint = theme.get("hint");
    let diagnostics = doc.diagnostics();

    Box::new(
        move |line: usize, _selected: bool, first_visual_line: bool, out: &mut String| {
            if !first_visual_line {
                return None;
            }
            use ::diagnostics::Severity;
            let first_diag_idx_maybe_on_line = diagnostics.partition_point(|d| d.line < line);
            let diagnostics_on_line = diagnostics[first_diag_idx_maybe_on_line..]
                .iter()
                .take_while(|d| {
                    d.line == line
                        && d.provider.language_server_id().is_none_or(|id| {
                            doc.language_servers_with_feature(LanguageServerFeature::Diagnostics)
                                .any(|ls| ls.id() == id)
                        })
                });
            diagnostics_on_line.max_by_key(|d| d.severity).map(|d| {
                let icon = if editor.config().icons {
                    let icons = ICONS.load();
                    Some(match d.severity {
                        Some(Severity::Error) => icons.diagnostic().error(),
                        Some(Severity::Warning) | None => icons.diagnostic().warning(),
                        Some(Severity::Info) => icons.diagnostic().info(),
                        Some(Severity::Hint) => icons.diagnostic().hint(),
                    })
                } else {
                    None
                };
                write!(
                    out,
                    "{}",
                    icon.map_or_else(
                        || {
                            let icons = ICONS.load();
                            match d.severity {
                                Some(Severity::Error) => icons.diagnostic().error(),
                                Some(Severity::Info) => icons.diagnostic().info(),
                                Some(Severity::Hint) => icons.diagnostic().hint(),
                                _ => icons.diagnostic().warning(),
                            }
                            .with_no_padding()
                            .to_string()
                        },
                        |icon| icon.with_no_padding().to_string()
                    )
                )
                .ok();
                match d.severity {
                    Some(Severity::Error) => error,
                    Some(Severity::Warning) | None => warning,
                    Some(Severity::Info) => info,
                    Some(Severity::Hint) => hint,
                }
            })
        },
    )
}

fn code_lens<'doc>(editor: &'doc Editor, doc: &'doc Document, theme: &Theme) -> GutterFn<'doc> {
    let info = theme.get("info");
    let enabled = editor.config().lsp.enable && editor.config().lsp.code_lenses.enable;
    Box::new(move |line, _, first_visual_line, out| {
        if !enabled || !first_visual_line {
            return None;
        }
        let start = doc.text().try_line_to_char_idx(line).ok()?;
        let lenses = doc.code_lenses();
        let index = lenses.partition_point(|lens| lens.char_idx < start);
        if lenses
            .get(index)
            .is_some_and(|lens| doc.text().char_to_line_idx(lens.char_idx) == line)
        {
            out.push('▶');
            Some(info)
        } else {
            None
        }
    })
}

fn diff<'doc>(
    _editor: &'doc Editor,
    doc: &'doc Document,
    _view: &View,
    theme: &Theme,
    _is_focused: bool,
) -> GutterFn<'doc> {
    let added = theme.get("diff.plus.gutter");
    let deleted = theme.get("diff.minus.gutter");
    let modified = theme.get("diff.delta.gutter");
    if let Some(review) = doc.review() {
        return Box::new(move |line, _, first, out| {
            if !first {
                return None;
            }
            let (marker, style) = match review.row(line)?.kind {
                view::features::change_review::ReviewLineKind::Added => ("+", added),
                view::features::change_review::ReviewLineKind::Removed => ("−", deleted),
                _ => return None,
            };
            out.push_str(marker);
            Some(style)
        });
    }
    if let Some(diff_handle) = doc.diff_handle() {
        let hunks = diff_handle.load();
        let mut hunk_i = 0;
        let mut hunk = hunks.nth_hunk(hunk_i);
        Box::new(
            move |line: usize, _selected: bool, first_visual_line: bool, out: &mut String| {
                // truncating the line is fine here because we don't compute diffs
                // for files with more lines than i32::MAX anyways
                // we need to special case removals here
                // these technically do not have a range of lines to highlight (`hunk.after.start == hunk.after.end`).
                // However we still want to display these hunks correctly we must not yet skip to the next hunk here
                while hunk.after.end < line as u32
                    || !hunk.is_pure_removal() && line as u32 == hunk.after.end
                {
                    hunk_i += 1;
                    hunk = hunks.nth_hunk(hunk_i);
                }

                if hunk.after.start > line as u32 {
                    return None;
                }

                let (icon, style) = if hunk.is_pure_insertion() {
                    ("▍", added)
                } else if hunk.is_pure_removal() {
                    if !first_visual_line {
                        return None;
                    }
                    ("▔", deleted)
                } else {
                    ("▍", modified)
                };

                write!(out, "{}", icon).unwrap();
                Some(style)
            },
        )
    } else {
        Box::new(move |_, _, _, _| None)
    }
}

fn line_numbers<'doc>(
    editor: &'doc Editor,
    doc: &'doc Document,
    view: &View,
    theme: &Theme,
    is_focused: bool,
) -> GutterFn<'doc> {
    let text = doc.text().slice(..);
    let width = width(GutterType::LineNumbers, view, doc);

    if let Some(review) = doc.review() {
        let digits = review.review().line_number_width();
        let normal = theme.get("ui.linenr");
        let selected_style = theme.get("ui.linenr.selected");
        return Box::new(move |line, selected, first, out| {
            if !first {
                return None;
            }
            let row = review.row(line)?;
            let number = |value: Option<usize>| {
                value.map_or_else(
                    || " ".repeat(digits),
                    |value| format!("{:>digits$}", value + 1),
                )
            };
            write!(out, "{} {}", number(row.before), number(row.after)).unwrap();
            Some(if selected { selected_style } else { normal })
        });
    }

    let layout = view.layout(doc, None);
    let last_line_in_view = layout.last_document_line();

    // Whether to draw the line number for the last line of the
    // document or not.  We only draw it if it's not an empty line.
    let draw_last = text.line_to_byte_idx(last_line_in_view, stdx::rope::LINE_TYPE) < text.len();

    let linenr = theme.get("ui.linenr");
    let linenr_select = theme.get("ui.linenr.selected");

    let current_line = layout.block_line_at_pos(doc.selection(view.id).primary().cursor(text));

    let line_number = editor.config().line_number;
    let folds = doc.folded_ranges(view.id);
    let wrap_indicator = editor
        .config()
        .soft_wrap
        .wrap_indicator
        .as_ref()
        .map_or_else(
            || "↪".into(),
            |indicator| indicator.chars().take(width).collect::<String>(),
        );
    let mode = editor.mode();

    Box::new(
        move |line: usize, selected: bool, first_visual_line: bool, out: &mut String| {
            if line == last_line_in_view && !draw_last {
                write!(out, "{:>1$}", '~', width).unwrap();
                Some(linenr)
            } else {
                use editor_config::LineNumber;
                use view::document::Mode;

                let relative = line_number == LineNumber::Relative
                    && mode != Mode::Insert
                    && is_focused
                    && current_line != line;

                let display_num = if relative {
                    text_layout::movement::document_line_distance(text, current_line, line, folds)
                } else {
                    line + 1
                };

                let style = if selected && is_focused {
                    linenr_select
                } else {
                    linenr
                };

                if first_visual_line {
                    write!(out, "{:>1$}", display_num, width).unwrap();
                } else {
                    write!(out, "{:>1$}", wrap_indicator, width).unwrap();
                }

                Some(style)
            }
        },
    )
}

fn padding<'doc>(
    _editor: &'doc Editor,
    _doc: &'doc Document,
    _view: &View,
    _theme: &Theme,
    _is_focused: bool,
) -> GutterFn<'doc> {
    Box::new(|_line: usize, _selected: bool, _first_visual_line: bool, _out: &mut String| None)
}

fn breakpoints<'doc>(
    editor: &'doc Editor,
    doc: &'doc Document,
    _view: &View,
    theme: &Theme,
    _is_focused: bool,
) -> GutterFn<'doc> {
    let error = theme.get("error");
    let info = theme.get("info");
    let breakpoint_style = theme.get("ui.debug.breakpoint");

    let breakpoints = editor
        .debugger_source(doc.id())
        .map(|source| editor.debugger_breakpoint_locations(&source))
        .unwrap_or_default();

    Box::new(
        move |line: usize, _selected: bool, first_visual_line: bool, out: &mut String| {
            if !first_visual_line {
                return None;
            }
            let breakpoint = breakpoints
                .iter()
                .find(|breakpoint| breakpoint.line == line)?;

            let conditional = breakpoint
                .definition
                .as_ref()
                .is_some_and(|definition| definition.condition.is_some());
            let logpoint = breakpoint
                .definition
                .as_ref()
                .is_some_and(|definition| definition.log_message.is_some());
            let style = match (conditional, logpoint) {
                (true, true) => error.underline_style(UnderlineStyle::Line),
                (true, false) => error,
                (false, true) => info,
                (false, false) => breakpoint_style,
            };

            let sym = if breakpoint.verified { "●" } else { "◯" };
            write!(out, "{}", sym).unwrap();
            Some(style)
        },
    )
}

fn execution_pause_indicator<'doc>(
    editor: &'doc Editor,
    doc: &'doc Document,
    theme: &Theme,
    is_focused: bool,
) -> GutterFn<'doc> {
    let style = theme.get("ui.debug.active");
    let frame_line = editor
        .debugger_frame_in_document(doc)
        .and_then(|frame| view::debugger::source::line_index(frame.line));

    Box::new(
        move |line: usize, _selected: bool, first_visual_line: bool, out: &mut String| {
            if !first_visual_line || !is_focused || line != frame_line? {
                return None;
            }

            let sym = "▶";
            write!(out, "{}", sym).unwrap();
            Some(style)
        },
    )
}

fn diagnostics_or_breakpoints<'doc>(
    editor: &'doc Editor,
    doc: &'doc Document,
    view: &View,
    theme: &Theme,
    is_focused: bool,
) -> GutterFn<'doc> {
    let mut diagnostics = diagnostic(editor, doc, view, theme, is_focused);
    let mut breakpoints = breakpoints(editor, doc, view, theme, is_focused);
    let mut execution_pause_indicator = execution_pause_indicator(editor, doc, theme, is_focused);
    let mut lenses = code_lens(editor, doc, theme);

    Box::new(move |line, selected, first_visual_line: bool, out| {
        execution_pause_indicator(line, selected, first_visual_line, out)
            .or_else(|| breakpoints(line, selected, first_visual_line, out))
            .or_else(|| diagnostics(line, selected, first_visual_line, out))
            .or_else(|| lenses(line, selected, first_visual_line, out))
    })
}

fn code_action_hint<'doc>(
    _editor: &'doc Editor,
    doc: &'doc Document,
    view: &View,
    theme: &Theme,
    is_focused: bool,
) -> GutterFn<'doc> {
    let style = theme.get("ui.text");
    let text = doc.text().slice(..);
    let show_hint = doc.code_action_hints(view.id);
    let current_line = view
        .layout(doc, None)
        .block_line_at_pos(doc.selection(view.id).primary().cursor(text));

    Box::new(
        move |line: usize, _selected: bool, first_visual_line: bool, out: &mut String| {
            (is_focused && show_hint && current_line == line && first_visual_line).then(|| {
                write!(out, "⋮").unwrap();
                style
            })
        },
    )
}
