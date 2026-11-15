//! Plain-text and Markdown tooltips for complete, structured inlay hints.

use crate::{
    compositor::{Component, DrawContext},
    job::Callback,
    ui::{self, Popup},
};
use lsp_client::lsp;
use tui::{
    buffer::Buffer,
    text::{Line, Text},
    widgets::Widget,
};
use view::{
    features::inlay_hints::{self, ResolvedInlayHint},
    graphics::Rect,
    Editor,
};

enum Content {
    Plain(String),
    Markdown(ui::Markdown),
}

pub(crate) struct HintDetails {
    resolved: ResolvedInlayHint,
    sections: Vec<Content>,
}

impl HintDetails {
    pub(crate) const ID: &'static str = "inlay-hint-details";

    pub(crate) fn new(resolved: ResolvedInlayHint, editor: &Editor) -> Self {
        let mut sections = vec![Content::Plain(inlay_hints::label(&resolved.hint().label))];
        let content = |kind, value: String| {
            if kind == lsp::MarkupKind::Markdown {
                Content::Markdown(ui::Markdown::new(value, editor.syn_loader.clone()))
            } else {
                Content::Plain(value)
            }
        };
        if let Some(tooltip) = &resolved.hint().tooltip {
            sections.push(match tooltip {
                lsp::InlayHintTooltip::String(value) => Content::Plain(value.clone()),
                lsp::InlayHintTooltip::MarkupContent(markup) => {
                    content(markup.kind.clone(), markup.value.clone())
                }
            });
        }
        if let lsp::InlayHintLabel::LabelParts(parts) = &resolved.hint().label {
            for part in parts {
                if let Some(tooltip) = &part.tooltip {
                    sections.push(Content::Plain(part.value.clone()));
                    sections.push(match tooltip {
                        lsp::InlayHintLabelPartTooltip::String(value) => {
                            Content::Plain(value.clone())
                        }
                        lsp::InlayHintLabelPartTooltip::MarkupContent(markup) => {
                            content(markup.kind.clone(), markup.value.clone())
                        }
                    });
                }
            }
        }
        Self { resolved, sections }
    }

    fn text(&self, theme: Option<&theme::Theme>) -> Text<'_> {
        let mut lines = Vec::new();
        for section in &self.sections {
            if !lines.is_empty() {
                lines.push(Line::default());
            }
            lines.extend(
                match section {
                    Content::Plain(text) => Text::from(text.as_str()),
                    Content::Markdown(markdown) => markdown.parse(theme),
                }
                .lines,
            );
        }
        Text::from(lines)
    }
}

impl Component for HintDetails {
    fn update(&mut self, _: Rect, cx: &mut crate::compositor::Context) {
        if !self.resolved.is_current(cx.editor) {
            cx.jobs.sender().enqueue(Callback::ui(|compositor, cx| {
                if compositor
                    .find_id::<Popup<HintDetails>>(HintDetails::ID)
                    .is_some_and(|popup| !popup.contents().resolved.is_current(cx.editor))
                {
                    compositor.remove(HintDetails::ID);
                }
            }));
        }
    }

    fn render(&mut self, area: Rect, surface: &mut Buffer, cx: &mut DrawContext) {
        super::text::paragraph(self.text(Some(&cx.editor.theme)))
            .scroll((cx.scroll.unwrap_or_default() as u16, 0))
            .render(super::panel::content_area(area), surface);
    }

    fn required_size(&mut self, viewport: (u16, u16)) -> Option<(u16, u16)> {
        let paragraph = super::text::paragraph(self.text(None));
        let (width, height) = super::text::required_size(
            &paragraph,
            viewport
                .0
                .saturating_sub(super::panel::HORIZONTAL_INSET)
                .min(120),
        );
        Some((
            width + super::panel::HORIZONTAL_INSET,
            height + super::panel::VERTICAL_INSET,
        ))
    }
}
