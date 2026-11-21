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

use crate::{compositor::DrawContext, ui::ProgressSpinners};
use std::{borrow::Cow, collections::HashSet, path::Path};
use tokio::time::Instant;

use editor_config::StatusLineElement as StatusLineElementID;
use tui::buffer::Buffer as Surface;
use tui::layout::{Constraint, Layout};
use tui::text::{truncate_line, Line, Span};
use tui::widgets::Widget;

pub(crate) const ACTIVITY_DELAY: std::time::Duration = std::time::Duration::from_millis(200);

pub struct RenderContext<'a> {
    pub editor: &'a Editor,
    pub selected_register: Option<char>,
    pub doc: &'a Document,
    pub view: &'a View,
    pub spinners: &'a ProgressSpinners,
    base_style: Style,
    parts: Line<'a>,
    animation_deadline: Option<Instant>,
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
            base_style: Style::default(),
            parts: Line::default(),
            animation_deadline: None,
        }
    }

    fn push_span(&mut self, mut span: Span<'a>) {
        span.style = self.base_style.patch(span.style);
        self.parts.spans.push(span);
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

pub fn render(
    context: &mut RenderContext,
    viewport: Rect,
    surface: &mut Surface,
    draw: &mut DrawContext,
) {
    if viewport.is_empty() {
        return;
    }

    context.base_style = context.editor.theme.get(if context.is_focused() {
        "ui.statusline"
    } else {
        "ui.statusline.inactive"
    });
    surface.set_style(viewport.with_height(1), context.base_style);

    context.animation_deadline = None;
    let config = context.editor.config();
    let mut left = render_parts(context, &config.statusline.left);
    let mut right = render_parts(context, &config.statusline.right);
    fit_parts(&mut left, &mut right, viewport.width as usize);

    let left = join_parts(left);
    let right = join_parts(right);
    let [left_area, right_area] =
        statusline_areas(viewport, right.width().min(viewport.width as usize) as u16);
    left.render(left_area, surface);
    right.right_aligned().render(right_area, surface);
    if let Some(due) = context.animation_deadline {
        draw.request_animation_at(due);
    }
}

fn render_parts<'a>(
    context: &mut RenderContext<'a>,
    elements: &[StatusLineElementID],
) -> Vec<Part<'a>> {
    elements
        .iter()
        .map(|&element| {
            render_element(context, element);
            let line = std::mem::take(&mut context.parts);
            let compact = if is_filename(element) {
                let mut compact = Line::from(Span::styled(
                    format!(" {} ", filename(context, FileNameFormat::Base)),
                    context.base_style,
                ));
                if context.doc.review().is_some() {
                    compact
                        .spans
                        .push(Span::styled("DIFF ", context.base_style));
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
        E::Activity,
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

fn render_element(context: &mut RenderContext<'_>, element_id: StatusLineElementID) {
    match element_id {
        StatusLineElementID::Activity => render_activity(context),
        StatusLineElementID::Mode => render_mode(context),
        StatusLineElementID::Spinner => render_lsp_spinner(context),
        StatusLineElementID::FileBaseName => render_file_base_name(context),
        StatusLineElementID::FileName => render_file_name(context),
        StatusLineElementID::FileAbsolutePath => render_file_absolute_path(context),
        StatusLineElementID::FileModificationIndicator => {
            render_file_modification_indicator(context)
        }
        StatusLineElementID::ReadOnlyIndicator => render_read_only_indicator(context),
        StatusLineElementID::FileEncoding => render_file_encoding(context),
        StatusLineElementID::FileLineEnding => render_file_line_ending(context),
        StatusLineElementID::FileIndentStyle => render_file_indent_style(context),
        StatusLineElementID::FileType => render_file_type(context),
        StatusLineElementID::Diagnostics => render_diagnostics(context),
        StatusLineElementID::WorkspaceDiagnostics => render_workspace_diagnostics(context),
        StatusLineElementID::Selections => render_selections(context),
        StatusLineElementID::PrimarySelectionLength => render_primary_selection_length(context),
        StatusLineElementID::Position => render_position(context),
        StatusLineElementID::PositionPercentage => render_position_percentage(context),
        StatusLineElementID::TotalLineNumbers => render_total_line_numbers(context),
        StatusLineElementID::Separator => render_separator(context),
        StatusLineElementID::Spacer => render_spacer(context),
        StatusLineElementID::Branch => render_branch(context),
        StatusLineElementID::Register => render_register(context),
        StatusLineElementID::SearchPosition => render_search_position(context),
        StatusLineElementID::CurrentWorkingDirectory => render_cwd(context),
        StatusLineElementID::CodeActionHint => render_code_action_hint(context),
        StatusLineElementID::DebugSession => render_debug_session(context),
    }
}

fn render_debug_session(context: &mut RenderContext<'_>) {
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
    context.push_span(Span::raw(content));
}

fn render_mode(context: &mut RenderContext<'_>) {
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
    context.push_span(Span::styled(content, style));
}

fn render_activity(context: &mut RenderContext<'_>) {
    let Some(activity) = context.editor.activities().find(|activity| {
        activity.document.map_or_else(
            || context.is_focused(),
            |document| document == context.doc.id(),
        )
    }) else {
        return;
    };

    let now = Instant::now();
    let visible_at = activity.started + ACTIVITY_DELAY;
    if now < visible_at {
        context.animation_deadline = Some(visible_at);
    } else {
        let frame = super::spinner::frame(now - activity.started);
        context.push_span(format!(" {frame} {} ", activity.label).into());
        context.animation_deadline = Some(now + super::spinner::FRAME_INTERVAL);
    }
}

fn render_lsp_spinner(context: &mut RenderContext<'_>) {
    context.push_span(
        context
            .doc
            .language_servers()
            .find_map(|srv| context.spinners.frame(srv.id()))
            // Even if there's no spinner; reserve its space to avoid elements frequently shifting.
            .unwrap_or(" ")
            .into(),
    );
}

fn render_diagnostics(context: &mut RenderContext<'_>) {
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
    );
}

fn render_workspace_diagnostics(context: &mut RenderContext<'_>) {
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
    );
}

fn render_diagnostic_counts(
    context: &mut RenderContext<'_>,
    (hints, info, warnings, errors): (u32, u32, u32, u32),
    severities: &[::diagnostics::Severity],
    prefix: &str,
) {
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
            context.push_span(prefix.to_owned().into());
            needs_prefix = false;
        }
        let icon = if context.editor.config().icons {
            icon
        } else {
            icon.with_no_padding()
        };
        context.push_span(Span::styled(
            icon.to_string(),
            context.editor.theme.get(style),
        ));
        context.push_span(format!(" {count} ").into());
    }
}

fn render_selections(context: &mut RenderContext<'_>) {
    let selection = context.doc.selection(context.view.id);
    let count = selection.len();
    if count > 1 {
        context.push_span(format!(" {}/{count} sels ", selection.primary_index() + 1).into());
    }
}

fn render_primary_selection_length(context: &mut RenderContext<'_>) {
    let tot_sel = context.doc.selection(context.view.id).primary().len();
    context.push_span(format!(" {} char{} ", tot_sel, if tot_sel == 1 { "" } else { "s" }).into());
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

fn render_position(context: &mut RenderContext<'_>) {
    let position = get_position(context);
    context.push_span(format!(" {}:{} ", position.row + 1, position.col + 1).into());
}

fn render_total_line_numbers(context: &mut RenderContext<'_>) {
    let total_line_numbers = context.doc.text().len_lines(stdx::rope::LINE_TYPE);

    context.push_span(format!(" {} ", total_line_numbers).into());
}

fn render_position_percentage(context: &mut RenderContext<'_>) {
    let position = get_position(context);
    let maxrows = context.doc.text().len_lines(stdx::rope::LINE_TYPE);
    context.push_span(format!("{}%", (position.row + 1) * 100 / maxrows).into());
}

fn render_file_encoding(context: &mut RenderContext<'_>) {
    let enc = context.doc.encoding();

    if enc != encoding::UTF_8 {
        context.push_span(format!(" {} ", enc.name()).into());
    }
}

fn render_file_line_ending(context: &mut RenderContext<'_>) {
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

    context.push_span(format!(" {} ", line_ending).into());
}

fn render_file_type(context: &mut RenderContext<'_>) {
    let file_type = context.doc.language_name().unwrap_or(DEFAULT_LANGUAGE_NAME);

    if context.editor.config().icons
        && let Some(path) = context.doc.path()
    {
        context.push_span(icon_span(
            ICONS
                .fs()
                .file()
                .get_with_style_or_default(path, &context.editor.theme),
        ));
        context.push_span(format!("{file_type} ").into());
        return;
    }

    context.push_span(format!(" {} ", file_type).into());
}

fn render_file_name(context: &mut RenderContext<'_>) {
    render_filename(context, FileNameFormat::Relative);
}

fn render_file_absolute_path(context: &mut RenderContext<'_>) {
    render_filename(context, FileNameFormat::Absolute);
}

fn render_file_modification_indicator(context: &mut RenderContext<'_>) {
    let title = if context.doc.is_modified() { "[+]" } else { "" };

    context.push_span(title.into());
}

fn render_read_only_indicator(context: &mut RenderContext<'_>) {
    let title = if context.doc.is_readonly() {
        " [readonly] "
    } else {
        ""
    };
    context.push_span(title.into());
}

fn render_file_base_name(context: &mut RenderContext<'_>) {
    render_filename(context, FileNameFormat::Base);
}

enum FileNameFormat {
    Relative,
    Absolute,
    Base,
}

fn render_filename(context: &mut RenderContext<'_>, format: FileNameFormat) {
    context.push_span(format!(" {} ", filename(context, format)).into());
    render_review_status(context);
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
            .map(Cow::Borrowed)
            .or_else(|| {
                doc.path().map(|path| {
                    stdx::path::get_relative_path_from(path, context.editor.workspace().root())
                })
            }),
        FileNameFormat::Absolute => display.source_path.map(Cow::Borrowed),
        FileNameFormat::Base => display
            .source_path
            .and_then(|path| path.file_name())
            .map(Path::new)
            .map(Cow::Borrowed),
    };
    let name = path.map_or_else(
        || doc.display_name(),
        |path| display.format_path(&path).into(),
    );
    name.into_owned()
}

fn render_review_status(context: &mut RenderContext<'_>) {
    let Some(review) = context.doc.review() else {
        return;
    };
    let number = if review.instance() > 1 {
        format!(" #{}", review.instance())
    } else {
        String::new()
    };
    context.push_span(
        format!(
            " DIFF {}{number} ",
            review
                .revision_label()
                .unwrap_or(review.comparison().name())
        )
        .into(),
    );
    context.push_span(Span::styled(
        format!("+{} ", review.review().additions),
        context.editor.theme.get("diff.plus"),
    ));
    context.push_span(Span::styled(
        format!("−{} ", review.review().deletions),
        context.editor.theme.get("diff.minus"),
    ));
    if review.review().layout_only {
        context.push_span("layout only ".into());
    }
    if review.is_loading() {
        context.push_span("loading ".into());
    }
}

fn render_separator(context: &mut RenderContext<'_>) {
    let sep = &context.editor.config().statusline.separator;
    let style = context.editor.theme.get("ui.statusline.separator");

    context.push_span(Span::styled(sep.to_string(), style));
}

fn render_spacer(context: &mut RenderContext<'_>) {
    context.push_span(" ".into());
}

fn render_branch(context: &mut RenderContext<'_>) {
    let head = context
        .doc
        .version_control_head()
        .unwrap_or_default()
        .to_string();

    if context.editor.config().icons && !head.is_empty() {
        context.push_span(icon_span(ICONS.vcs().branch()));
        context.push_span(format!("{head} ").into());
        return;
    }

    if !head.is_empty() {
        context.push_span(format!(" {head} ").into());
    }
}

fn render_register(context: &mut RenderContext<'_>) {
    if let Some(reg) = context.selected_register {
        context.push_span(format!(" reg={} ", reg).into())
    }
}

fn render_search_position(context: &mut RenderContext<'_>) {
    let Some(position) = context.doc.search_position(context.view.id) else {
        return;
    };
    let count = |value, unknown: &str| match value {
        Count::Exact(n) => n.to_string(),
        Count::MoreThan(n) => format!(">{n}"),
        Count::Unknown => unknown.to_owned(),
    };
    context.push_span(
        format!(
            " [{}/{}] ",
            count(position.current, "?"),
            count(position.total, "??")
        )
        .into(),
    );
}

fn render_file_indent_style(context: &mut RenderContext<'_>) {
    let style = context.doc.indent_style;

    context.push_span(match style {
        IndentStyle::Tabs => " tabs ".into(),
        IndentStyle::Spaces(indent) => {
            format!(" {} space{} ", indent, if indent == 1 { "" } else { "s" }).into()
        }
    });
}

fn render_cwd(context: &mut RenderContext<'_>) {
    let cwd = stdx::env::current_working_dir();
    let cwd = cwd
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();
    context.push_span(cwd.into())
}

fn render_code_action_hint(context: &mut RenderContext<'_>) {
    if context.doc.code_action_hints(context.view.id) {
        context.push_span(" ⋮ ".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn file_names_keep_external_package_paths() -> anyhow::Result<()> {
        let mut app = crate::ui::picker::test::application(false)?;
        let document = view::current_ref!(app.editor).1.id();
        let external = tempfile::tempdir()?;
        let external = stdx::path::canonicalize(external.path());
        let module = external
            .join("go")
            .join("pkg")
            .join("mod")
            .join("example.com")
            .join("package@v1.2.3")
            .join("function.go");
        let repository = external.join("dependency");
        std::fs::create_dir_all(module.parent().unwrap())?;
        std::fs::create_dir_all(repository.join(".git"))?;

        for (path, expected) in [
            (
                app.editor.workspace().root().join("src").join("main.go"),
                Path::new("src").join("main.go"),
            ),
            (
                module.clone(),
                stdx::path::fold_home_dir(&module).into_owned(),
            ),
            (
                repository.join("src").join("function.go"),
                stdx::path::fold_home_dir(repository.join("src").join("function.go")).into_owned(),
            ),
        ] {
            app.editor.set_doc_path(document, &path);
            let (view, doc) = view::current_ref!(app.editor);
            let spinners = ProgressSpinners::default();
            let mut context = RenderContext::new(&app.editor, doc, view, &spinners, None);
            render_file_name(&mut context);
            assert_eq!(
                context.parts.to_string(),
                format!(" {} ", expected.display())
            );
            assert_eq!(
                filename(&context, FileNameFormat::Base),
                path.file_name().unwrap().to_string_lossy()
            );
            assert_eq!(
                filename(&context, FileNameFormat::Absolute),
                path.to_string_lossy()
            );
        }
        Ok(())
    }

    fn rendered_activity(editor: &Editor, view: &View) -> String {
        let doc = editor.document(view.document_id()).unwrap();
        let spinners = ProgressSpinners::default();
        let mut context = RenderContext::new(editor, doc, view, &spinners, None);
        render_activity(&mut context);
        context.parts.to_string()
    }

    #[tokio::test(start_paused = true)]
    async fn activity_is_delayed_animates_and_retires_without_idle_padding() -> anyhow::Result<()> {
        let mut app = crate::ui::picker::test::application(false)?;
        let (view, doc) = view::current_ref!(app.editor);
        let (view_id, document) = (view.id, doc.id());
        let first = app.editor.start_activity("Formatting", Some(document));
        let second = app.editor.start_activity("References", Some(document));
        let view = app.editor.tree.get(view_id);
        assert_eq!(rendered_activity(&app.editor, view), "");
        tokio::time::advance(ACTIVITY_DELAY - std::time::Duration::from_millis(1)).await;
        assert_eq!(rendered_activity(&app.editor, view), "");
        tokio::time::advance(std::time::Duration::from_millis(1)).await;
        let first_frame = rendered_activity(&app.editor, view);
        assert!(first_frame.contains("Formatting"), "{first_frame:?}");
        tokio::time::advance(std::time::Duration::from_millis(80)).await;
        assert_ne!(rendered_activity(&app.editor, view), first_frame);
        drop(first);
        assert!(rendered_activity(&app.editor, view).contains("References"));
        drop(second);
        assert_eq!(rendered_activity(&app.editor, view), "");
        Ok(())
    }

    #[tokio::test(start_paused = true)]
    async fn activity_targets_its_document_or_the_focused_statusline() -> anyhow::Result<()> {
        let mut app = crate::ui::picker::test::application(false)?;
        let document = view::current_ref!(app.editor).1.id();
        let action = app.editor.start_activity("Formatting", Some(document));
        app.editor.new_file(view::editor::Action::VerticalSplit);
        tokio::time::advance(ACTIVITY_DELAY).await;
        for (view, _) in app.editor.tree.views() {
            assert_eq!(
                rendered_activity(&app.editor, view).contains("Formatting"),
                view.document_id() == document,
            );
        }
        drop(action);
        let workspace = app.editor.start_activity("Git", None);
        tokio::time::advance(ACTIVITY_DELAY).await;
        for (view, focused) in app.editor.tree.views() {
            assert_eq!(
                rendered_activity(&app.editor, view).contains("Git"),
                focused
            );
        }
        drop(workspace);
        Ok(())
    }

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
                    part(E::Activity, " ⣾ Formatting ", None),
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
