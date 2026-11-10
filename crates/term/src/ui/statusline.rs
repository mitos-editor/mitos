use super::icon_span;
use editor_core::indent::IndentStyle;
use editor_core::search::statistics::Count;
use editor_core::{coords_at_pos, encoding, Position};

use lsp_client::lsp::DiagnosticSeverity;
use theme::icons::ICONS;
use theme::Style;
use view::document::DEFAULT_LANGUAGE_NAME;
use view::graphics::RectExt as _;
use view::{document::Mode, graphics::Rect, Document, Editor, View};

use crate::ui::ProgressSpinners;
use std::{collections::HashSet, path::Path};

use editor_config::StatusLineElement as StatusLineElementID;
use tui::buffer::Buffer as Surface;
use tui::layout::{Constraint, Layout};
use tui::text::{truncate_line, Line, Span};
use tui::widgets::Widget;

pub struct RenderContext<'a> {
    pub editor: &'a Editor,
    pub selected_register: Option<char>,
    pub doc: &'a Document,
    pub view: &'a View,
    pub spinners: &'a ProgressSpinners,
    parts: Line<'a>,
}

impl<'a> RenderContext<'a> {
    pub fn new(
        editor: &'a Editor,
        doc: &'a Document,
        view: &'a View,
        spinners: &'a ProgressSpinners,
        selected_register: Option<char>,
    ) -> Self {
        RenderContext {
            editor,
            selected_register,
            doc,
            view,
            spinners,
            parts: Line::default(),
        }
    }

    fn is_focused(&self) -> bool {
        self.view.id == self.editor.tree.focus()
    }
}

struct Part<'a> {
    element: StatusLineElementID,
    line: Line<'a>,
    compact: Option<Line<'a>>,
}

pub fn render(context: &mut RenderContext, viewport: Rect, surface: &mut Surface) {
    if viewport.is_empty() {
        return;
    }

    let base_style = context.editor.theme.get(if context.is_focused() {
        "ui.statusline"
    } else {
        "ui.statusline.inactive"
    });
    surface.set_style(viewport.with_height(1), base_style);

    let config = context.editor.config();
    let mut left = render_parts(context, &config.statusline.left, base_style);
    let mut right = render_parts(context, &config.statusline.right, base_style);
    fit_parts(&mut left, &mut right, viewport.width as usize);

    let left = join_parts(left);
    let right = join_parts(right);
    let [left_area, right_area] =
        statusline_areas(viewport, right.width().min(viewport.width as usize) as u16);
    left.render(left_area, surface);
    right.right_aligned().render(right_area, surface);
}

fn render_parts<'a>(
    context: &mut RenderContext<'a>,
    elements: &[StatusLineElementID],
    base_style: Style,
) -> Vec<Part<'a>> {
    elements
        .iter()
        .map(|&element| {
            let render = get_render_function(element);
            render(context, |context, span| {
                append(&mut context.parts, span, base_style)
            });
            let line = std::mem::take(&mut context.parts);
            let compact = if is_filename(element) {
                let mut compact = Line::from(Span::styled(
                    format!(" {} ", filename(context, FileNameFormat::Base)),
                    base_style,
                ));
                if context.doc.review().is_some() {
                    compact.spans.push(Span::styled("DIFF ", base_style));
                }
                Some(compact)
            } else {
                None
            };
            Part {
                element,
                line,
                compact,
            }
        })
        .collect()
}

fn join_parts(parts: Vec<Part<'_>>) -> Line<'_> {
    Line::from(
        parts
            .into_iter()
            .flat_map(|part| part.line.spans)
            .collect::<Vec<_>>(),
    )
}

fn is_filename(element: StatusLineElementID) -> bool {
    use StatusLineElementID as E;
    matches!(element, E::FileName | E::FileAbsolutePath | E::FileBaseName)
}

fn parts_width(left: &[Part<'_>], right: &[Part<'_>]) -> usize {
    left.iter().chain(right).map(|part| part.line.width()).sum()
}

fn fit_parts<'a>(left: &mut [Part<'a>], right: &mut [Part<'a>], width: usize) {
    use StatusLineElementID as E;
    let mut excess = parts_width(left, right).saturating_sub(width);
    if excess == 0 {
        return;
    }

    // Language is useful at full width, but is the first field to give up space.
    for part in left.iter_mut().chain(right.iter_mut()) {
        if part.element == E::FileType && excess > 0 {
            excess = excess.saturating_sub(part.line.width());
            part.line.spans.clear();
        }
    }

    // Shorten paths and context before hiding any other fields.
    for part in left.iter_mut().chain(right.iter_mut()) {
        if excess == 0 {
            return;
        }
        if let Some(compact) = part.compact.take() {
            let saved = part.line.width().saturating_sub(compact.width());
            if saved > 0 {
                part.line = compact;
                excess = excess.saturating_sub(saved);
            }
        }
    }
    for part in left.iter_mut().chain(right.iter_mut()) {
        let minimum = match part.element {
            E::Branch => 8,
            E::DebugSession => 16,
            _ => continue,
        };
        shrink_part(part, &mut excess, minimum, false);
    }

    // Keep custom ordering and sides. Only visibility changes as space runs out.
    for element in [
        E::DebugSession,
        E::Branch,
        E::CurrentWorkingDirectory,
        E::FileIndentStyle,
        E::FileLineEnding,
        E::FileEncoding,
        E::TotalLineNumbers,
        E::PositionPercentage,
        E::PrimarySelectionLength,
        E::WorkspaceDiagnostics,
        E::CodeActionHint,
        E::Spinner,
        E::Separator,
        E::Spacer,
        E::Diagnostics,
        E::SearchPosition,
        E::Register,
        E::Selections,
    ] {
        for part in left.iter_mut().chain(right.iter_mut()) {
            if excess == 0 {
                return;
            }
            if part.element == element {
                excess = excess.saturating_sub(part.line.width());
                part.line.spans.clear();
            }
        }
    }

    // If the basename itself is too long, preserve its suffix and file extension.
    for part in left.iter_mut().chain(right.iter_mut()) {
        if is_filename(part.element) && excess > 0 {
            let old_width = part.line.width();
            part.line.spans.truncate(1);
            excess = excess.saturating_sub(old_width - part.line.width());
            shrink_part(part, &mut excess, 3, true);
        }
    }
}

fn shrink_part(part: &mut Part<'_>, excess: &mut usize, minimum: usize, from_start: bool) {
    let old_width = part.line.width();
    let width = old_width.saturating_sub(*excess).max(minimum);
    if width >= old_width {
        return;
    }
    truncate_line(&mut part.line, width, from_start);
    *excess = excess.saturating_sub(old_width - part.line.width());
}

fn statusline_areas(viewport: Rect, right_width: u16) -> [Rect; 2] {
    Layout::horizontal([Constraint::Min(0), Constraint::Length(right_width)]).areas(viewport)
}

fn append<'a>(buffer: &mut Line<'a>, mut span: Span<'a>, base_style: Style) {
    span.style = base_style.patch(span.style);
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
        StatusLineElementID::SearchPosition => render_search_position,
        StatusLineElementID::CurrentWorkingDirectory => render_cwd,
        StatusLineElementID::CodeActionHint => render_code_action_hint,
        StatusLineElementID::DebugSession => render_debug_session,
    }
}

fn render_debug_session<'a, F>(context: &mut RenderContext<'a>, write: F)
where
    F: Fn(&mut RenderContext<'a>, Span<'a>) + Copy,
{
    if !context.is_focused() {
        return;
    }
    let debugger = context.editor.debugger();
    let Some(session) = debugger.get_active_session() else {
        return;
    };
    let count = debugger.sessions().len();
    let content = format!(
        " Debug #{} {} ({}, {} {}) ",
        session.number(),
        session.name(),
        session.status(),
        count,
        if count == 1 { "session" } else { "sessions" }
    );
    write(context, Span::raw(content));
}

fn render_mode<'a, F>(context: &mut RenderContext<'a>, write: F)
where
    F: Fn(&mut RenderContext<'a>, Span<'a>) + Copy,
{
    if !context.is_focused() {
        return;
    }

    let config = context.editor.config();
    let modenames = &config.statusline.mode;
    let mode_str = match context.editor.mode() {
        Mode::Insert => &modenames.insert,
        Mode::Select => &modenames.select,
        Mode::Normal => &modenames.normal,
    };
    let content = format!(" {mode_str} ");
    let style = if config.statusline.mode.color {
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
    use ::diagnostics::Severity;
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

    render_diagnostic_counts(
        context,
        (hints, info, warnings, errors),
        &context.editor.config().statusline.diagnostics,
        "",
        write,
    );
}

fn render_workspace_diagnostics<'a, F>(context: &mut RenderContext<'a>, write: F)
where
    F: Fn(&mut RenderContext<'a>, Span<'a>) + Copy,
{
    use ::diagnostics::Severity;
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
    for (uri, diags) in editor.diagnostics() {
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

    render_diagnostic_counts(
        context,
        (hints, info, warnings, errors),
        &context.editor.config().statusline.workspace_diagnostics,
        " W ",
        write,
    );
}

fn render_diagnostic_counts<'a, F>(
    context: &mut RenderContext<'a>,
    (hints, info, warnings, errors): (u32, u32, u32, u32),
    severities: &[::diagnostics::Severity],
    prefix: &str,
    write: F,
) where
    F: Fn(&mut RenderContext<'a>, Span<'a>) + Copy,
{
    use ::diagnostics::Severity;
    let mut needs_prefix = !prefix.is_empty();
    for severity in severities {
        let (count, icon, style) = match severity {
            Severity::Hint => (hints, ICONS.diagnostic().hint(), "hint"),
            Severity::Info => (info, ICONS.diagnostic().info(), "info"),
            Severity::Warning => (warnings, ICONS.diagnostic().warning(), "warning"),
            Severity::Error => (errors, ICONS.diagnostic().error(), "error"),
        };
        if count == 0 {
            continue;
        }
        if needs_prefix {
            write(context, prefix.to_owned().into());
            needs_prefix = false;
        }
        let icon = if context.editor.config().icons {
            icon
        } else {
            icon.with_no_padding()
        };
        write(
            context,
            Span::styled(icon.to_string(), context.editor.theme.get(style)),
        );
        write(context, format!(" {count} ").into());
    }
}

fn render_selections<'a, F>(context: &mut RenderContext<'a>, write: F)
where
    F: Fn(&mut RenderContext<'a>, Span<'a>) + Copy,
{
    let selection = context.doc.selection(context.view.id);
    let count = selection.len();
    if count > 1 {
        write(
            context,
            format!(" {}/{count} sels ", selection.primary_index() + 1).into(),
        );
    }
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
    let total_line_numbers = context.doc.text().len_lines(stdx::rope::LINE_TYPE);

    write(context, format!(" {} ", total_line_numbers).into());
}

fn render_position_percentage<'a, F>(context: &mut RenderContext<'a>, write: F)
where
    F: Fn(&mut RenderContext<'a>, Span<'a>) + Copy,
{
    let position = get_position(context);
    let maxrows = context.doc.text().len_lines(stdx::rope::LINE_TYPE);
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
        && let Some(path) = context.doc.path()
    {
        write(
            context,
            icon_span(
                ICONS
                    .fs()
                    .file()
                    .get_with_style_or_default(path, &context.editor.theme),
            ),
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
    render_filename(context, write, FileNameFormat::Relative);
}

fn render_file_absolute_path<'a, F>(context: &mut RenderContext<'a>, write: F)
where
    F: Fn(&mut RenderContext<'a>, Span<'a>) + Copy,
{
    render_filename(context, write, FileNameFormat::Absolute);
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
    let title = if context.doc.is_readonly() {
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
    render_filename(context, write, FileNameFormat::Base);
}

enum FileNameFormat {
    Relative,
    Absolute,
    Base,
}

fn render_filename<'a, F>(context: &mut RenderContext<'a>, write: F, format: FileNameFormat)
where
    F: Fn(&mut RenderContext<'a>, Span<'a>) + Copy,
{
    write(context, format!(" {} ", filename(context, format)).into());
    render_review_status(context, write);
}

fn filename(context: &RenderContext<'_>, format: FileNameFormat) -> String {
    let doc = context.doc;
    let display = doc.display_metadata();
    let path = match format {
        FileNameFormat::Relative => doc
            .review()
            .map(|review| {
                review
                    .path()
                    .strip_prefix(review.workspace())
                    .unwrap_or(review.path())
            })
            .or_else(|| doc.display_path()),
        FileNameFormat::Absolute => display.source_path,
        FileNameFormat::Base => display
            .source_path
            .and_then(|path| path.file_name())
            .map(Path::new),
    };
    let name = path.map_or_else(
        || doc.display_name(),
        |path| display.format_path(path).into(),
    );
    name.into_owned()
}

fn render_review_status<'a, F>(context: &mut RenderContext<'a>, write: F)
where
    F: Fn(&mut RenderContext<'a>, Span<'a>) + Copy,
{
    let Some(review) = context.doc.review() else {
        return;
    };
    let number = if review.instance() > 1 {
        format!(" #{}", review.instance())
    } else {
        String::new()
    };
    write(
        context,
        format!(
            " DIFF {}{number} ",
            review
                .revision_label()
                .unwrap_or(review.comparison().name())
        )
        .into(),
    );
    write(
        context,
        Span::styled(
            format!("+{} ", review.review().additions),
            context.editor.theme.get("diff.plus"),
        ),
    );
    write(
        context,
        Span::styled(
            format!("−{} ", review.review().deletions),
            context.editor.theme.get("diff.minus"),
        ),
    );
    write(
        context,
        format!("{} ", review.review().provider.unwrap_or("line")).into(),
    );
    if review.review().unchanged_by_provider
        && (review.review().additions != 0 || review.review().deletions != 0)
    {
        write(context, "layout only ".into());
    }
    if review.is_loading() {
        write(context, "loading ".into());
    }
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
        && let Some(icon) = ICONS.vcs().branch()
    {
        write(context, icon_span(icon));
        write(context, format!("{head} ").into());
        return;
    }

    if !head.is_empty() {
        write(context, format!(" {head} ").into());
    }
}

fn render_register<'a, F>(context: &mut RenderContext<'a>, write: F)
where
    F: Fn(&mut RenderContext<'a>, Span<'a>) + Copy,
{
    if let Some(reg) = context.selected_register {
        write(context, format!(" reg={} ", reg).into())
    }
}

fn render_search_position<'a, F>(context: &mut RenderContext<'a>, write: F)
where
    F: Fn(&mut RenderContext<'a>, Span<'a>) + Copy,
{
    let Some(position) = context.doc.search_position(context.view.id) else {
        return;
    };
    let count = |value, unknown: &str| match value {
        Count::Exact(n) => n.to_string(),
        Count::MoreThan(n) => format!(">{n}"),
        Count::Unknown => unknown.to_owned(),
    };
    write(
        context,
        format!(
            " [{}/{}] ",
            count(position.current, "?"),
            count(position.total, "??")
        )
        .into(),
    );
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

    fn part(element: StatusLineElementID, text: &str, compact: Option<&str>) -> Part<'static> {
        Part {
            element,
            line: Line::raw(text.to_owned()),
            compact: compact.map(|text| Line::raw(text.to_owned())),
        }
    }

    #[test]
    fn language_is_hidden_before_other_fields_change() {
        use StatusLineElementID as E;
        let mut left = vec![part(E::FileName, " src/main.rs ", Some(" main.rs "))];
        let mut right = vec![
            part(E::FileType, " rust ", None),
            part(E::Position, " 1:1 ", None),
        ];
        fit_parts(&mut left, &mut right, 18);
        assert_eq!(join_parts(left).to_string(), " src/main.rs ");
        assert_eq!(join_parts(right).to_string(), " 1:1 ");
    }

    #[test]
    fn narrow_statuslines_preserve_essential_fields_on_either_side() {
        use StatusLineElementID as E;
        for swap_sides in [false, true] {
            for width in 34..=160 {
                let mut left = vec![
                    part(E::Mode, " NOR ", None),
                    part(
                        E::FileName,
                        " very/long/directory/main.rs ",
                        Some(" main.rs "),
                    ),
                    part(E::ReadOnlyIndicator, " [readonly] ", None),
                    part(E::FileModificationIndicator, "[+]", None),
                    part(E::Branch, &"feature/long-branch-".repeat(20), None),
                    part(
                        E::DebugSession,
                        &format!(
                            " Debug #1 {} (running, 1 session) ",
                            "long-name-".repeat(20)
                        ),
                        None,
                    ),
                ];
                let mut right = vec![
                    part(E::Diagnostics, " E 12345 W 12345 ", None),
                    part(E::Selections, " 1/20 sels ", None),
                    part(E::Register, " reg=a ", None),
                    part(E::SearchPosition, " [12345/12345] ", None),
                    part(E::FileType, " rust ", None),
                    part(E::Position, " 1:1 ", None),
                ];
                if swap_sides {
                    std::mem::swap(&mut left, &mut right);
                }
                fit_parts(&mut left, &mut right, width);
                assert!(parts_width(&left, &right) <= width);
                let text = format!("{}{}", join_parts(left), join_parts(right));
                for essential in ["NOR", "main.rs", "[readonly]", "[+]", "1:1"] {
                    assert!(
                        text.contains(essential),
                        "width={width}, swapped={swap_sides}: {text:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn long_basenames_keep_the_extension_and_file_state() {
        use StatusLineElementID as E;
        let mut left = vec![
            part(E::Mode, " NOR ", None),
            part(
                E::FileName,
                " very-long-basename.rs ",
                Some(" very-long-basename.rs "),
            ),
            part(E::ReadOnlyIndicator, " [readonly] ", None),
            part(E::FileModificationIndicator, "[+]", None),
        ];
        let mut right = vec![part(E::Position, " 1:1 ", None)];
        fit_parts(&mut left, &mut right, 34);
        assert!(parts_width(&left, &right) <= 34);
        let text = format!("{}{}", join_parts(left), join_parts(right));
        for essential in ["NOR", "…", ".rs", "[readonly]", "[+]", "1:1"] {
            assert!(text.contains(essential), "{text:?}");
        }
    }

    #[test]
    fn review_details_do_not_displace_the_basename() {
        use StatusLineElementID as E;
        let mut filename = part(E::FileName, " main.rs ", None);
        filename.line.spans.push(Span::raw("DIFF "));
        let mut left = vec![filename];
        let mut right = vec![part(E::Position, " 1:1 ", None)];
        fit_parts(&mut left, &mut right, 14);
        assert_eq!(join_parts(left).to_string(), " main.rs ");
        assert_eq!(join_parts(right).to_string(), " 1:1 ");
    }

    #[test]
    fn tiny_and_empty_statuslines_do_not_write_outside_their_viewport() {
        use StatusLineElementID as E;
        for width in 0..=12 {
            let mut left = vec![part(E::FileName, " wide-界.rs ", Some(" wide-界.rs "))];
            let mut right = vec![part(E::Position, " 123:456 ", None)];
            fit_parts(&mut left, &mut right, width as usize);
            let area = Rect::new(2, 1, width, 1);
            let mut buffer = Surface::empty(Rect::new(0, 0, 20, 3));
            buffer[(1, 1)].set_symbol("L");
            buffer[(area.right(), 1)].set_symbol("R");
            let right = join_parts(right);
            let [left_area, right_area] =
                statusline_areas(area, right.width().min(width as usize) as u16);
            join_parts(left).render(left_area, &mut buffer);
            right.right_aligned().render(right_area, &mut buffer);
            assert_eq!(buffer[(1, 1)].symbol(), "L");
            assert_eq!(buffer[(area.right(), 1)].symbol(), "R");
        }
    }

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
