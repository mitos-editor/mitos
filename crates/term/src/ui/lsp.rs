pub mod hover;
pub mod signature_help;

pub(crate) fn symbol_kind_scope(kind: lsp_client::lsp::SymbolKind) -> &'static str {
    use lsp_client::lsp::SymbolKind;

    match kind {
        SymbolKind::FILE => "ui.text.directory",
        SymbolKind::MODULE | SymbolKind::NAMESPACE | SymbolKind::PACKAGE => "namespace",
        SymbolKind::OBJECT | SymbolKind::STRUCT | SymbolKind::INTERFACE | SymbolKind::CLASS => {
            "type"
        }
        SymbolKind::METHOD => "function.method",
        SymbolKind::FUNCTION => "function",
        SymbolKind::ENUM => "type.enum",
        SymbolKind::ENUM_MEMBER => "type.enum.variant",
        SymbolKind::FIELD | SymbolKind::PROPERTY => "variable.other.member",
        SymbolKind::VARIABLE => "variable",
        SymbolKind::CONSTANT => "constant",
        SymbolKind::CONSTRUCTOR => "constructor",
        SymbolKind::STRING => "string",
        SymbolKind::NUMBER => "constant.numeric",
        SymbolKind::BOOLEAN => "constant.builtin.boolean",
        SymbolKind::ARRAY => "punctuation.bracket",
        SymbolKind::KEY => "label",
        SymbolKind::NULL => "constant.builtin",
        SymbolKind::EVENT => "function",
        SymbolKind::OPERATOR => "operator",
        SymbolKind::TYPE_PARAMETER => "type.parameter",
        _ => "ui.text",
    }
}

#[derive(Default)]
struct PageSelection {
    keys: ui_core::keymap::KeymapState,
    index: usize,
}

impl PageSelection {
    fn handle_event(
        &mut self,
        event: &crate::compositor::Event,
        keymap: &ui_core::keymap::Keymap<crate::keymap::component::PageAction>,
        len: usize,
    ) -> crate::compositor::EventResult {
        use crate::{
            compositor::{Event, EventResult},
            keymap::component::PageAction,
        };
        use ui_core::keymap::Lookup;
        let Event::Key(key) = event else {
            if matches!(event, Event::Paste(_)) {
                self.keys.clear();
            }
            return EventResult::Ignored(None);
        };
        let actions = match self.keys.lookup(keymap, *key) {
            Lookup::Actions(actions) => actions,
            Lookup::Pending => return EventResult::Consumed(None),
            Lookup::Unbound => return EventResult::Ignored(None),
        };
        for action in actions {
            if len <= 1 {
                continue;
            }
            self.index = match action {
                PageAction::Previous => self.index.checked_sub(1).unwrap_or(len - 1),
                PageAction::Next => (self.index + 1) % len,
            };
        }
        EventResult::Consumed(None)
    }
}
