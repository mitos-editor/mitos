use std::{borrow::Cow, collections::HashSet, fmt, future::Future};

use editor_core::syntax::config::LanguageServerFeature;
use lsp_client::{
    lsp::{self, CodeActionKind, CodeActionOrCommand, CodeActionTriggerKind},
    util::{diagnostic_to_lsp_diagnostic, range_to_lsp_range},
    LanguageServerId,
};

use crate::{Document, Editor};

/// A titled action against the editor, shown in the code action menu.
///
/// LSP code actions are converted into `Action`s, but the type is provider-agnostic: internal
/// features (for example the spell checker) produce `Action`s the same way, so they appear in the
/// same menu without the menu knowing where they came from.
pub struct Action {
    title: Cow<'static, str>,
    /// Sort key; higher priority actions are shown first. See `lsp_code_action_priority`.
    pub priority: u8,
    action: Box<dyn Fn(&mut Editor) + Send + Sync + 'static>,
}

impl fmt::Debug for Action {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Action")
            .field("title", &self.title)
            .field("priority", &self.priority)
            .finish_non_exhaustive()
    }
}

impl Action {
    pub fn new<T: Into<Cow<'static, str>>, F: Fn(&mut Editor) + Send + Sync + 'static>(
        title: T,
        priority: u8,
        action: F,
    ) -> Self {
        Self {
            title: title.into(),
            priority,
            action: Box::new(action),
        }
    }

    pub fn title(&self) -> &str {
        &self.title
    }

    pub fn execute(&self, editor: &mut Editor) {
        (self.action)(editor);
    }

    /// Builds an `Action` from an LSP code action or command.
    pub fn lsp(server_id: LanguageServerId, action: lsp::CodeActionOrCommand) -> Self {
        let title = match &action {
            lsp::CodeActionOrCommand::CodeAction(action) => action.title.clone(),
            lsp::CodeActionOrCommand::Command(command) => command.title.clone(),
        };
        let priority = lsp_code_action_priority(&action);

        Self::new(title, priority, move |editor| {
            let Some(language_server) = editor.language_server_by_id(server_id) else {
                editor.set_error(|| "Language Server disappeared");
                return;
            };
            let offset_encoding = language_server.offset_encoding();
            match &action {
                lsp::CodeActionOrCommand::Command(command) => {
                    log::debug!("code action command: {:?}", command);
                    editor.execute_lsp_command(command.clone(), server_id);
                }
                lsp::CodeActionOrCommand::CodeAction(code_action) => {
                    log::debug!("code action: {:?}", code_action);
                    // We support lsp "codeAction/resolve" for `edit` and `command` fields.
                    let resolved;
                    let code_action = if code_action.edit.is_none() || code_action.command.is_none()
                    {
                        resolved = language_server
                            .resolve_code_action(code_action)
                            .and_then(|future| lsp_client::block_on(future).ok());
                        resolved.as_ref().unwrap_or(code_action)
                    } else {
                        code_action
                    };

                    if let Some(ref workspace_edit) = code_action.edit {
                        let _ = editor.apply_workspace_edit(offset_encoding, workspace_edit);
                    }

                    // If a code action provides both an edit and a command the edit is applied
                    // first, then the command.
                    if let Some(command) = &code_action.command {
                        editor.execute_lsp_command(command.clone(), server_id);
                    }
                }
            }
        })
    }
}

/// Computes the sort priority for an LSP code action; higher is shown first.
///
/// This roughly matches VSCode's ordering: code actions are sorted first by the category declared
/// on the `kind` field, then by whether they fix a diagnostic, then by whether they are marked
/// preferred. See <https://github.com/microsoft/vscode/blob/eaec601dd69aeb4abb63b9601a6f44308c8d8c6e/src/vs/editor/contrib/codeAction/browser/codeActionWidget.ts>.
fn lsp_code_action_priority(action: &lsp::CodeActionOrCommand) -> u8 {
    // The `kind` field is open ended in the LSP spec, but in practice a closed set of common values
    // (mostly suggested by the spec) is used. VSCode shows these as menu headers; we don't, but we
    // sort by them the same way.
    let category = if let lsp::CodeActionOrCommand::CodeAction(lsp::CodeAction {
        kind: Some(kind),
        ..
    }) = action
    {
        let mut components = kind.as_str().split('.');
        match components.next() {
            Some("quickfix") => 7,
            Some("refactor") => match components.next() {
                Some("extract") => 6,
                Some("inline") => 5,
                Some("rewrite") => 4,
                Some("move") => 3,
                Some("surround") => 2,
                _ => 1,
            },
            Some("source") => 1,
            _ => 0,
        }
    } else {
        0
    };
    let fixes_diagnostic = matches!(
        action,
        lsp::CodeActionOrCommand::CodeAction(lsp::CodeAction {
            diagnostics: Some(diagnostics),
            ..
        }) if !diagnostics.is_empty()
    );
    let is_preferred = matches!(
        action,
        lsp::CodeActionOrCommand::CodeAction(lsp::CodeAction {
            is_preferred: Some(true),
            ..
        })
    );

    // Weight the criteria so their scores can't overlap: two actions in the same category sort
    // closer than actions in different categories, with `fixes_diagnostic` and `is_preferred`
    // breaking ties.
    let mut priority = category * 4;
    if fixes_diagnostic {
        priority += 2;
    }
    if is_preferred {
        priority += 1;
    }
    priority
}

/// Request code actions for a character range from each distinct supporting server.
/// Used by automatic hints, interactive actions, and actions on save.
// Extracting this to a type alias would require boxing this future
#[allow(clippy::type_complexity)]
pub fn code_actions_for_range(
    doc: &Document,
    range: editor_core::Range,
    only: Option<Vec<CodeActionKind>>,
    trigger_kind: CodeActionTriggerKind,
) -> Vec<(
    impl Future<Output = Result<Option<Vec<CodeActionOrCommand>>, lsp_client::Error>> + use<>,
    LanguageServerId,
)> {
    code_actions_for_range_filtered(doc, range, only, trigger_kind, None)
}

/// Select live providers before constructing requests: the client sends them immediately.
#[allow(clippy::type_complexity)]
pub(crate) fn code_actions_for_range_filtered(
    doc: &Document,
    range: editor_core::Range,
    only: Option<Vec<CodeActionKind>>,
    trigger_kind: CodeActionTriggerKind,
    servers: Option<&HashSet<LanguageServerId>>,
) -> Vec<(
    impl Future<Output = Result<Option<Vec<CodeActionOrCommand>>, lsp_client::Error>> + use<>,
    LanguageServerId,
)> {
    let mut seen_language_servers = HashSet::new();

    doc.language_servers_with_feature(LanguageServerFeature::CodeAction)
        .filter(|ls| servers.is_none_or(|servers| servers.contains(&ls.id())))
        .filter(|ls| seen_language_servers.insert(ls.id()))
        // TODO this should probably already been filtered in something like "language_servers_with_feature"
        .filter_map(|language_server| {
            let offset_encoding = language_server.offset_encoding();
            let language_server_id = language_server.id();
            let lsp_range = range_to_lsp_range(doc.text(), range, offset_encoding);
            // Filter and convert overlapping diagnostics
            let code_action_context = lsp::CodeActionContext {
                diagnostics: doc
                    .diagnostics()
                    .iter()
                    .filter(|&diag| {
                        range.overlaps(&editor_core::Range::new(diag.range.start, diag.range.end))
                    })
                    .map(|diag| diagnostic_to_lsp_diagnostic(doc.text(), diag, offset_encoding))
                    .collect(),
                only: only.clone(),
                trigger_kind: Some(trigger_kind),
            };
            let code_action_request =
                language_server.code_actions(doc.identifier(), lsp_range, code_action_context)?;
            Some((code_action_request, language_server_id))
        })
        .collect::<Vec<_>>()
}
