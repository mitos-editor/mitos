//! Pane drawing and image caches, separate from terminal input dispatch.

use super::super::{
    document::{render_document, LinePos, TextRenderer},
    highlights::{self},
    image::{render_placeholder, ImageDocumentView},
    layout::ApplicationLayout,
    statusline,
    text_decorations::{self, Decoration, DecorationManager, InlineDiagnostics},
    ProgressSpinners,
};
use diagnostics::NumberOrString;
use editor_config::{DiagnosticFilter, RulerStyle};
use editor_core::unicode::width::UnicodeWidthStr;
use loader::VERSION_AND_GIT_HASH;
use std::{collections::HashMap, rc::Rc, sync::LazyLock};
use stdx::rope::RopeExt as _;
use text_layout::{visual_offset_from_block, DisplayPosition};
use theme::{icons::ICONS, Theme};
use tui::{
    buffer::{Buffer as Surface, BufferExt as _},
    layout::{Alignment, Constraint, Layout, Size},
    text::{Line, Span},
    widgets::{Paragraph, Row, StatefulWidget, Table, TableState, Widget},
};
use view::{
    document::Mode,
    graphics::{Color, Modifier, Rect, RectExt as _, Style},
    Document, Editor, View, ViewId,
};

#[derive(Clone, Copy)]
pub(super) struct PaneOptions {
    pub area: Rect,
    pub terminal_focused: bool,
    pub is_cursor_owner: bool,
    pub true_color: bool,
}

#[derive(Default)]
pub(super) struct PaneRenderer {
    spinners: ProgressSpinners,
    images: HashMap<ViewId, ImageDocumentView>,
}

impl PaneRenderer {
    pub fn render(
        &mut self,
        editor: &Editor,
        layout: ApplicationLayout,
        surface: &mut Surface,
        image_picker: Option<&ratatui_image::picker::Picker>,
        options: PaneOptions,
    ) {
        self.spinners
            .synchronize(editor.progressing_language_servers());
        surface.set_style(options.area, editor.theme.get("ui.background"));
        if let Some(bufferline) = layout.bufferline {
            Self::render_bufferline(editor, bufferline, surface);
        }

        self.images.retain(|id, _| {
            editor.tree.try_get(*id).is_some_and(|view| {
                editor
                    .document(view.document_id())
                    .is_some_and(Document::is_binary)
            })
        });
        for (view, is_focused) in editor.tree.views() {
            let doc = editor.document(view.document_id()).unwrap();
            let content = view.content_area(doc);
            if doc.is_binary() && !content.is_empty() {
                let style = editor.theme.get("ui.text");
                if let Some(picker) = image_picker {
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
            Self::render_view(
                editor,
                options,
                editor.completion_presentation(doc),
                view,
                options.area,
                surface,
                is_focused,
            );
            let mut context = statusline::RenderContext::new(editor, doc, view, &self.spinners);
            statusline::render(&mut context, view.statusline_area(doc), surface);
        }

        if layout.statusline.height > 0 {
            let (view, doc) = current_ref!(editor);
            let mut context = statusline::RenderContext::new(editor, doc, view, &self.spinners);
            statusline::render(&mut context, layout.statusline, surface);
        }
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
        let table = Table::new(
            rows,
            [
                Constraint::Length(command_width),
                Constraint::Length(description_width),
            ],
        )
        .column_spacing(COMMAND_SPACING)
        .flex(tui::layout::Flex::Legacy);
        StatefulWidget::render(table, commands_area, surface, &mut TableState::default());

        Paragraph::new(footer.to_vec())
            .alignment(Alignment::Center)
            .render(footer_area, surface);
    }

    #[allow(clippy::too_many_lines)]
    fn render_view(
        editor: &Editor,
        options: PaneOptions,
        doc: &Document,
        view: &View,
        viewport: Rect,
        surface: &mut Surface,
        is_focused: bool,
    ) {
        if doc.is_binary() {
            Self::render_view_border(editor, view, viewport, surface);
            return;
        }
        let layout = view.layout(doc, Some(&editor.theme));
        let inner = layout.viewport;
        let area = view.content_area(doc);
        let theme = &editor.theme;
        let config = editor.config();

        let mut decorations = DecorationManager::default();
        if doc.review().is_some() {
            decorations.add_decoration(crate::ui::change_review::document::line_backgrounds(
                doc, theme,
            ));
        }

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

        let mut overlays = Vec::new();
        if let Some(highlights) = crate::ui::change_review::document::highlights(
            doc,
            layout.offset.anchor,
            inner.height,
            theme,
        ) {
            overlays.push(highlights);
        }

        if doc
            .language_config()
            .and_then(|config| config.rainbow_brackets)
            .unwrap_or(config.rainbow_brackets.enable)
            && let Some(overlay) = highlights::rainbow_highlights(doc, &layout, theme)
        {
            overlays.push(overlay);
        }

        if let Some(overlay) = highlights::document_link_highlights(doc, theme) {
            overlays.push(overlay);
        }

        overlays.push(highlights::overlay_highlights(&layout));

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
                &config.cursor,
                options.terminal_focused,
                options.is_cursor_owner,
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
                is_focused & options.terminal_focused,
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
            editor
                .debugger_frame_in_document(doc)
                .map_or(&[], |_| editor.debugger_inline_values(view.id)),
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
        render_document(surface, doc, &layout, overlays, theme, decorations);

        if editor.config().welcome_screen && doc.version() == 0 && doc.is_welcome {
            Self::render_welcome(theme, inner, surface, options.true_color);
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

    fn render_rulers(
        editor: &Editor,
        doc: &Document,
        view: &View,
        viewport: Rect,
        surface: &mut Surface,
        theme: &Theme,
    ) {
        let config = editor.config();
        let editor_rulers = &config.rulers.columns;
        let ruler_theme = theme.get("ui.virtual.ruler");
        let (character, ruler_style) = match config.rulers.style {
            RulerStyle::Background => (
                ' ',
                Style::default().bg(ruler_theme.bg.unwrap_or(Color::Red)),
            ),
            RulerStyle::Glyph => (
                config.rulers.character,
                Style::default().fg(ruler_theme.fg.or(ruler_theme.bg).unwrap_or(Color::Red)),
            ),
        };

        let rulers = doc
            .language_config()
            .and_then(|config| config.rulers.as_ref())
            .unwrap_or(editor_rulers);

        let view_offset = doc.view_offset(view.id);

        let mut character_buffer = [0; 4];
        let character: &str = character.encode_utf8(&mut character_buffer);
        let ruler_cols = rulers
            .iter()
            // Convert one-based document columns to viewport columns without
            // truncating large horizontal offsets to u16.
            .filter_map(|&ruler| {
                usize::from(ruler)
                    .checked_sub(1)?
                    .checked_sub(view_offset.horizontal_offset)
            })
            .filter(|&ruler| ruler < usize::from(viewport.width))
            .map(|ruler| viewport.x + ruler as u16);

        for col in ruler_cols {
            for y in viewport.top()..viewport.bottom() {
                surface.set_stringn(col, y, character, 1, ruler_style);
            }
        }
    }

    fn render_bufferline(editor: &Editor, viewport: Rect, surface: &mut Surface) {
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
        let labels = editor.document_tab_labels();
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

                let fname = &labels[&doc.id()];

                let style = if current_doc == doc.id() {
                    bufferline_active
                } else {
                    bufferline_inactive
                };

                let source_path = doc.display_metadata().source_path;
                let show_file_icon = editor.config().icons && source_path.is_some();
                let modified = if doc.is_modified() { "[+]" } else { "" };

                if show_file_icon && let Some(path) = source_path {
                    let icons = ICONS.load();
                    if let Some(file) = icons.fs().file() {
                        let icon =
                            file.get_with_active_style_or_default(path, &editor.theme, style);
                        return Line::from(vec![
                            super::super::icon_span(icon),
                            Span::styled(format!("{fname}{modified} "), style),
                        ]);
                    }
                }

                Line::styled(format!(" {fname}{modified} "), style)
            })
            .collect();

        super::super::bufferline::render(&titles, current_tab, viewport, surface);
    }

    #[allow(clippy::too_many_lines)]
    fn render_breadcrumb(
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
                x = draw_element(surface, viewport, x, &doc.display_name(), style);
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

    fn render_gutter<'d>(
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

        for &gutter_type in view.gutters_for(doc) {
            let mut gutter =
                super::super::gutter::style(gutter_type, editor, doc, view, theme, is_focused);
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

    fn render_diagnostics(
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
    fn cursorline(doc: &Document, view: &View, theme: &Theme) -> impl Decoration + use<> {
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
    fn highlight_cursorcolumn(
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
}
