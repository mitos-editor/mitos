//! Details and explicit actions for complete, structured inlay hints.

use std::sync::Arc;

use crate::{
    compositor::{Component, Compositor, Context, DrawContext, Event, EventResult},
    job::Callback,
    ui::{self, Popup, PromptEvent},
};
use lsp_client::lsp;
use tui::{
    buffer::Buffer,
    text::{Line, Text},
    widgets::{Row, Widget},
};
use view::{
    editor::Action,
    features::inlay_hints::{self, InlayHintAction, InlayHintTarget, ResolvedInlayHint},
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
            present(compositor, cx, Arc::new(resolved), None);
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

/// Present a hint, optionally limiting label-part actions to the clicked part.
pub(crate) fn present(
    compositor: &mut Compositor,
    cx: &mut Context,
    resolved: Arc<ResolvedInlayHint>,
    part: Option<usize>,
) {
    if !resolved.is_current(cx.editor) {
        return;
    }
    let actions: Vec<_> = resolved
        .actions()
        .into_iter()
        .filter(|action| match action {
            InlayHintAction::LabelLocation(index) | InlayHintAction::LabelCommand(index) => {
                part.is_none_or(|part| part == *index)
            }
            InlayHintAction::ApplyTextEdits => true,
        })
        .collect();
    if actions.is_empty() {
        show_details(compositor, cx.editor, resolved);
        return;
    }
    let mut items = vec![HintMenuItem {
        title: "Show details".into(),
        action: None,
    }];
    for action in actions {
        let title = match action {
            InlayHintAction::LabelLocation(index) => {
                format!("Go to {}", part_label(&resolved, index))
            }
            InlayHintAction::LabelCommand(index) => {
                let lsp::InlayHintLabel::LabelParts(parts) = &resolved.hint().label else {
                    unreachable!()
                };
                format!(
                    "Run {} ({})",
                    parts[index].command.as_ref().unwrap().title,
                    part_label(&resolved, index)
                )
            }
            InlayHintAction::ApplyTextEdits => "Apply hint text edits".into(),
        };
        items.push(HintMenuItem {
            title,
            action: Some(action),
        });
    }
    let jobs = cx.jobs.sender();
    let result = resolved.clone();
    let mut menu = ui::Menu::new(
        items,
        (),
        move |editor, item: Option<&HintMenuItem>, event| {
            if event != PromptEvent::Validate {
                return;
            }
            let Some(item) = item else { return };
            if let Some(action) = item.action {
                if let Err(error) = result.execute(editor, action, Action::Replace) {
                    editor.set_error(|| format!("Failed to use inlay hint: {error}"));
                }
            } else {
                let result = result.clone();
                jobs.enqueue(Callback::ui(move |compositor, cx| {
                    if result.is_current(cx.editor) {
                        show_details(compositor, cx.editor, result);
                    }
                }));
            }
        },
    );
    menu.move_down();
    let contents = HintActions { resolved, menu };
    compositor.replace_or_push(
        HintActions::ID,
        Popup::new(HintActions::ID, contents)
            .with_border(ui::PopupBorder::Hover)
            .with_scrollbar(false)
            .auto_close(true),
    );
}

fn part_label(hint: &ResolvedInlayHint, index: usize) -> &str {
    match &hint.hint().label {
        lsp::InlayHintLabel::LabelParts(parts) => &parts[index].value,
        _ => unreachable!(),
    }
}

fn show_details(compositor: &mut Compositor, editor: &Editor, resolved: Arc<ResolvedInlayHint>) {
    compositor.replace_or_push(
        HintDetails::ID,
        Popup::new(HintDetails::ID, HintDetails::new(resolved, editor))
            .with_border(ui::PopupBorder::Hover)
            .auto_close(true),
    );
}

struct HintMenuItem {
    title: String,
    action: Option<InlayHintAction>,
}

impl ui::menu::Item for HintMenuItem {
    type Data = ();
    fn format(&self, _: &()) -> Row<'_> {
        self.title.as_str().into()
    }
}

struct HintActions {
    resolved: Arc<ResolvedInlayHint>,
    menu: ui::Menu<HintMenuItem>,
}

impl HintActions {
    const ID: &'static str = "inlay-hint-actions";
}

impl Component for HintActions {
    fn update(&mut self, _: Rect, cx: &mut Context) {
        if !self.resolved.is_current(cx.editor) {
            cx.jobs.sender().enqueue(Callback::ui(|compositor, cx| {
                if compositor
                    .find_id::<Popup<HintActions>>(HintActions::ID)
                    .is_some_and(|popup| !popup.contents().resolved.is_current(cx.editor))
                {
                    compositor.remove(HintActions::ID);
                }
            }));
        }
    }

    fn has_pending_keys(&self) -> bool {
        self.menu.has_pending_keys()
    }

    fn handle_event(&mut self, event: &Event, cx: &mut Context) -> EventResult {
        self.menu.handle_event(event, cx)
    }

    fn render(&mut self, area: Rect, surface: &mut Buffer, cx: &mut DrawContext) {
        self.menu.render(area, surface, cx);
    }

    fn required_size(&mut self, viewport: (u16, u16)) -> Option<(u16, u16)> {
        self.menu.required_size(viewport)
    }
}

enum Content {
    Plain(String),
    Markdown(ui::Markdown),
}

struct HintDetails {
    resolved: Arc<ResolvedInlayHint>,
    sections: Vec<Content>,
}

impl HintDetails {
    const ID: &'static str = "inlay-hint-details";

    fn new(resolved: Arc<ResolvedInlayHint>, editor: &Editor) -> Self {
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
