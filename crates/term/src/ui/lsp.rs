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
