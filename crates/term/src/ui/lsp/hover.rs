use crate::compositor::DrawContext;
use std::sync::Arc;

use arc_swap::ArcSwap;
use language::syntax;
use lsp_client::lsp;
use tui::buffer::Buffer;
use tui::layout::{Constraint, Layout};
use tui::widgets::{Paragraph, Widget, Wrap};
use ui_core::input::Event;
use view::graphics::Rect;

use crate::compositor::{Component, Context, EventResult};

use crate::ui::{panel, Markdown};

pub struct Hover {
    page: super::PageSelection,
    contents: Vec<(Option<Markdown>, Markdown)>,
}

impl Hover {
    pub const ID: &'static str = "hover";

    pub fn new(
        hovers: Vec<(String, lsp::Hover)>,
        config_loader: Arc<ArcSwap<syntax::Loader>>,
    ) -> Self {
        let n_hovers = hovers.len();
        let contents = hovers
            .into_iter()
            .enumerate()
            .map(|(idx, (server_name, hover))| {
                let header = (n_hovers > 1).then(|| {
                    Markdown::new(
                        format!("**[{}/{}] {}**", idx + 1, n_hovers, server_name),
                        config_loader.clone(),
                    )
                });
                let body = Markdown::new(
                    hover_contents_to_string(hover.contents),
                    config_loader.clone(),
                );
                (header, body)
            })
            .collect();

        Self {
            page: super::PageSelection::default(),
            contents,
        }
    }

    fn has_header(&self) -> bool {
        self.contents.len() > 1
    }

    fn content(&self) -> &(Option<Markdown>, Markdown) {
        &self.contents[self.page.index]
    }
}

const PADDING_HORIZONTAL: u16 = 2;
const PADDING_TOP: u16 = 1;
const PADDING_BOTTOM: u16 = 1;
const HEADER_HEIGHT: u16 = 1;
const SEPARATOR_HEIGHT: u16 = 1;

impl Component for Hover {
    fn render(&mut self, area: Rect, surface: &mut Buffer, cx: &mut DrawContext) {
        let area = panel::content_area(area);

        let (header, contents) = self.content();

        // show header and border only when more than one results
        let contents_area = if let Some(header) = header {
            let [header_area, separator_area, contents_area] = Layout::vertical([
                Constraint::Length(HEADER_HEIGHT),
                Constraint::Length(SEPARATOR_HEIGHT),
                Constraint::Min(0),
            ])
            .areas(area);
            // header LSP Name
            let header = header.parse(Some(&cx.editor.theme));
            let header = Paragraph::new(header);
            header.render(header_area, surface);

            panel::top_border(
                cx.editor.theme.get("ui.window"),
                panel::PopupBorder::Hover.resolve(&cx.editor.config()),
            )
            .render(separator_area, surface);
            contents_area
        } else {
            area
        };

        // hover content
        let contents = contents.parse(Some(&cx.editor.theme));
        let contents_para = Paragraph::new(contents)
            .wrap(Wrap { trim: false })
            .scroll((cx.scroll.unwrap_or_default() as u16, 0));
        contents_para.render(contents_area, surface);
    }

    fn required_size(&mut self, viewport: (u16, u16)) -> Option<(u16, u16)> {
        let max_text_width = viewport.0.saturating_sub(PADDING_HORIZONTAL).clamp(10, 120);

        let (header, contents) = self.content();

        let header_width = header
            .as_ref()
            .map(|header| {
                let header = crate::ui::text::paragraph(header.parse(None));
                let (width, _height) = crate::ui::text::required_size(&header, max_text_width);
                width
            })
            .unwrap_or_default();

        let contents = crate::ui::text::paragraph(contents.parse(None));
        let (content_width, content_height) =
            crate::ui::text::required_size(&contents, max_text_width);

        let width = PADDING_HORIZONTAL + header_width.max(content_width);
        let height = if self.has_header() {
            PADDING_TOP + HEADER_HEIGHT + SEPARATOR_HEIGHT + content_height + PADDING_BOTTOM
        } else {
            PADDING_TOP + content_height + PADDING_BOTTOM
        };

        Some((width, height))
    }

    fn has_pending_keys(&self) -> bool {
        self.page.keys.is_pending()
    }

    fn handle_event(&mut self, event: &Event, cx: &mut Context) -> EventResult {
        self.page.handle_event(
            event,
            &cx.config.current.component_keys.hover,
            self.contents.len(),
        )
    }
}

fn hover_contents_to_string(contents: lsp::HoverContents) -> String {
    fn marked_string_to_markdown(contents: lsp::MarkedString) -> String {
        match contents {
            lsp::MarkedString::String(contents) => contents,
            lsp::MarkedString::LanguageString(string) => {
                if string.language == "markdown" {
                    string.value
                } else {
                    format!("```{}\n{}\n```", string.language, string.value)
                }
            }
        }
    }
    match contents {
        lsp::HoverContents::Scalar(contents) => marked_string_to_markdown(contents),
        lsp::HoverContents::Array(contents) => contents
            .into_iter()
            .map(marked_string_to_markdown)
            .collect::<Vec<_>>()
            .join("\n\n"),
        lsp::HoverContents::Markup(contents) => contents.value,
    }
}
