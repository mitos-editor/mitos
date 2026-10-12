use std::sync::Arc;
use ui_core::keymap::{KeymapState, Lookup};

use arc_swap::ArcSwap;
use editor_core::syntax::{self, OverlayHighlights};
use tui::buffer::Buffer;
use tui::layout::{Alignment, Constraint, Layout};
use tui::text::Text;
use tui::widgets::{Paragraph, Widget, Wrap};
use ui_core::input::Event;
use view::graphics::Rect;

use crate::compositor::{Component, Compositor, Context, EventResult};

use crate::keymap::component::PageAction;
use crate::ui::{panel, Markdown};

use crate::ui::Popup;

pub struct Signature {
    pub signature: String,
    pub signature_doc: Option<String>,
    /// Part of signature text
    pub active_param_range: Option<(usize, usize)>,
}

pub struct SignatureHelp {
    keys: KeymapState,
    language: String,
    config_loader: Arc<ArcSwap<syntax::Loader>>,
    active_signature: usize,
    lsp_signature: Option<usize>,
    signatures: Vec<Signature>,
}

impl SignatureHelp {
    pub const ID: &'static str = "signature-help";

    pub fn new(
        language: String,
        config_loader: Arc<ArcSwap<syntax::Loader>>,
        active_signature: usize,
        lsp_signature: Option<usize>,
        signatures: Vec<Signature>,
    ) -> Self {
        Self {
            language,
            keys: KeymapState::default(),
            config_loader,
            active_signature,
            lsp_signature,
            signatures,
        }
    }

    pub fn active_signature(&self) -> usize {
        self.active_signature
    }

    pub fn lsp_signature(&self) -> Option<usize> {
        self.lsp_signature
    }

    pub fn visible_popup(compositor: &mut Compositor) -> Option<&mut Popup<Self>> {
        compositor.find_id::<Popup<Self>>(Self::ID)
    }

    fn signature_index(&self) -> String {
        format!("({}/{})", self.active_signature + 1, self.signatures.len())
    }
}

impl Component for SignatureHelp {
    fn has_pending_keys(&self) -> bool {
        self.keys.is_pending()
    }

    fn handle_event(&mut self, event: &Event, cx: &mut Context) -> EventResult {
        let Event::Key(event) = event else {
            if matches!(event, Event::Paste(_)) {
                self.keys.clear();
            }
            return EventResult::Ignored(None);
        };
        let actions = match self
            .keys
            .lookup(&cx.config.current.component_keys.signature_help, *event)
        {
            Lookup::Actions(actions) => actions,
            Lookup::Pending => return EventResult::Consumed(None),
            Lookup::Unbound => return EventResult::Ignored(None),
        };
        for &action in actions {
            if self.signatures.len() <= 1 {
                continue;
            }
            match action {
                PageAction::Previous => {
                    self.active_signature = self
                        .active_signature
                        .checked_sub(1)
                        .unwrap_or(self.signatures.len() - 1);
                }
                PageAction::Next => {
                    self.active_signature = (self.active_signature + 1) % self.signatures.len();
                }
            }
        }
        EventResult::Consumed(None)
    }

    fn render(&mut self, area: Rect, surface: &mut Buffer, cx: &mut Context) {
        let area = panel::content_area(area);

        let signature = self
            .signatures
            .get(self.active_signature)
            .unwrap_or_else(|| &self.signatures[0]);

        let active_param_span = signature.active_param_range.map(|(start, end)| {
            let highlight = cx
                .editor
                .theme
                .find_highlight_exact("ui.selection")
                .unwrap();
            OverlayHighlights::single(highlight, start..end)
        });

        let sig_text = crate::ui::markdown::highlighted_code_block(
            signature.signature.as_str(),
            &self.language,
            Some(&cx.editor.theme),
            &self.config_loader.load(),
            active_param_span,
        );

        if self.signatures.len() > 1 {
            let signature_index = self.signature_index();
            let text = Text::from(signature_index);
            let paragraph = Paragraph::new(text).alignment(Alignment::Right);
            paragraph.render(area.with_height(1).clip_right(1), surface);
        }

        let sig_text_para =
            crate::ui::text::paragraph(sig_text).scroll((cx.scroll.unwrap_or_default() as u16, 0));
        let (_, sig_text_height) = crate::ui::text::required_size(&sig_text_para, area.width);
        let sig_text_area = area.with_height(sig_text_height.min(area.height));
        let sig_text_area = sig_text_area.intersection(surface.area);
        sig_text_para.render(sig_text_area, surface);

        if signature.signature_doc.is_none() {
            return;
        }

        let [_sig_text_area, separator_area, _spacing, sig_doc_area, _bottom_border] =
            Layout::vertical([
                Constraint::Length(sig_text_area.height),
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Min(0),
                Constraint::Length(1),
            ])
            .areas(area);
        panel::top_border(cx.editor.theme.get("ui.window")).render(separator_area, surface);

        let sig_doc = match &signature.signature_doc {
            None => return,
            Some(doc) => Markdown::new(doc.clone(), Arc::clone(&self.config_loader)),
        };
        let sig_doc = sig_doc.parse(Some(&cx.editor.theme));
        let sig_doc_para = Paragraph::new(sig_doc)
            .wrap(Wrap { trim: false })
            .scroll((cx.scroll.unwrap_or_default() as u16, 0));
        sig_doc_para.render(sig_doc_area, surface);
    }

    fn required_size(&mut self, viewport: (u16, u16)) -> Option<(u16, u16)> {
        const PADDING: u16 = 2;
        const SEPARATOR_HEIGHT: u16 = 1;

        let signature = self
            .signatures
            .get(self.active_signature)
            .unwrap_or_else(|| &self.signatures[0]);

        let max_text_width = viewport.0.saturating_sub(PADDING).clamp(10, 120);

        let signature_text = crate::ui::markdown::highlighted_code_block(
            signature.signature.as_str(),
            &self.language,
            None,
            &self.config_loader.load(),
            None,
        );
        let signature_text = crate::ui::text::paragraph(signature_text);
        let (sig_width, sig_height) =
            crate::ui::text::required_size(&signature_text, max_text_width);

        let (width, height) = match signature.signature_doc {
            Some(ref doc) => {
                let doc_md = Markdown::new(doc.clone(), Arc::clone(&self.config_loader));
                let doc_text = crate::ui::text::paragraph(doc_md.parse(None));
                let (doc_width, doc_height) =
                    crate::ui::text::required_size(&doc_text, max_text_width);
                (
                    sig_width.max(doc_width),
                    sig_height + SEPARATOR_HEIGHT + doc_height,
                )
            }
            None => (sig_width, sig_height),
        };

        let sig_index_width = if self.signatures.len() > 1 {
            self.signature_index().len() + 1
        } else {
            0
        };

        Some((width + PADDING + sig_index_width as u16, height + PADDING))
    }
}
use view::graphics::RectExt as _;
