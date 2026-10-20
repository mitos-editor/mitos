use super::highlights::{self, primary_cursor_is_software};
use crate::repeat::InsertEvent;
use crate::{
    commands::{self, OnKeyCallback, OnKeyCallbackKind},
    compositor::{Component, Context, Cursor, Event, EventResult},
    events::OnModeSwitch,
    key,
    keymap::{KeymapResult, Keymaps},
    ui::{
        document::{render_document, LinePos, TextRenderer},
        image::{render_placeholder, ImageDocumentView},
        layout::ApplicationLayout,
        statusline,
        text_decorations::{self, Decoration, DecorationManager, InlineDiagnostics},
        Completion, ProgressSpinners,
    },
};
use stdx::rope::RopeExt as _;
use text_layout::DisplayPosition;

use ::diagnostics::NumberOrString;
use editor_config::{BufferLine, DiagnosticFilter};
use editor_core::{movement::Direction, unicode::width::UnicodeWidthStr, Range, Selection};
use loader::VERSION_AND_GIT_HASH;
use std::{
    collections::HashMap, mem::take, num::NonZeroUsize, path::PathBuf, rc::Rc, sync::LazyLock,
};
use text_layout::visual_offset_from_block;
use theme::icons::ICONS;
use theme::Theme;
use ui_core::{
    input::{KeyEvent, MouseButton, MouseEvent, MouseEventKind},
    keyboard::{KeyCode, KeyModifiers},
};
use view::{
    document::{Mode, SCRATCH_BUFFER_NAME},
    features::completion::CompletionItem,
    graphics::{Color, CursorKind, Modifier, Rect, Style},
    Document, Editor, View, ViewId,
};

use tui::{
    buffer::Buffer as Surface,
    layout::{Alignment, Constraint, Layout, Size},
    text::{Line, Span},
    widgets::{Paragraph, Row, Table, TableState, Widget},
};

pub struct EditorView {
    pub keymaps: Keymaps,
    on_next_key: Option<(OnKeyCallback, OnKeyCallbackKind)>,
    pseudo_pending: Vec<KeyEvent>,
    pub(crate) repeat: crate::repeat::RepeatController,
    pub(crate) completion: Option<Completion>,
    spinners: ProgressSpinners,
    images: HashMap<ViewId, ImageDocumentView>,
    /// Tracks if the terminal window is focused by reaction to terminal focus events
    terminal_focused: bool,
}

impl EditorView {
    pub fn new(keymaps: Keymaps) -> Self {
        Self {
            keymaps,
            on_next_key: None,
            pseudo_pending: Vec::new(),
            repeat: crate::repeat::RepeatController::default(),
            completion: None,
            spinners: ProgressSpinners::default(),
            images: HashMap::new(),
            terminal_focused: true,
        }
    }

    pub(crate) fn set_next_key_callback(
        &mut self,
        callback: (commands::OnKeyCallback, OnKeyCallbackKind),
    ) {
        self.on_next_key = Some(callback);
    }

    pub(crate) fn record_insert_event(&mut self, event: InsertEvent) {
        self.repeat.record(event);
    }

    fn render_welcome(theme: &Theme, area: Rect, surface: &mut Surface, colorful: bool) {
        const LOGO: &str = r#"███╗   ███╗██╗████████╗ ██████╗ ███████╗
████╗ ████║██║╚══██╔══╝██╔═══██╗██╔════╝
██╔████╔██║██║   ██║   ██║   ██║███████╗
██║╚██╔╝██║██║   ██║   ██║   ██║╚════██║
██║ ╚═╝ ██║██║   ██║   ╚██████╔╝███████║
╚═╝     ╚═╝╚═╝   ╚═╝    ╚═════╝ ╚══════╝"#;
        const LOGO_HELP_PADDING: u16 = 6;

        static LOGO_WIDTH: LazyLock<u16> = LazyLock::new(|| {
            LOGO.lines()
                .map(UnicodeWidthStr::width)
                .max()
                .unwrap_or_default() as u16
        });
        static PLAIN_LOGO: LazyLock<Vec<Line<'static>>> = LazyLock::new(|| {
            LOGO.lines()
                .map(|line| Line::from(line.to_owned()))
                .collect()
        });
        static COLOR_LOGO: LazyLock<Vec<Line<'static>>> = LazyLock::new(|| {
            let colors = [
                Color::Rgb(110, 170, 255),
                Color::Rgb(125, 130, 255),
                Color::Rgb(180, 100, 245),
                Color::Rgb(230, 95, 190),
                Color::Rgb(245, 120, 130),
                Color::Rgb(245, 160, 90),
            ];
            LOGO.lines()
                .zip(colors)
                .map(|(line, color)| Line::styled(line.to_owned(), Style::default().fg(color)))
                .collect()
        });

        let text = theme.get("ui.text");
        let dim = theme.get("ui.text.inactive");
        let accent = theme.get("special");
        let command_style = theme.get("constant");
        let header = [
            Line::styled(
                format!("mitos {VERSION_AND_GIT_HASH}"),
                accent.add_modifier(Modifier::BOLD),
            ),
            Line::default(),
            Line::styled(
                "A post-modern, modal text editor",
                dim.add_modifier(Modifier::ITALIC),
            ),
            Line::default(),
        ];
        let commands = [
            (":tutor", "<enter>", "learn Mitos"),
            (":theme", "<space><tab>", "choose a theme"),
            ("<space>e", "", "file explorer"),
            ("<space>?", "", "see all commands"),
            (":quit", "<enter>", "quit Mitos"),
        ];
        let footer = [
            Line::default(),
            Line::from(vec![
                Span::styled("project: ", dim),
                Span::styled("github.com/mitos-editor/mitos", accent),
            ]),
        ];
        let command_width = commands
            .iter()
            .map(|(command, suffix, _)| command.width() + suffix.width())
            .max()
            .unwrap_or_default() as u16;
        let description_width = commands
            .iter()
            .map(|(_, _, description)| description.width())
            .max()
            .unwrap_or_default() as u16;
        const COMMAND_SPACING: u16 = 2;
        let command_table_width = command_width + COMMAND_SPACING + description_width;

        let help_height = (header.len() + commands.len() + footer.len()) as u16;
        if area.height < help_height {
            return;
        }

        let help_width = header
            .iter()
            .chain(&footer)
            .map(|line| line.width() as u16)
            .chain(std::iter::once(command_table_width))
            .max()
            .unwrap_or_default();
        if area.width < help_width {
            return;
        }

        let combined_width = LOGO_WIDTH
            .saturating_add(LOGO_HELP_PADDING)
            .saturating_add(help_width);
        let show_logo = area.width >= combined_width;
        let content_width = if show_logo {
            combined_width
        } else {
            help_width
        };
        let welcome_area = area.centered(
            Constraint::Length(content_width),
            Constraint::Length(help_height),
        );

        let help_area = if show_logo {
            let [logo_area, _padding, help_area] = Layout::horizontal([
                Constraint::Length(*LOGO_WIDTH),
                Constraint::Length(LOGO_HELP_PADDING),
                Constraint::Length(help_width),
            ])
            .areas(welcome_area);
            let logo = if colorful { &*COLOR_LOGO } else { &*PLAIN_LOGO };
            let logo_area = logo_area.centered_vertically(Constraint::Length(logo.len() as u16));
            Paragraph::new(logo.clone()).render(logo_area, surface);
            help_area
        } else {
            welcome_area
        };

        let [header_area, commands_area, footer_area] = Layout::vertical([
            Constraint::Length(header.len() as u16),
            Constraint::Length(commands.len() as u16),
            Constraint::Length(footer.len() as u16),
        ])
        .areas(help_area);

        Paragraph::new(header.to_vec())
            .alignment(Alignment::Center)
            .render(header_area, surface);

        let rows = commands.map(|(command, suffix, description)| {
            Row::new([
                Line::from(vec![
                    Span::styled(command.to_owned(), command_style),
                    Span::styled(suffix.to_owned(), dim),
                ]),
                Line::styled(description.to_owned(), text),
            ])
        });
        Table::new(rows)
            .widths(&[
                Constraint::Length(command_width),
                Constraint::Length(description_width),
            ])
            .column_spacing(COMMAND_SPACING)
            .render_table(commands_area, surface, &mut TableState::default(), false);

        Paragraph::new(footer.to_vec())
            .alignment(Alignment::Center)
            .render(footer_area, surface);
    }

    #[allow(clippy::too_many_lines)]
    pub fn render_view(
        &self,
        cx: &Context,
        doc: &Document,
        view: &View,
        viewport: Rect,
        surface: &mut Surface,
        is_focused: bool,
    ) {
        let editor = &*cx.editor;
        if doc.is_binary() {
            Self::render_view_border(editor, view, viewport, surface);
            return;
        }
        let layout = view.layout(doc, Some(&editor.theme));
        let inner = layout.viewport;
        let area = view.content_area(doc);
        let theme = &editor.theme;
        let config = editor.config();

        let view_offset = layout.offset;

        let text_annotations = &layout.annotations;
        let mut decorations = DecorationManager::default();

        if doc.breadcrumb_enabled() && !area.is_empty() {
            Self::render_breadcrumb(editor, doc, view, area.with_height(1), surface);
        }

        if is_focused && config.cursor.highlight_line {
            decorations.add_decoration(Self::cursorline(doc, view, theme));
        }

        if is_focused && config.cursor.highlight_column {
            Self::highlight_cursorcolumn(doc, view, surface, theme, &layout);
        }

        // Set DAP highlights, if needed.
        if let Some(frame) = editor.debugger_frame_in_document(doc)
            && let Some(dap_line) = view::debugger::source::line_index(frame.line)
        {
            let style = theme.get("ui.highlight.frameline");
            let line_decoration = move |renderer: &mut TextRenderer, pos: LinePos| {
                if pos.doc_line != dap_line {
                    return;
                }
                renderer.set_style(Rect::new(inner.x, pos.visual_line, inner.width, 1), style);
            };

            decorations.add_decoration(line_decoration);
        }

        let syntax_highlighter =
            highlights::syntax_highlighter(doc, view_offset.anchor, inner.height);
        let mut overlays = Vec::new();

        overlays.push(highlights::overlay_highlights(
            doc,
            view_offset.anchor,
            inner.height,
            text_annotations,
        ));

        if doc
            .language_config()
            .and_then(|config| config.rainbow_brackets)
            .unwrap_or(config.rainbow_brackets.enable)
            && let Some(overlay) =
                highlights::rainbow_highlights(doc, view_offset.anchor, inner.height, theme)
        {
            overlays.push(overlay);
        }

        if let Some(overlay) = highlights::document_link_highlights(doc, theme) {
            overlays.push(overlay);
        }

        highlights::diagnostic_highlights(doc, theme, &mut overlays);

        if is_focused {
            if config.lsp.document_highlight.auto_trigger
                && let Some(overlay) = highlights::document_highlights(doc, view, theme)
            {
                overlays.push(overlay);
            }
            if let Some(tabstops) = highlights::tabstop_highlights(doc, theme) {
                overlays.push(tabstops);
            }
            overlays.push(highlights::selection_highlights(
                editor.mode(),
                doc,
                view,
                theme,
                &config.cursor.shape,
                self.terminal_focused,
                cx.is_cursor_owner,
            ));
            if let Some(overlay) = highlights::matching_bracket_highlight(view, doc, theme) {
                overlays.push(overlay);
            }
        }

        let gutter_overflow = view.gutter_offset(doc) == 0;
        if !gutter_overflow {
            Self::render_gutter(
                editor,
                doc,
                view,
                area,
                theme,
                is_focused & self.terminal_focused,
                &mut decorations,
            );
        }

        Self::render_rulers(editor, doc, view, inner, surface, theme);

        let primary_cursor = doc
            .selection(view.id)
            .primary()
            .cursor(doc.text().slice(..));
        if is_focused {
            // This frame's rendered grapheme is authoritative. An offscreen cursor
            // stays absent instead of reusing a position cached before an edit.
            editor.cursor_cache.set(None);
            decorations.add_decoration(text_decorations::Cursor {
                cache: &editor.cursor_cache,
                primary_cursor,
            });
        }
        if is_focused
            && editor.mode() == Mode::Insert
            && let Some(completion) = doc.inline_completion(view.id)
        {
            let line = doc.text().char_to_line_idx(completion.cursor);
            let line_end =
                editor_core::line_ending::line_end_char_index(&doc.text().slice(..), line);
            decorations.add_decoration(
                text_decorations::inline_completion::InlineCompletionDecoration::new(
                    completion,
                    theme
                        .try_get_exact("ui.virtual.inline-completion")
                        .unwrap_or_else(|| theme.get("ui.virtual.inlay-hint")),
                    layout.format.clone(),
                    line_end,
                ),
            );
        }
        let width = view.inner_width(doc);
        let document_config = doc.config.load();
        let enable_cursor_line = view
            .diagnostics_handler
            .show_cursorline_diagnostics(doc, view.id);
        let inline_diagnostic_config = view::annotations::diagnostics::prepare_inline_diagnostics(
            &document_config.diagnostics.inline,
            width,
            enable_cursor_line,
        );
        decorations.add_decoration(InlineDiagnostics::new(
            doc,
            theme,
            primary_cursor,
            inline_diagnostic_config,
            document_config.diagnostics.end_of_line.severity,
            document_config.icons,
        ));
        if config.inline_blame.show != editor_config::InlineBlameShow::Never {
            decorations.add_decoration(text_decorations::blame::InlineBlame::new(
                doc,
                view.id,
                &config.inline_blame,
                theme
                    .try_get_exact("ui.virtual.inline-blame")
                    .unwrap_or_else(|| theme.get("ui.virtual.inlay-hint")),
            ));
        }
        let hidden_line = if is_focused && editor.mode() == Mode::Insert {
            doc.inline_completion(view.id)
                .map(|completion| doc.text().char_to_line_idx(completion.cursor))
        } else {
            None
        };
        decorations.add_decoration(text_decorations::debug_values::DebugValues::new(
            doc.text(),
            editor.debugger_inline_values(view.id),
            theme
                .try_get_exact("ui.virtual.debug-value")
                .unwrap_or_else(|| theme.get("ui.virtual.inlay-hint")),
            hidden_line,
        ));
        if document_config.lsp.enable && document_config.lsp.code_lenses.enable {
            decorations.add_decoration(text_decorations::code_lenses::CodeLensDecoration::new(
                doc.text(),
                doc.code_lenses(),
                theme
                    .try_get_exact("ui.virtual.code-lens")
                    .unwrap_or_else(|| theme.get("ui.virtual.inlay-hint")),
                hidden_line,
            ));
        }
        render_document(
            surface,
            doc,
            &layout,
            syntax_highlighter,
            overlays,
            theme,
            decorations,
        );

        if editor.config().welcome_screen && doc.version() == 0 && doc.is_welcome {
            Self::render_welcome(
                theme,
                inner,
                surface,
                cx.config.current.terminal.true_color || crate::true_color(),
            );
        }

        Self::render_view_border(editor, view, viewport, surface);

        if config.diagnostics.inline.disabled()
            && config.diagnostics.end_of_line.severity == DiagnosticFilter::Disable
        {
            Self::render_diagnostics(doc, view, inner, surface, theme);
        }
    }

    fn render_view_border(editor: &Editor, view: &View, viewport: Rect, surface: &mut Surface) {
        // if we're not at the edge of the screen, draw a right border
        if viewport.right() != view.area.right() {
            let x = view.area.right();
            let border_style = editor.theme.get("ui.window");
            for y in view.area.top()..view.area.bottom() {
                surface[(x, y)]
                    .set_symbol(tui::symbols::line::VERTICAL)
                    .set_style(border_style);
            }
        }
    }

    pub fn render_rulers(
        editor: &Editor,
        doc: &Document,
        view: &View,
        viewport: Rect,
        surface: &mut Surface,
        theme: &Theme,
    ) {
        let editor_rulers = &editor.config().rulers;
        let ruler_theme = theme
            .try_get("ui.virtual.ruler")
            .unwrap_or_else(|| Style::default().bg(Color::Red));

        let rulers = doc
            .language_config()
            .and_then(|config| config.rulers.as_ref())
            .unwrap_or(editor_rulers);

        let view_offset = doc.view_offset(view.id);

        rulers
            .iter()
            // View might be horizontally scrolled, convert from absolute distance
            // from the 1st column to relative distance from left of viewport
            .filter_map(|ruler| ruler.checked_sub(1 + view_offset.horizontal_offset as u16))
            .filter(|ruler| ruler < &viewport.width)
            .map(|ruler| viewport.clip_left(ruler).with_width(1))
            .for_each(|area| surface.set_style(area, ruler_theme))
    }

    pub fn render_bufferline(editor: &Editor, viewport: Rect, surface: &mut Surface) {
        let scratch = PathBuf::from(SCRATCH_BUFFER_NAME); // default filename to use for scratch buffer
        surface.clear_with(
            viewport,
            editor
                .theme
                .try_get("ui.bufferline.background")
                .unwrap_or_else(|| editor.theme.get("ui.statusline")),
        );

        let bufferline_active = editor
            .theme
            .try_get("ui.bufferline.active")
            .unwrap_or_else(|| editor.theme.get("ui.statusline.active"));

        let bufferline_inactive = editor
            .theme
            .try_get("ui.bufferline")
            .unwrap_or_else(|| editor.theme.get("ui.statusline.inactive"));

        let current_doc = view!(editor).document_id();
        let mut current_tab = 0;
        // PERF: Bufferline rendering needs an owned title list every frame. Cache these by document
        // metadata if bufferline rendering becomes measurable with very large buffer sets.
        let titles: Vec<Line<'static>> = editor
            .documents()
            .enumerate()
            .map(|(index, doc)| {
                if current_doc == doc.id() {
                    current_tab = index;
                }

                let fname = doc
                    .path()
                    .unwrap_or(&scratch)
                    .file_name()
                    .unwrap_or_default()
                    .to_str()
                    .unwrap_or_default();

                let style = if current_doc == doc.id() {
                    bufferline_active
                } else {
                    bufferline_inactive
                };

                let show_file_icon = editor.config().icons && doc.path().is_some();
                let modified = if doc.is_modified() { "[+]" } else { "" };

                if show_file_icon && let Some(path) = doc.path() {
                    let icons = ICONS.load();
                    if let Some(file) = icons.fs().file() {
                        let icon =
                            file.get_with_active_style_or_default(path, &editor.theme, style);
                        return Line::from(vec![
                            super::icon_span(icon),
                            Span::styled(format!("{fname}{modified} "), style),
                        ]);
                    }
                }

                Line::styled(format!(" {fname}{modified} "), style)
            })
            .collect();

        super::bufferline::render(&titles, current_tab, viewport, surface);
    }

    #[allow(clippy::too_many_lines)]
    pub fn render_breadcrumb(
        editor: &Editor,
        doc: &Document,
        view: &View,
        viewport: Rect,
        surface: &mut Surface,
    ) {
        use editor_config::BreadcrumbPathOptions::{File, Full};

        #[inline]
        #[must_use]
        fn draw_element(
            surface: &mut Surface,
            viewport: Rect,
            x: u16,
            content: &str,
            style: Style,
        ) -> u16 {
            let remaining = viewport.right().saturating_sub(x) as usize;
            surface
                .set_stringn(x, viewport.y, content, remaining, style)
                .0
        }

        let config = editor.config();

        // PERF: Render directly into the surface to avoid allocating a span list every frame.

        let style = editor
            .theme
            .try_get_exact("ui.breadcrumb")
            .unwrap_or_else(|| editor.theme.get("ui.text"));

        surface.clear_with(viewport, style);

        let mut x = viewport.x.saturating_add(1);

        let separator = " > ";
        let separator_style = editor.theme.get("ui.breadcrumb.separator");
        let mut draw_separator = false;

        if matches!(config.breadcrumb.path, Full | File) {
            if let Some(path) = doc.display_path() {
                let mut components = path.components().peekable();
                let file_only = matches!(config.breadcrumb.path, File);

                loop {
                    let component = if file_only {
                        components.next_back()
                    } else {
                        components.next()
                    };
                    let Some(component) = component else {
                        break;
                    };
                    if draw_separator {
                        x = draw_element(surface, viewport, x, separator, separator_style);
                    } else {
                        draw_separator = true;
                    }

                    let segment = component.as_os_str().to_string_lossy();
                    let is_directory = !file_only && components.peek().is_some();

                    let style = if is_directory {
                        editor.theme.get("ui.text.directory")
                    } else {
                        style
                    };

                    x = draw_element(surface, viewport, x, &segment, style);

                    if file_only {
                        break;
                    }
                }
            } else {
                // Handle `[scratch]`
                x = draw_element(surface, viewport, x, SCRATCH_BUFFER_NAME, style);
            }
        }

        // Draw symbols, if any.
        if let Some(breadcrumb) = doc.breadcrumbs(view.id) {
            for symbol in breadcrumb.iter() {
                if draw_separator {
                    x = draw_element(surface, viewport, x, separator, separator_style);
                } else {
                    draw_separator = true;
                }

                let style = editor
                    .theme
                    .get(crate::ui::lsp::symbol_kind_scope(symbol.kind));

                x = draw_element(surface, viewport, x, symbol.name.as_ref(), style);
            }
        }
    }

    pub fn render_gutter<'d>(
        editor: &'d Editor,
        doc: &'d Document,
        view: &View,
        viewport: Rect,
        theme: &Theme,
        is_focused: bool,
        decoration_manager: &mut DecorationManager<'d>,
    ) {
        let text = doc.text().slice(..);
        let cursors: Rc<[_]> = doc
            .selection(view.id)
            .iter()
            .map(|range| range.cursor_line(text))
            .collect();

        let mut offset = 0;

        let gutter_style = theme.get("ui.gutter");
        let gutter_selected_style = theme.get("ui.gutter.selected");
        let gutter_style_virtual = theme.get("ui.gutter.virtual");
        let gutter_selected_style_virtual = theme.get("ui.gutter.selected.virtual");

        for &gutter_type in view.gutters() {
            let mut gutter =
                super::gutter::style(gutter_type, editor, doc, view, theme, is_focused);
            let width = view::gutter::width(gutter_type, view, doc);
            // avoid lots of small allocations by reusing a text buffer for each line
            let mut text = String::with_capacity(width);
            let cursors = cursors.clone();
            let gutter_decoration = move |renderer: &mut TextRenderer, pos: LinePos| {
                // TODO handle softwrap in gutters
                let selected = cursors.contains(&pos.doc_line);
                let x = viewport.x + offset;
                let y = pos.visual_line;

                let gutter_style = match (selected, pos.first_visual_line) {
                    (false, true) => gutter_style,
                    (true, true) => gutter_selected_style,
                    (false, false) => gutter_style_virtual,
                    (true, false) => gutter_selected_style_virtual,
                };

                if let Some(style) =
                    gutter(pos.doc_line, selected, pos.first_visual_line, &mut text)
                {
                    renderer.set_stringn(x, y, &text, width, gutter_style.patch(style));
                } else {
                    renderer.set_style(
                        Rect {
                            x,
                            y,
                            width: width as u16,
                            height: 1,
                        },
                        gutter_style,
                    );
                }
                text.clear();
            };
            decoration_manager.add_decoration(gutter_decoration);

            offset += width as u16;
        }
    }

    pub fn render_diagnostics(
        doc: &Document,
        view: &View,
        viewport: Rect,
        surface: &mut Surface,
        theme: &Theme,
    ) {
        use ::diagnostics::Severity;
        use tui::{
            layout::Alignment,
            text::Text,
            widgets::{Paragraph, Widget, Wrap},
        };

        let cursor = doc
            .selection(view.id)
            .primary()
            .cursor(doc.text().slice(..));

        let diagnostics = doc.diagnostics().iter().filter(|diagnostic| {
            diagnostic.range.start <= cursor && diagnostic.range.end >= cursor
        });

        let warning = theme.get("warning");
        let error = theme.get("error");
        let info = theme.get("info");
        let hint = theme.get("hint");

        let mut lines = Vec::new();
        let background_style = theme.get("ui.background");
        for diagnostic in diagnostics {
            let style = Style::reset()
                .patch(background_style)
                .patch(match diagnostic.severity {
                    Some(Severity::Error) => error,
                    Some(Severity::Warning) | None => warning,
                    Some(Severity::Info) => info,
                    Some(Severity::Hint) => hint,
                });
            let text = Text::styled(&*diagnostic.message, style);
            lines.extend(text.lines);
            let code = diagnostic.code.as_ref().map(|x| match x {
                NumberOrString::Number(n) => format!("({n})"),
                NumberOrString::String(s) => format!("({s})"),
            });
            if let Some(code) = code {
                let span = Span::styled(code, style);
                lines.push(span.into());
            }
        }

        let text = Text::from(lines);
        let paragraph = Paragraph::new(text)
            .alignment(Alignment::Right)
            .wrap(Wrap { trim: true });
        let width = 100.min(viewport.width);
        let height = 15.min(viewport.height);
        paragraph.render(
            Rect::new(viewport.right() - width, viewport.y + 1, width, height),
            surface,
        );
    }

    /// Apply the highlighting on the lines where a cursor is active
    pub fn cursorline(doc: &Document, view: &View, theme: &Theme) -> impl Decoration + use<> {
        let text = doc.text().slice(..);
        // TODO only highlight the visual line that contains the cursor instead of the full visual line
        let primary_line = doc.selection(view.id).primary().cursor_line(text);

        // The secondary_lines do contain the primary_line, it doesn't matter
        // as the else-if clause in the loop later won't test for the
        // secondary_lines if primary_line == line.
        // It's used inside a loop so the collect isn't needless:
        // https://github.com/rust-lang/rust-clippy/issues/6164
        #[allow(clippy::needless_collect)]
        let secondary_lines: Vec<_> = doc
            .selection(view.id)
            .iter()
            .map(|range| range.cursor_line(text))
            .collect();

        let primary_style = theme.get("ui.cursorline.primary");
        let secondary_style = theme.get("ui.cursorline.secondary");
        let viewport = view.area;

        move |renderer: &mut TextRenderer, pos: LinePos| {
            let area = Rect::new(viewport.x, pos.visual_line, viewport.width, 1);
            if primary_line == pos.doc_line {
                renderer.set_style(area, primary_style);
            } else if secondary_lines.binary_search(&pos.doc_line).is_ok() {
                renderer.set_style(area, secondary_style);
            }
        }
    }

    /// Apply the highlighting on the columns where a cursor is active
    pub fn highlight_cursorcolumn(
        doc: &Document,
        view: &View,
        surface: &mut Surface,
        theme: &Theme,
        layout: &view::view::ViewLayout,
    ) {
        let text = layout.text;

        // Manual fallback behaviour:
        // ui.cursorcolumn.{p/s} -> ui.cursorcolumn -> ui.cursorline.{p/s}
        let primary_style = theme
            .try_get_exact("ui.cursorcolumn.primary")
            .or_else(|| theme.try_get_exact("ui.cursorcolumn"))
            .unwrap_or_else(|| theme.get("ui.cursorline.primary"));
        let secondary_style = theme
            .try_get_exact("ui.cursorcolumn.secondary")
            .or_else(|| theme.try_get_exact("ui.cursorcolumn"))
            .unwrap_or_else(|| theme.get("ui.cursorline.secondary"));

        let inner_area = layout.viewport;

        let selection = doc.selection(view.id);
        let view_offset = layout.offset;
        let primary = selection.primary();
        let text_format = &layout.format;
        let text_annotations = &layout.annotations;
        for range in selection.iter() {
            let is_primary = primary == *range;
            let cursor = range.cursor(text);

            let DisplayPosition { col, .. } =
                visual_offset_from_block(text, cursor, cursor, text_format, text_annotations).0;

            // if the cursor is horizontally in the view
            if col >= view_offset.horizontal_offset
                && inner_area.width > (col - view_offset.horizontal_offset) as u16
            {
                let area = Rect::new(
                    inner_area.x + (col - view_offset.horizontal_offset) as u16,
                    inner_area.y,
                    1,
                    inner_area.height,
                );
                if is_primary {
                    surface.set_style(area, primary_style)
                } else {
                    surface.set_style(area, secondary_style)
                }
            }
        }
    }

    /// Handle events by looking them up in `self.keymaps`. Returns None
    /// if event was handled (a command was executed or a subkeymap was
    /// activated). Only KeymapResult::{NotFound, Cancelled} is returned
    /// otherwise.
    fn handle_keymap_event(
        &mut self,
        mode: Mode,
        cxt: &mut commands::Context,
        event: KeyEvent,
    ) -> Option<KeymapResult> {
        let mut last_mode = mode;
        self.pseudo_pending.extend(self.keymaps.pending());
        let key_result = self.keymaps.get(mode, event);
        cxt.editor.autoinfo = self.keymaps.sticky().map(|node| node.infobox());

        let mut execute_command = |command: &commands::MappableCommand| {
            command.execute(cxt);

            let current_mode = cxt.editor.mode();
            if current_mode != last_mode {
                event::dispatch(OnModeSwitch {
                    old_mode: last_mode,
                    new_mode: current_mode,
                    cx: cxt,
                })
                .report_to(cxt.editor.status_sender());

                // HAXX: if we just entered insert mode from normal, clear key buf
                // and record the command that got us into this mode.
                if current_mode == Mode::Insert {
                    // how we entered insert mode is important, and we should track that so
                    // we can repeat the side effect.
                    self.repeat.start_insert(command.clone());
                }
            }

            last_mode = current_mode;
        };

        match &key_result {
            KeymapResult::Matched(command) => {
                execute_command(command);
            }
            KeymapResult::Pending(node) => cxt.editor.autoinfo = Some(node.infobox()),
            KeymapResult::MatchedSequence(commands) => {
                for command in commands {
                    execute_command(command);
                }
            }
            KeymapResult::NotFound | KeymapResult::Cancelled(_) => return Some(key_result),
        }
        None
    }

    pub(crate) fn insert_mode(
        &mut self,
        cx: &mut commands::Context,
        event: KeyEvent,
    ) -> Result<(), view::editing::EditError> {
        if let Some(keyresult) = self.handle_keymap_event(Mode::Insert, cx, event) {
            match keyresult {
                KeymapResult::NotFound => {
                    if !self.on_next_key(OnKeyCallbackKind::Fallback, cx, event)
                        && let Some(ch) = event.char()
                    {
                        commands::insert::try_insert_char(cx, ch)?
                    }
                }
                KeymapResult::Cancelled(pending) => {
                    for ev in pending {
                        match ev.char() {
                            Some(ch) => commands::insert::try_insert_char(cx, ch)?,
                            None => {
                                if let KeymapResult::Matched(command) =
                                    self.keymaps.get(Mode::Insert, ev)
                                {
                                    command.execute(cx);
                                }
                            }
                        }
                    }
                }
                _ => unreachable!(),
            }
        }
        Ok(())
    }

    fn command_mode(&mut self, mode: Mode, cxt: &mut commands::Context, event: KeyEvent) {
        match (event, cxt.editor.count) {
            // If the count is already started and the input is a number, always continue the count.
            (key!(i @ '0'..='9'), Some(count)) => {
                let i = i.to_digit(10).unwrap() as usize;
                let count = count.get() * 10 + i;
                if count > 100_000_000 {
                    return;
                }
                cxt.editor.count = NonZeroUsize::new(count);
            }
            // A non-zero digit will start the count if that number isn't used by a keymap.
            (key!(i @ '1'..='9'), None) if !self.keymaps.contains_key(mode, event) => {
                let i = i.to_digit(10).unwrap() as usize;
                cxt.editor.count = NonZeroUsize::new(i);
            }
            // special handling for repeat operator
            (key!('.'), _) if self.keymaps.pending().is_empty() => {
                if let Some(plan) = self.repeat.begin_replay() {
                    let count = cxt.editor.count.map_or(1, NonZeroUsize::into);
                    let result = plan.replay(cxt, count, |cx, key| self.insert_mode(cx, key));
                    self.repeat.end_replay();
                    if let Err(error) = result {
                        cxt.editor.set_error(|| error.to_string());
                    }
                }
                cxt.editor.count = None;
            }
            _ => {
                // set the count
                cxt.count = cxt.editor.count;
                // TODO: edge case: 0j -> reset to 1
                // if this fails, count was Some(0)
                // debug_assert!(cxt.count != 0);

                // set the register
                cxt.register = cxt.editor.selected_register.take();

                let res = self.handle_keymap_event(mode, cxt, event);
                if matches!(&res, Some(KeymapResult::NotFound)) {
                    self.on_next_key(OnKeyCallbackKind::Fallback, cxt, event);
                }
                if self.keymaps.pending().is_empty() {
                    cxt.editor.count = None
                } else {
                    cxt.editor.selected_register = cxt.register.take();
                }
            }
        }
    }

    pub fn set_completion(
        &mut self,
        editor: &mut Editor,
        items: Vec<CompletionItem>,
        session: view::features::completion::CompletionSession,
        size: Rect,
    ) -> Option<Rect> {
        let mut completion = Completion::new(editor, items, session.clone());

        if completion.is_empty() || !editor.begin_completion(&session) {
            // skip if we got no completion results
            return None;
        }

        let area = completion.area(size, editor);
        self.record_insert_event(InsertEvent::TriggerCompletion);

        // TODO : propagate required size on resize to completion too
        self.completion = Some(completion);
        Some(area)
    }

    pub fn clear_completion(&mut self, editor: &mut Editor) -> Option<OnKeyCallback> {
        self.completion = None;
        let mut on_next_key: Option<OnKeyCallback> = None;
        let applied = match editor.finish_completion() {
            Ok(applied) => applied,
            Err(error) => {
                editor.set_error(|| error.to_string());
                return None;
            }
        };
        if let Some(view::features::completion::CompletionApply {
            trigger_offset,
            changes,
            placeholder,
        }) = applied
        {
            self.record_insert_event(InsertEvent::CompletionApply {
                trigger_offset,
                changes,
            });
            on_next_key = placeholder.then_some(Box::new(|cx, key| {
                if let Some(c) = key.char() {
                    if let Err(error) = cx.editor.clear_snippet_placeholder() {
                        cx.editor.set_error(|| error.to_string());
                        return;
                    }
                    commands::insert::insert_char(cx, c);
                }
            }))
        }
        on_next_key
    }

    pub fn handle_idle_timeout(&mut self, cx: &mut commands::Context) -> EventResult {
        view::features::inlay_hints::refresh(cx.editor);

        EventResult::Ignored(None)
    }
}

/// Whether the focused doc's workspace is in restricted mode and running `trust` would
/// change something visible at the workspace level.
fn workspace_trust_indicator_visible(editor: &Editor) -> bool {
    if editor.workspace_trust.implicit_level() == workspace::trust::ImplicitTrustLevel::Insecure {
        return false;
    }
    let (_, doc) = view::current_ref!(editor);
    editor
        .workspace_trust
        .restricted_for_doc(doc.workspace_root(), doc.servers_to_load())
}

impl EditorView {
    /// must be called whenever the editor processed input that
    /// is not a `KeyEvent`. In these cases any pending keys/on next
    /// key callbacks must be canceled.
    fn handle_non_key_input(&mut self, cxt: &mut commands::Context) {
        cxt.editor.status_msg = None;
        cxt.editor.reset_idle_timer();
        // HACKS: create a fake key event that will never trigger any actual map
        // and therefore simply acts as "dismiss"
        let null_key_event = KeyEvent {
            code: KeyCode::Null,
            modifiers: KeyModifiers::empty(),
        };
        // dismiss any pending keys
        if let Some((on_next_key, _)) = self.on_next_key.take() {
            on_next_key(cxt, null_key_event);
        }
        self.handle_keymap_event(cxt.editor.mode(), cxt, null_key_event);
        self.pseudo_pending.clear();
    }

    fn handle_mouse_event(
        &mut self,
        event: &MouseEvent,
        cxt: &mut commands::Context,
    ) -> EventResult {
        if event.kind != MouseEventKind::Moved {
            self.handle_non_key_input(cxt)
        }

        let config = cxt.editor.config();
        let MouseEvent {
            kind,
            row,
            column,
            modifiers,
            ..
        } = *event;

        let pos_and_view = |editor: &Editor, row, column, ignore_virtual_text| {
            editor.tree.views().find_map(|(view, _focus)| {
                view.pos_at_screen_coords(
                    &editor.documents[&view.document_id()],
                    row,
                    column,
                    ignore_virtual_text,
                )
                .map(|pos| (pos, view.id))
            })
        };

        let gutter_coords_and_view = |editor: &Editor, row, column| {
            editor.tree.views().find_map(|(view, _focus)| {
                view.gutter_coords_at_screen_coords(
                    &editor.documents[&view.document_id()],
                    row,
                    column,
                )
                .map(|coords| (coords, view.id))
            })
        };

        match kind {
            MouseEventKind::Down(MouseButton::Left) => {
                let editor = &mut cxt.editor;

                if let Some((pos, view_id)) = pos_and_view(editor, row, column, true) {
                    editor.focus(view_id);

                    let prev_view_id = view!(editor).id;
                    let mode = editor.mode();
                    let doc = doc_mut!(editor, &view!(editor, view_id).document_id());

                    if modifiers == KeyModifiers::ALT {
                        let selection = doc.selection(view_id).clone();
                        doc.set_selection(view_id, selection.push(Range::point(pos)));
                    } else if mode == Mode::Select {
                        // Discards non-primary selections for consistent UX with normal mode
                        let primary = doc.selection(view_id).primary().put_cursor(
                            doc.text().slice(..),
                            pos,
                            true,
                        );
                        editor.mouse_down_range = Some(primary);
                        doc.set_selection(view_id, Selection::single(primary.anchor, primary.head));
                    } else {
                        doc.set_selection(view_id, Selection::point(pos));
                    }

                    if view_id != prev_view_id {
                        self.clear_completion(editor);
                    }

                    editor.ensure_cursor_in_view(view_id);

                    return EventResult::Consumed(None);
                }

                if let Some((coords, view_id)) = gutter_coords_and_view(editor, row, column) {
                    editor.focus(view_id);

                    let (view, doc) = current!(cxt.editor);

                    let Some(path) = doc.path().map(ToOwned::to_owned) else {
                        return EventResult::Ignored(None);
                    };

                    if let Some(char_idx) =
                        view.pos_at_visual_coords(doc, coords.row as u16, coords.col as u16, true)
                    {
                        let line = doc.text().char_to_line_idx(char_idx);
                        commands::dap_toggle_breakpoints_impl(cxt, path, &[line]);
                        return EventResult::Consumed(None);
                    }
                }

                EventResult::Ignored(None)
            }

            MouseEventKind::Drag(MouseButton::Left) => {
                let (view, doc) = current!(cxt.editor);

                let pos = match view.pos_at_screen_coords(doc, row, column, true) {
                    Some(pos) => pos,
                    None => return EventResult::Ignored(None),
                };

                let mut selection = doc.selection(view.id).clone();
                let primary = selection.primary_mut();
                *primary = primary.put_cursor(doc.text().slice(..), pos, true);
                doc.set_selection(view.id, selection);
                let view_id = view.id;
                cxt.editor.ensure_cursor_in_view(view_id);
                EventResult::Consumed(None)
            }

            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                let current_view = cxt.editor.tree.focus();

                let direction = match event.kind {
                    MouseEventKind::ScrollUp => Direction::Backward,
                    MouseEventKind::ScrollDown => Direction::Forward,
                    _ => unreachable!(),
                };

                match pos_and_view(cxt.editor, row, column, false) {
                    Some((_, view_id)) => cxt.editor.tree.set_focus(view_id),
                    None => return EventResult::Ignored(None),
                }

                let offset = config.scroll.wheel_lines.unsigned_abs();
                commands::scroll(cxt, offset, direction, false);

                cxt.editor.tree.set_focus(current_view);
                cxt.editor.ensure_cursor_in_view(current_view);

                EventResult::Consumed(None)
            }

            MouseEventKind::Up(MouseButton::Left) => {
                if !config.mouse.middle_click_paste {
                    return EventResult::Ignored(None);
                }

                let (view, doc) = current!(cxt.editor);

                let should_yank = match cxt.editor.mouse_down_range.take() {
                    Some(down_range) => doc.selection(view.id).primary() != down_range,
                    None => {
                        // This should not happen under normal cases. We fall back to the original
                        // behavior of yanking on non-single-char selections.
                        doc.selection(view.id)
                            .primary()
                            .slice(doc.text().slice(..))
                            .len_chars()
                            > 1
                    }
                };

                if should_yank {
                    commands::yank_main_selection_to_register(
                        cxt.editor,
                        config.mouse.yank_register,
                    );
                    EventResult::Consumed(None)
                } else {
                    EventResult::Ignored(None)
                }
            }

            MouseEventKind::Up(MouseButton::Right) => {
                if let Some((pos, view_id)) = gutter_coords_and_view(cxt.editor, row, column) {
                    cxt.editor.focus(view_id);

                    if let Some((pos, _)) = pos_and_view(cxt.editor, row, column, true) {
                        doc_mut!(cxt.editor).set_selection(view_id, Selection::point(pos));
                    } else {
                        let (view, doc) = current!(cxt.editor);

                        if let Some(pos) = view.pos_at_visual_coords(doc, pos.row as u16, 0, true) {
                            doc.set_selection(view_id, Selection::point(pos));
                            match modifiers {
                                KeyModifiers::ALT => {
                                    commands::MappableCommand::dap_edit_log.execute(cxt)
                                }
                                _ => commands::MappableCommand::dap_edit_condition.execute(cxt),
                            };
                        }
                    }

                    cxt.editor.ensure_cursor_in_view(view_id);
                    return EventResult::Consumed(None);
                }
                EventResult::Ignored(None)
            }

            MouseEventKind::Up(MouseButton::Middle) => {
                let editor = &mut cxt.editor;
                if !config.mouse.middle_click_paste {
                    return EventResult::Ignored(None);
                }

                if modifiers == KeyModifiers::ALT {
                    commands::replace_selections_with_register(
                        cxt.editor,
                        config.mouse.yank_register,
                        cxt.count(),
                    );

                    return EventResult::Consumed(None);
                }

                if let Some((pos, view_id)) = pos_and_view(editor, row, column, true) {
                    let doc = doc_mut!(editor, &view!(editor, view_id).document_id());
                    doc.set_selection(view_id, Selection::point(pos));
                    cxt.editor.focus(view_id);

                    commands::paste(
                        cxt.editor,
                        config.mouse.yank_register,
                        editor_core::transform::PastePosition::Before,
                        cxt.count(),
                    );

                    return EventResult::Consumed(None);
                }

                EventResult::Ignored(None)
            }

            _ => EventResult::Ignored(None),
        }
    }
    fn on_next_key(
        &mut self,
        kind: OnKeyCallbackKind,
        ctx: &mut commands::Context,
        event: KeyEvent,
    ) -> bool {
        match self.on_next_key.take() {
            Some((on_next_key, kind_)) => {
                if kind == kind_ {
                    on_next_key(ctx, event);
                    true
                } else {
                    self.on_next_key = Some((on_next_key, kind_));
                    false
                }
            }
            _ => false,
        }
    }
}

impl Component for EditorView {
    fn handle_event(
        &mut self,
        event: &Event,
        context: &mut crate::compositor::Context,
    ) -> EventResult {
        let mut cx = commands::Context {
            config: context.config,
            editor: context.editor,
            count: None,
            register: None,
            callback: Vec::new(),
            on_next_key_callback: None,
            jobs: context.jobs,
        };

        // Lifecycle updates may be queued while input is already ready. Never let
        // a stale menu consume input after its shared session was invalidated.
        if self.completion.is_some() && !view::features::completion::is_active(cx.editor) {
            self.clear_completion(cx.editor);
        }

        match event {
            Event::Paste(contents) => {
                self.handle_non_key_input(&mut cx);
                cx.count = cx.editor.count;
                commands::paste_bracketed_value(&mut cx, contents.clone());
                cx.editor.count = None;

                let config = cx.editor.config();
                let mode = cx.editor.mode();
                let (view, doc) = current!(cx.editor);
                view.ensure_cursor_in_view(doc, config.scroll.padding);

                // Store a history state if not in insert mode. Otherwise wait till we exit insert
                // to include any edits to the paste in the history state.
                if mode != Mode::Insert {
                    view::editing::commit(doc, view);
                }

                EventResult::Consumed(None)
            }
            Event::Resize(_width, _height) => {
                // Ignore this event, we handle resizing just before rendering to screen.
                // Handling it here but not re-rendering will cause flashing
                EventResult::Consumed(None)
            }
            &Event::Key(mut key) => {
                cx.editor.reset_idle_timer();
                canonicalize_key(&mut key);
                if cx.jobs.command_expansion.is_running()
                    && matches!(key, key!(Esc) | crate::ctrl!('c'))
                {
                    cx.jobs.command_expansion.cancel();
                    cx.editor.set_status("Command cancelled");
                    return EventResult::Consumed(None);
                }

                // clear status
                cx.editor.status_msg = None;

                let mode = cx.editor.mode();

                if !self.on_next_key(OnKeyCallbackKind::PseudoPending, &mut cx, key) {
                    match mode {
                        Mode::Insert => {
                            // let completion swallow the event if necessary
                            let mut consumed = false;
                            if let Some(completion) = &mut self.completion {
                                let res = {
                                    // use a fake context here
                                    let mut cx = Context {
                                        config: cx.config,
                                        editor: cx.editor,
                                        jobs: cx.jobs,
                                        scroll: None,
                                        image_picker: None,
                                        is_cursor_owner: false,
                                    };

                                    match completion.handle_event(event, &mut cx) {
                                        EventResult::Consumed(callback) => {
                                            consumed = true;
                                            Some(callback)
                                        }
                                        _ => match completion.accept(&mut cx) {
                                            EventResult::Consumed(callback) => Some(callback),
                                            _ => None,
                                        },
                                    }
                                };

                                if let Some(callback) = res
                                    && callback.is_some()
                                {
                                    // assume close_fn
                                    if let Some(cb) = self.clear_completion(cx.editor) {
                                        if consumed {
                                            cx.on_next_key_callback =
                                                Some((cb, OnKeyCallbackKind::Fallback))
                                        } else {
                                            self.on_next_key =
                                                Some((cb, OnKeyCallbackKind::Fallback));
                                        }
                                    }
                                }
                            }

                            // if completion didn't take the event, we pass it onto commands
                            if !consumed {
                                match self.insert_mode(&mut cx, key) {
                                    Ok(()) => self.record_insert_event(InsertEvent::Key(key)),
                                    Err(error) => cx.editor.set_error(|| error.to_string()),
                                }
                            }
                        }
                        mode => self.command_mode(mode, &mut cx, key),
                    }
                }

                self.on_next_key = cx.on_next_key_callback.take();
                match self.on_next_key {
                    Some((_, OnKeyCallbackKind::PseudoPending)) => self.pseudo_pending.push(key),
                    _ => self.pseudo_pending.clear(),
                }

                // appease borrowck
                let callbacks = take(&mut cx.callback);

                // if the command consumed the last view, skip the render.
                // on the next loop cycle the Application will then terminate.
                if cx.editor.should_close() {
                    return EventResult::Ignored(None);
                }

                let config = cx.editor.config();
                let mode = cx.editor.mode();
                let (view, doc) = current!(cx.editor);

                view.ensure_cursor_in_view(doc, config.scroll.padding);

                // Store a history state if not in insert mode. This also takes care of
                // committing changes when leaving insert mode.
                if mode != Mode::Insert {
                    view::editing::commit(doc, view);
                }
                let callback = if callbacks.is_empty() {
                    None
                } else {
                    let callback: crate::compositor::Callback = Box::new(move |compositor, cx| {
                        for callback in callbacks {
                            callback(compositor, cx)
                        }
                    });
                    Some(callback)
                };

                EventResult::Consumed(callback)
            }

            Event::Mouse(event) => self.handle_mouse_event(event, &mut cx),
            Event::IdleTimeout => self.handle_idle_timeout(&mut cx),
            Event::FocusGained => {
                self.terminal_focused = true;
                view::features::auto_reload::check_unwatched(context.editor);
                EventResult::Consumed(None)
            }
            Event::FocusLost => {
                view::features::auto_save::focus_lost(context.editor);
                self.terminal_focused = false;
                EventResult::Consumed(None)
            }
        }
    }

    fn render(&mut self, area: Rect, surface: &mut Surface, cx: &mut Context) {
        self.spinners
            .synchronize(cx.editor.progressing_language_servers());
        // clear with background color
        surface.set_style(area, cx.editor.theme.get("ui.background"));
        let config = cx.editor.config();

        // check if bufferline should be rendered
        let use_bufferline = match config.bufferline.show {
            BufferLine::Always => true,
            BufferLine::Multiple if cx.editor.documents.len() > 1 => true,
            _ => false,
        };

        let layout = ApplicationLayout::new(area, use_bufferline, config.statusline.global);

        // if the terminal size suddenly changed, we need to trigger a resize
        cx.editor.resize(layout.editor);
        cx.editor.refresh_debugger_inline_values();

        if let Some(bufferline) = layout.bufferline {
            Self::render_bufferline(cx.editor, bufferline, surface);
        }

        self.images.retain(|id, _| {
            cx.editor.tree.try_get(*id).is_some_and(|view| {
                cx.editor
                    .document(view.document_id())
                    .is_some_and(Document::is_binary)
            })
        });
        for (view, is_focused) in cx.editor.tree.views() {
            let doc = cx.editor.document(view.document_id()).unwrap();
            let content = view.content_area(doc);
            if doc.is_binary() && !content.is_empty() {
                let style = cx.editor.theme.get("ui.text");
                if let Some(picker) = cx.image_picker {
                    let size = Size::new(content.width, content.height.saturating_sub(2).max(1));
                    let image = self
                        .images
                        .entry(view.id)
                        .or_insert_with(|| ImageDocumentView::new(doc, picker.clone(), size));
                    if !image.matches(doc, size) {
                        *image = ImageDocumentView::new(doc, picker.clone(), size);
                    }
                    image.render(content, surface, style);
                } else {
                    render_placeholder("<Binary file>", content, surface, style);
                }
            }
            self.render_view(cx, doc, view, area, surface, is_focused);
            let mut context = statusline::RenderContext::new(cx.editor, doc, view, &self.spinners);
            statusline::render(&mut context, view.statusline_area(doc), surface);
        }

        if layout.statusline.height > 0 {
            let (view, doc) = current_ref!(cx.editor);
            let mut context = statusline::RenderContext::new(cx.editor, doc, view, &self.spinners);
            statusline::render(&mut context, layout.statusline, surface);
        }

        if config.auto_info
            && let Some(mut info) = cx.editor.autoinfo.take()
        {
            // Global statuslines are already outside layout.editor; per-view
            // statuslines occupy its bottom row.
            let info_area = layout
                .editor
                .clip_bottom(u16::from(!config.statusline.global));
            info.render(info_area, surface, cx);
            cx.editor.autoinfo = Some(info)
        }

        let key_width = 15u16; // for showing pending keys
        let status_msg_width = cx
            .editor
            .status_msg
            .as_ref()
            .map_or(0, |(status_msg, _)| status_msg.width());
        let macro_width = if cx.editor.macro_recording.is_some() {
            3
        } else {
            0
        };
        let restricted = workspace_trust_indicator_visible(cx.editor);
        let trust_width = if restricted { 3 } else { 0 };
        let show_pending = layout
            .commandline
            .width
            .saturating_sub(status_msg_width as u16)
            > key_width;
        let (status_area, pending_area, trust_area, macro_area) = if show_pending {
            let [status_area, pending_area, trust_area, macro_area] = Layout::horizontal([
                Constraint::Min(0),
                Constraint::Length(key_width),
                Constraint::Length(trust_width),
                Constraint::Length(macro_width),
            ])
            .areas(layout.commandline);
            (status_area, pending_area, trust_area, macro_area)
        } else {
            (
                layout.commandline,
                Rect::default(),
                Rect::default(),
                Rect::default(),
            )
        };

        // render status msg
        if let Some((status_msg, severity)) = &cx.editor.status_msg {
            use ::diagnostics::Severity;
            let style = if *severity == Severity::Error {
                cx.editor.theme.get("error")
            } else if *severity == Severity::Warning {
                cx.editor.theme.get("warning")
            } else {
                cx.editor.theme.get("ui.text")
            };

            Paragraph::new(status_msg.as_ref())
                .style(style)
                .render(status_area, surface);
        }

        if show_pending {
            let mut disp = String::new();
            if let Some(count) = cx.editor.count {
                disp.push_str(&count.to_string())
            }
            for key in self.keymaps.pending() {
                disp.push_str(&key.key_sequence_format());
            }
            for key in &self.pseudo_pending {
                disp.push_str(&key.key_sequence_format());
            }
            let style = cx.editor.theme.get("ui.text");
            let disp = disp
                .get(disp.len().saturating_sub(key_width as usize)..)
                .unwrap_or(&disp);
            Paragraph::new(disp)
                .style(style)
                .alignment(Alignment::Right)
                .render(pending_area, surface);

            if restricted {
                let style = style
                    .fg(view::graphics::Color::Yellow)
                    .add_modifier(Modifier::BOLD);
                Paragraph::new("[⚠]")
                    .style(style)
                    .render(trust_area, surface);
            }
            if let Some((reg, _)) = cx.editor.macro_recording {
                let disp = format!("[{}]", reg);
                let style = style
                    .fg(view::graphics::Color::Yellow)
                    .add_modifier(Modifier::BOLD);
                Paragraph::new(disp)
                    .style(style)
                    .render(macro_area, surface);
            }
        }

        if let Some(completion) = self.completion.as_mut() {
            completion.render(area, surface, cx);
        }
    }

    fn owns_cursor(&self) -> bool {
        true
    }

    fn cursor(&self, _area: Rect, editor: &Editor) -> Cursor {
        if current_ref!(editor).1.is_binary() {
            return Cursor::Hidden;
        }
        let (pos, kind) = editor.cursor();
        let Some(pos) = pos else {
            return Cursor::Hidden;
        };
        let pos = (pos.col as u16, pos.row as u16).into();
        if primary_cursor_is_software(kind, self.terminal_focused, true) {
            Cursor::Software(pos)
        } else {
            // Let the terminal provide its unfocused cursor appearance.
            let kind = if kind == CursorKind::Block {
                CursorKind::Underline
            } else {
                kind
            };
            Cursor::Native(pos, kind)
        }
    }
}

fn canonicalize_key(key: &mut KeyEvent) {
    if let KeyEvent {
        code: KeyCode::Char(_),
        modifiers: _,
    } = key
    {
        key.modifiers.remove(KeyModifiers::SHIFT)
    }
}
use tui::buffer::BufferExt as _;
use view::graphics::RectExt as _;
