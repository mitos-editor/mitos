//! Plain-text and Markdown tooltips for complete, structured inlay hints.

use crate::{
    compositor::{Component, Compositor, Context, DrawContext},
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
    features::inlay_hints::{self, InlayHintTarget, ResolvedInlayHint},
    graphics::Rect,
    Editor,
};

pub(crate) fn request(compositor: &mut Compositor, cx: &mut Context, target: InlayHintTarget) {
    let Some(future) = inlay_hints::resolve(cx.editor, target.clone()) else {
        return;
    };
    let mut lifetime = event::TaskController::new();
    let pending = lifetime.restart();
    let loading = PendingHint {
        target,
        _lifetime: lifetime,
        text: ui::Text::new("Loading inlay hint…".into()),
    };
    compositor.replace_or_push(
        PendingHint::ID,
        Popup::new(PendingHint::ID, loading)
            .with_border(ui::PopupBorder::Hover)
            .auto_close(true),
    );
    let jobs = cx.jobs.sender();
    tokio::spawn(async move {
        let resolved = event::cancelable_future(future, &pending).await.flatten();
        jobs.send_callback(Callback::ui(move |compositor, cx| {
            // Dropping or replacing the loading layer also invalidates replies
            // that have already reached the frontend callback queue.
            if pending.is_canceled() {
                return;
            }
            compositor.remove(PendingHint::ID);
            let Some(resolved) = resolved else { return };
            if !resolved.is_current(cx.editor) {
                return;
            }
            resolved.cache(cx.editor);
            let details = HintDetails::new(resolved, cx.editor);
            compositor.replace_or_push(
                HintDetails::ID,
                Popup::new(HintDetails::ID, details)
                    .with_border(ui::PopupBorder::Hover)
                    .auto_close(true),
            );
        }))
        .await;
    });
}

struct PendingHint {
    target: InlayHintTarget,
    // This frontend input lifetime ends on Escape, unbound input, replacement,
    // or application shutdown, independently of the document's request lifetime.
    _lifetime: event::TaskController,
    text: ui::Text,
}

impl PendingHint {
    const ID: &'static str = "inlay-hint-loading";
}

impl Component for PendingHint {
    fn update(&mut self, _: Rect, cx: &mut Context) {
        if !self.target.is_current(cx.editor) {
            cx.jobs.sender().enqueue(Callback::ui(|compositor, cx| {
                if compositor
                    .find_id::<Popup<PendingHint>>(PendingHint::ID)
                    .is_some_and(|popup| !popup.contents().target.is_current(cx.editor))
                {
                    compositor.remove(PendingHint::ID);
                }
            }));
        }
    }

    fn render(&mut self, area: Rect, surface: &mut Buffer, cx: &mut DrawContext) {
        self.text
            .render(super::panel::content_area(area), surface, cx);
    }

    fn required_size(&mut self, viewport: (u16, u16)) -> Option<(u16, u16)> {
        self.text.required_size(viewport).map(|(width, height)| {
            (
                width + super::panel::HORIZONTAL_INSET,
                height + super::panel::VERTICAL_INSET,
            )
        })
    }
}

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
