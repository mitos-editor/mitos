//! Selection between language-server and syntax symbol providers.

use crate::commands::{
    context::Context,
    lsp,
    syntax::{syntax_symbol_picker, syntax_workspace_symbol_picker},
};
use editor_core::syntax::config::LanguageServerFeature;

pub(super) fn lsp_or_syntax_symbol_picker(cx: &mut Context) {
    let doc = doc!(cx.editor);

    if doc
        .language_servers_with_feature(LanguageServerFeature::DocumentSymbols)
        .next()
        .is_some()
    {
        lsp::symbol_picker(cx);
    } else if doc.syntax().is_some() || doc.is_syntax_pending() {
        syntax_symbol_picker(cx);
    } else {
        cx.editor.set_error(|| {
            "No language server supporting document symbols or syntax info available"
        });
    }
}

pub(super) fn lsp_or_syntax_workspace_symbol_picker(cx: &mut Context) {
    let doc = doc!(cx.editor);

    if doc
        .language_servers_with_feature(LanguageServerFeature::WorkspaceSymbols)
        .next()
        .is_some()
    {
        lsp::workspace_symbol_picker(cx);
    } else {
        syntax_workspace_symbol_picker(cx);
    }
}
