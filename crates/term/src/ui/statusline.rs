use super::icon_span;
use editor_core::indent::IndentStyle;
use editor_core::{coords_at_pos, encoding, Position};
use lsp_client::lsp::DiagnosticSeverity;
use view::document::DEFAULT_LANGUAGE_NAME;
use view::graphics::RectExt as _;
use view::{
    document::{Mode, SCRATCH_BUFFER_NAME},
    graphics::Rect,
    icons::ICONS,
    theme::Style,
    Document, Editor, View,
};

use crate::ui::ProgressSpinners;
use std::{collections::HashSet, path::Path};

use tui::buffer::Buffer as Surface;
use tui::layout::{Constraint, Layout};
use tui::text::{Line, Span};
use tui::widgets::Widget;
use view::config::StatusLineElement as StatusLineElementID;

pub struct RenderContext<'a> {
    pub editor: &'a Editor,
    pub doc: &'a Document,
    pub view: &'a View,
    pub spinners: &'a ProgressSpinners,
    pub parts: RenderBuffer<'a>,
}

impl<'a> RenderContext<'a> {
    pub fn new(
        editor: &'a Editor,
        doc: &'a Document,
        view: &'a View,
        spinners: &'a ProgressSpinners,
    ) -> Self {
        RenderContext {
            editor,
            doc,
            view,
            spinners,
            parts: RenderBuffer::default(),
        }
    }
}

#[derive(Default)]
pub struct RenderBuffer<'a> {
    pub left: Line<'a>,
    pub right: Line<'a>,
}

pub fn render(context: &mut RenderContext, viewport: Rect, surface: &mut Surface) {
    let base_style = context.editor.theme.get("ui.statusline");

    surface.set_style(viewport.with_height(1), base_style);

    // Left side of the status line.

    let config = context.editor.config();

    for element_id in &config.statusline.left {
        let render = get_render_function(*element_id);
        (render)(context, |context, span| {
            append(&mut context.parts.left, span, base_style)
        });
    }

    // Right side of the status line.

    for element_id in &config.statusline.right {
        let render = get_render_function(*element_id);
        (render)(context, |context, span| {
            append(&mut context.parts.right, span, base_style)
        })
    }

    let [left_area, right_area] = statusline_areas(viewport, context.parts.right.width() as u16);

    std::mem::take(&mut context.parts.left).render(left_area, surface);
    std::mem::take(&mut context.parts.right)
        .right_aligned()
        .render(right_area, surface);
}

fn statusline_areas(viewport: Rect, right_width: u16) -> [Rect; 2] {
    Layout::horizontal([Constraint::Min(0), Constraint::Length(right_width)]).areas(viewport)
}

fn append<'a>(buffer: &mut Line<'a>, mut span: Span<'a>, base_style: Style) {
    span.style = tui::style::Style::from(base_style).patch(span.style);
    buffer.spans.push(span);
}

fn get_render_function<'a, F>(element_id: StatusLineElementID) -> impl Fn(&mut RenderContext<'a>, F)
where
    F: Fn(&mut RenderContext<'a>, Span<'a>) + Copy,
{
    match element_id {
        StatusLineElementID::Mode => render_mode,
        StatusLineElementID::Spinner => render_lsp_spinner,
        StatusLineElementID::FileBaseName => render_file_base_name,
        StatusLineElementID::FileName => render_file_name,
        StatusLineElementID::FileAbsolutePath => render_file_absolute_path,
        StatusLineElementID::FileModificationIndicator => render_file_modification_indicator,
        StatusLineElementID::ReadOnlyIndicator => render_read_only_indicator,
        StatusLineElementID::FileEncoding => render_file_encoding,
        StatusLineElementID::FileLineEnding => render_file_line_ending,
        StatusLineElementID::FileIndentStyle => render_file_indent_style,
        StatusLineElementID::FileType => render_file_type,
        StatusLineElementID::Diagnostics => render_diagnostics,
        StatusLineElementID::WorkspaceDiagnostics => render_workspace_diagnostics,
        StatusLineElementID::Selections => render_selections,
        StatusLineElementID::PrimarySelectionLength => render_primary_selection_length,
        StatusLineElementID::Position => render_position,
        StatusLineElementID::PositionPercentage => render_position_percentage,
        StatusLineElementID::TotalLineNumbers => render_total_line_numbers,
        StatusLineElementID::Separator => render_separator,
        StatusLineElementID::Spacer => render_spacer,
        StatusLineElementID::Branch => render_branch,
        StatusLineElementID::Register => render_register,
        StatusLineElementID::CurrentWorkingDirectory => render_cwd,
        StatusLineElementID::CodeActionHint => render_code_action_hint,
    }
}

fn render_mode<'a, F>(context: &mut RenderContext<'a>, write: F)
where
    F: Fn(&mut RenderContext<'a>, Span<'a>) + Copy,
{
    let config = context.editor.config();
    let modenames = &config.statusline.mode;
    let mode_str = match context.editor.mode() {
        Mode::Insert => &modenames.insert,
        Mode::Select => &modenames.select,
        Mode::Normal => &modenames.normal,
    };
    let content = format!(" {mode_str} ");
    let style = if config.color_modes {
        match context.editor.mode() {
            Mode::Insert => context.editor.theme.get("ui.statusline.insert"),
            Mode::Select => context.editor.theme.get("ui.statusline.select"),
            Mode::Normal => context.editor.theme.get("ui.statusline.normal"),
        }
    } else {
        Style::default()
    };
    write(context, Span::styled(content, style));
}

fn render_lsp_spinner<'a, F>(context: &mut RenderContext<'a>, write: F)
where
    F: Fn(&mut RenderContext<'a>, Span<'a>) + Copy,
{
    write(
        context,
        context
            .doc
            .language_servers()
            .find_map(|srv| {
                context
                    .spinners
                    .get(srv.id())
                    .and_then(|spinner| spinner.frame())
            })
            // Even if there's no spinner; reserve its space to avoid elements frequently shifting.
            .unwrap_or(" ")
            .into(),
    );
}

fn render_diagnostics<'a, F>(context: &mut RenderContext<'a>, write: F)
where
    F: Fn(&mut RenderContext<'a>, Span<'a>) + Copy,
{
    use editor_core::diagnostic::Severity;
    let (hints, info, warnings, errors) =
        context
            .doc
            .diagnostics()
            .iter()
            .fold((0, 0, 0, 0), |mut counts, diag| {
                match diag.severity {
                    Some(Severity::Hint) | None => counts.0 += 1,
                    Some(Severity::Info) => counts.1 += 1,
                    Some(Severity::Warning) => counts.2 += 1,
                    Some(Severity::Error) => counts.3 += 1,
                }
                counts
            });

    for sev in &context.editor.config().statusline.diagnostics {
        match sev {
            Severity::Hint if hints > 0 => {
                let glyph = context
                    .editor
                    .config()
                    .icons
                    .then(|| ICONS.load().diagnostic().hint().to_string());
                write(
                    context,
                    Span::styled(
                        glyph.unwrap_or_else(|| "●".to_string()),
                        context.editor.theme.get("hint"),
                    ),
                );
                write(context, format!(" {} ", hints).into());
            }
            Severity::Info if info > 0 => {
                let glyph = context
                    .editor
                    .config()
                    .icons
                    .then(|| ICONS.load().diagnostic().info().to_string());
                write(
                    context,
                    Span::styled(
                        glyph.unwrap_or_else(|| "●".to_string()),
                        context.editor.theme.get("info"),
                    ),
                );
                write(context, format!(" {} ", info).into());
            }
            Severity::Warning if warnings > 0 => {
                let glyph = context
                    .editor
                    .config()
                    .icons
                    .then(|| ICONS.load().diagnostic().warning().to_string());
                write(
                    context,
                    Span::styled(
                        glyph.unwrap_or_else(|| "●".to_string()),
                        context.editor.theme.get("warning"),
                    ),
                );
                write(context, format!(" {} ", warnings).into());
            }
            Severity::Error if errors > 0 => {
                let glyph = context
                    .editor
                    .config()
                    .icons
                    .then(|| ICONS.load().diagnostic().error().to_string());
                write(
                    context,
                    Span::styled(
                        glyph.unwrap_or_else(|| "●".to_string()),
                        context.editor.theme.get("error"),
                    ),
                );
                write(context, format!(" {} ", errors).into());
            }
            _ => {}
        }
    }
}

fn render_workspace_diagnostics<'a, F>(context: &mut RenderContext<'a>, write: F)
where
    F: Fn(&mut RenderContext<'a>, Span<'a>) + Copy,
{
    use editor_core::diagnostic::Severity;
    let editor = context.editor;
    let (mut hints, mut info, mut warnings, mut errors) = (0u32, 0u32, 0u32, 0u32);

    // Open documents carry diagnostics from every provider (spelling included), edit-mapped.
    for doc in editor.documents() {
        for diag in doc.diagnostics() {
            match diag.severity {
                Some(Severity::Warning) => warnings += 1,
                Some(Severity::Error) => errors += 1,
                Some(Severity::Info) => info += 1,
                Some(Severity::Hint) | None => hints += 1,
            }
        }
    }

    // The store additionally holds LSP diagnostics for files which are not currently open.
    let open_paths: HashSet<&Path> = editor.documents().filter_map(|doc| doc.path()).collect();
    for (uri, diags) in &editor.diagnostics {
        if uri.as_path().is_some_and(|path| open_paths.contains(path)) {
            continue;
        }
        for (diag, _) in diags {
            // PERF: For large workspace diagnostics, this loop can be very tight.
            //
            // Most often the diagnostics will be for warnings and errors.
            // Errors should tend to be fixed fast, leaving warnings as the most common.
            match diag.severity {
                Some(DiagnosticSeverity::WARNING) => warnings += 1,
                Some(DiagnosticSeverity::ERROR) => errors += 1,
                Some(DiagnosticSeverity::HINT) => hints += 1,
                Some(DiagnosticSeverity::INFORMATION) => info += 1,
                // Fallback to `hint`.
                _ => hints += 1,
            }
        }
    }

    let sevs_to_show = &context.editor.config().statusline.workspace_diagnostics;

    // Avoid showing the " W " if no diagnostic counts will be shown.
    if !sevs_to_show.iter().any(|sev| match sev {
        Severity::Hint => hints != 0,
        Severity::Info => info != 0,
        Severity::Warning => warnings != 0,
        Severity::Error => errors != 0,
    }) {
        return;
    }

    write(context, " W ".into());

    for sev in sevs_to_show {
        match sev {
            Severity::Hint if hints > 0 => {
                let glyph = context
                    .editor
                    .config()
                    .icons
                    .then(|| ICONS.load().diagnostic().hint().to_string());
                write(
                    context,
                    Span::styled(
                        glyph.unwrap_or_else(|| "●".to_string()),
                        context.editor.theme.get("hint"),
                    ),
                );
                write(context, format!(" {} ", hints).into());
            }
            Severity::Info if info > 0 => {
                let glyph = context
                    .editor
                    .config()
                    .icons
                    .then(|| ICONS.load().diagnostic().info().to_string());
                write(
                    context,
                    Span::styled(
                        glyph.unwrap_or_else(|| "●".to_string()),
                        context.editor.theme.get("info"),
                    ),
                );
                write(context, format!(" {} ", info).into());
            }
            Severity::Warning if warnings > 0 => {
                let glyph = context
                    .editor
                    .config()
                    .icons
                    .then(|| ICONS.load().diagnostic().warning().to_string());
                write(
                    context,
                    Span::styled(
                        glyph.unwrap_or_else(|| "●".to_string()),
                        context.editor.theme.get("warning"),
                    ),
                );
                write(context, format!(" {} ", warnings).into());
            }
            Severity::Error if errors > 0 => {
                let glyph = context
                    .editor
                    .config()
                    .icons
                    .then(|| ICONS.load().diagnostic().error().to_string());
                write(
                    context,
                    Span::styled(
                        glyph.unwrap_or_else(|| "●".to_string()),
                        context.editor.theme.get("error"),
                    ),
                );
                write(context, format!(" {} ", errors).into());
            }
            _ => {}
        }
    }
}

fn render_selections<'a, F>(context: &mut RenderContext<'a>, write: F)
where
    F: Fn(&mut RenderContext<'a>, Span<'a>) + Copy,
{
    let selection = context.doc.selection(context.view.id);
    let count = selection.len();
    write(
        context,
        if count == 1 {
            " 1 sel ".into()
        } else {
            format!(" {}/{count} sels ", selection.primary_index() + 1).into()
        },
    );
}

fn render_primary_selection_length<'a, F>(context: &mut RenderContext<'a>, write: F)
where
    F: Fn(&mut RenderContext<'a>, Span<'a>) + Copy,
{
    let tot_sel = context.doc.selection(context.view.id).primary().len();
    write(
        context,
        format!(" {} char{} ", tot_sel, if tot_sel == 1 { "" } else { "s" }).into(),
    );
}

fn get_position(context: &RenderContext) -> Position {
    coords_at_pos(
        context.doc.text().slice(..),
        context
            .doc
            .selection(context.view.id)
            .primary()
            .cursor(context.doc.text().slice(..)),
    )
}

fn render_position<'a, F>(context: &mut RenderContext<'a>, write: F)
where
    F: Fn(&mut RenderContext<'a>, Span<'a>) + Copy,
{
    let position = get_position(context);
    write(
        context,
        format!(" {}:{} ", position.row + 1, position.col + 1).into(),
    );
}

fn render_total_line_numbers<'a, F>(context: &mut RenderContext<'a>, write: F)
where
    F: Fn(&mut RenderContext<'a>, Span<'a>) + Copy,
{
    let total_line_numbers = context.doc.text().len_lines();

    write(context, format!(" {} ", total_line_numbers).into());
}

fn render_position_percentage<'a, F>(context: &mut RenderContext<'a>, write: F)
where
    F: Fn(&mut RenderContext<'a>, Span<'a>) + Copy,
{
    let position = get_position(context);
    let maxrows = context.doc.text().len_lines();
    write(
        context,
        format!("{}%", (position.row + 1) * 100 / maxrows).into(),
    );
}

fn render_file_encoding<'a, F>(context: &mut RenderContext<'a>, write: F)
where
    F: Fn(&mut RenderContext<'a>, Span<'a>) + Copy,
{
    let enc = context.doc.encoding();

    if enc != encoding::UTF_8 {
        write(context, format!(" {} ", enc.name()).into());
    }
}

fn render_file_line_ending<'a, F>(context: &mut RenderContext<'a>, write: F)
where
    F: Fn(&mut RenderContext<'a>, Span<'a>) + Copy,
{
    use editor_core::LineEnding::*;
    let line_ending = match context.doc.line_ending {
        Crlf => "CRLF",
        LF => "LF",
        #[cfg(feature = "unicode-lines")]
        VT => "VT", // U+000B -- VerticalTab
        #[cfg(feature = "unicode-lines")]
        FF => "FF", // U+000C -- FormFeed
        #[cfg(feature = "unicode-lines")]
        CR => "CR", // U+000D -- CarriageReturn
        #[cfg(feature = "unicode-lines")]
        Nel => "NEL", // U+0085 -- NextLine
        #[cfg(feature = "unicode-lines")]
        LS => "LS", // U+2028 -- Line Separator
        #[cfg(feature = "unicode-lines")]
        PS => "PS", // U+2029 -- ParagraphSeparator
    };

    write(context, format!(" {} ", line_ending).into());
}

fn render_file_type<'a, F>(context: &mut RenderContext<'a>, write: F)
where
    F: Fn(&mut RenderContext<'a>, Span<'a>) + Copy,
{
    let file_type = context.doc.language_name().unwrap_or(DEFAULT_LANGUAGE_NAME);

    if context.editor.config().icons
        && let (Some(file), Some(path)) = (ICONS.load().fs().file(), context.doc.path())
    {
        write(
            context,
            icon_span(file.get_with_style_or_default(path, &context.editor.theme)),
        );
        write(context, format!("{file_type} ").into());
        return;
    }

    write(context, format!(" {} ", file_type).into());
}

fn render_file_name<'a, F>(context: &mut RenderContext<'a>, write: F)
where
    F: Fn(&mut RenderContext<'a>, Span<'a>) + Copy,
{
    let title = {
        let rel_path = context.doc.display_path();
        let path = rel_path
            .as_ref()
            .map(|p| p.to_string_lossy())
            .unwrap_or_else(|| SCRATCH_BUFFER_NAME.into());
        format!(" {} ", path)
    };

    write(context, title.into());
}

fn render_file_absolute_path<'a, F>(context: &mut RenderContext<'a>, write: F)
where
    F: Fn(&mut RenderContext<'a>, Span<'a>) + Copy,
{
    let title = {
        let path = context
            .doc
            .path()
            .as_ref()
            .map_or_else(|| SCRATCH_BUFFER_NAME.into(), |p| p.to_string_lossy());
        format!(" {} ", path)
    };

    write(context, title.into());
}

fn render_file_modification_indicator<'a, F>(context: &mut RenderContext<'a>, write: F)
where
    F: Fn(&mut RenderContext<'a>, Span<'a>) + Copy,
{
    let title = if context.doc.is_modified() {
        "[+]"
    } else {
        "   "
    };

    write(context, title.into());
}

fn render_read_only_indicator<'a, F>(context: &mut RenderContext<'a>, write: F)
where
    F: Fn(&mut RenderContext<'a>, Span<'a>) + Copy,
{
    let title = if context.doc.readonly {
        " [readonly] "
    } else {
        ""
    };
    write(context, title.into());
}

fn render_file_base_name<'a, F>(context: &mut RenderContext<'a>, write: F)
where
    F: Fn(&mut RenderContext<'a>, Span<'a>) + Copy,
{
    let title = {
        let rel_path = context.doc.display_path();
        let path = rel_path
            .as_ref()
            .and_then(|p| p.file_name().map(|s| s.to_string_lossy()))
            .unwrap_or_else(|| SCRATCH_BUFFER_NAME.into());
        format!(" {} ", path)
    };

    write(context, title.into());
}

fn render_separator<'a, F>(context: &mut RenderContext<'a>, write: F)
where
    F: Fn(&mut RenderContext<'a>, Span<'a>) + Copy,
{
    let sep = &context.editor.config().statusline.separator;
    let style = context.editor.theme.get("ui.statusline.separator");

    write(context, Span::styled(sep.to_string(), style));
}

fn render_spacer<'a, F>(context: &mut RenderContext<'a>, write: F)
where
    F: Fn(&mut RenderContext<'a>, Span<'a>) + Copy,
{
    write(context, " ".into());
}

fn render_branch<'a, F>(context: &mut RenderContext<'a>, write: F)
where
    F: Fn(&mut RenderContext<'a>, Span<'a>) + Copy,
{
    let head = context
        .doc
        .version_control_head()
        .unwrap_or_default()
        .to_string();

    if context.editor.config().icons
        && !head.is_empty()
        && let Some(icon) = ICONS.load().vcs().branch()
    {
        write(context, icon_span(icon));
        write(context, format!("{head} ").into());
        return;
    }

    write(context, head.into());
}

fn render_register<'a, F>(context: &mut RenderContext<'a>, write: F)
where
    F: Fn(&mut RenderContext<'a>, Span<'a>) + Copy,
{
    if let Some(reg) = context.editor.selected_register {
        write(context, format!(" reg={} ", reg).into())
    }
}

fn render_file_indent_style<'a, F>(context: &mut RenderContext<'a>, write: F)
where
    F: Fn(&mut RenderContext<'a>, Span<'a>) + Copy,
{
    let style = context.doc.indent_style;

    write(
        context,
        match style {
            IndentStyle::Tabs => " tabs ".into(),
            IndentStyle::Spaces(indent) => {
                format!(" {} space{} ", indent, if indent == 1 { "" } else { "s" }).into()
            }
        },
    );
}

fn render_cwd<'a, F>(context: &mut RenderContext<'a>, write: F)
where
    F: Fn(&mut RenderContext<'a>, Span<'a>) + Copy,
{
    let cwd = stdx::env::current_working_dir();
    let cwd = cwd
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();
    write(context, cwd.into())
}

fn render_code_action_hint<'a, F>(context: &mut RenderContext<'a>, write: F)
where
    F: Fn(&mut RenderContext<'a>, Span<'a>) + Copy,
{
    if context.doc.code_action_hints(context.view.id) {
        write(context, " ⋮ ".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn left_section_fills_space_not_needed_by_the_right_section() {
        let viewport = Rect::new(0, 0, 80, 1);

        let [left, right] = statusline_areas(viewport, 12);

        assert_eq!(left, Rect::new(0, 0, 68, 1));
        assert_eq!(right, Rect::new(68, 0, 12, 1));
    }

    #[test]
    fn right_section_keeps_priority_when_it_exceeds_the_viewport() {
        let viewport = Rect::new(4, 2, 60, 1);

        let [left, right] = statusline_areas(viewport, 80);

        assert_eq!(left, Rect::new(4, 2, 0, 1));
        assert_eq!(right, Rect::new(4, 2, 60, 1));
    }
}
