//! Explicit formatting operations. Frontends drive tasks and apply their validated updates.

mod provider;
pub use provider::FormatterError;

use std::sync::Arc;

use anyhow::{bail, Context};
use editor_core::{
    indent::IndentStyle, syntax, syntax::config::LanguageServerFeature, Transaction, Uri,
};
use futures_util::{future::BoxFuture, FutureExt};
use lsp_client::{lsp, LanguageServerId};

use crate::{callbacks::EditorCallbackSender, Document, DocumentId, Editor, ViewId};

/// The document and formatter configuration used to compute a formatting result.
pub(crate) struct FormatSnapshot {
    document: DocumentId,
    version: i32,
    uri: Option<Uri>,
    language: Option<String>,
    loader: Arc<syntax::Loader>,
    owner: EditorCallbackSender,
}

impl FormatSnapshot {
    pub(crate) fn new(editor: &Editor, doc: &Document) -> Self {
        Self {
            document: doc.id(),
            version: doc.version(),
            uri: doc.uri(),
            language: doc
                .language_config()
                .map(|config| config.language_id.clone()),
            loader: editor.syn_loader.load_full(),
            owner: editor.callback_sender().clone(),
        }
    }

    pub(crate) fn is_current(&self, editor: &Editor, doc: &Document) -> bool {
        self.document == doc.id()
            && self.owner.same_destination(editor.callback_sender())
            && doc.version() == self.version
            && doc.uri() == self.uri
            && doc
                .language_config()
                .map(|config| config.language_id.as_str())
                == self.language.as_deref()
            && Arc::ptr_eq(&self.loader, &editor.syn_loader.load_full())
    }
}

#[derive(Clone, Copy)]
enum FormatKind {
    Document,
    Selections,
}

/// One explicit formatting operation. Dropping the task abandons its pending result.
#[must_use = "formatting tasks must be run to produce an update"]
pub struct FormattingTask {
    target: FormatTarget,
    future: BoxFuture<'static, anyhow::Result<Option<Transaction>>>,
}

struct FormatTarget {
    snapshot: FormatSnapshot,
    view: ViewId,
    binding_revision: u64,
    server: Option<LanguageServerId>,
    kind: FormatKind,
}

/// A completed formatting operation, including errors bound to its original target.
#[must_use = "formatting updates must be applied to the requesting editor"]
pub struct FormattingUpdate {
    target: FormatTarget,
    result: anyhow::Result<Option<Transaction>>,
}

impl FormattingTask {
    pub async fn run(self) -> FormattingUpdate {
        FormattingUpdate {
            target: self.target,
            result: self.future.await,
        }
    }
}

impl FormattingUpdate {
    /// Apply a current result, returning whether it was applied. Stale results and
    /// their errors are discarded; current failures are returned for presentation.
    pub fn apply(self, editor: &mut Editor) -> anyhow::Result<bool> {
        let FormatTarget {
            snapshot,
            view,
            binding_revision,
            server,
            kind,
        } = self.target;
        let Some(doc) = editor.document(snapshot.document) else {
            return Ok(false);
        };
        if !snapshot.is_current(editor, doc)
            || editor.tree.try_get(view).is_none_or(|view| {
                view.doc != snapshot.document || view.binding_revision() != binding_revision
            })
            || server.is_some_and(|server| {
                editor.language_server_by_id(server).is_none()
                    || !doc.supports_language_server(server)
            })
        {
            return Ok(false);
        }
        let Some(transaction) = self.result? else {
            return Ok(false);
        };
        let scrolloff = editor.config().scroll.padding;
        let doc = doc_mut!(editor, &snapshot.document);
        if !crate::editing::apply(doc, &transaction, view) {
            return Ok(false);
        }
        if matches!(kind, FormatKind::Document) {
            let view = view_mut!(editor, view);
            crate::editing::commit(doc, view);
            doc.detect_indent_and_line_ending();
            view.ensure_cursor_in_view(doc, scrolloff);
        }
        Ok(true)
    }
}

fn prepare_target(
    editor: &Editor,
    view: ViewId,
    kind: FormatKind,
) -> anyhow::Result<(FormatTarget, &Document)> {
    let view = editor
        .tree
        .try_get(view)
        .context("Formatting view is closed")?;
    let doc = editor
        .document(view.doc)
        .context("Formatting document is closed")?;
    let target = FormatTarget {
        snapshot: FormatSnapshot::new(editor, doc),
        view: view.id,
        binding_revision: view.binding_revision(),
        server: None,
        kind,
    };
    Ok((target, doc))
}

/// Prepare whole-document formatting using the document's configured formatter.
pub fn prepare_document(editor: &Editor, view: ViewId) -> anyhow::Result<FormattingTask> {
    let (target, doc) = prepare_target(editor, view, FormatKind::Document)?;
    let format = doc.format(editor).context(
        "A formatter isn't available, and no language server provides formatting capabilities",
    )?;
    Ok(FormattingTask {
        target,
        future: async move { Ok(Some(format.await?)) }.boxed(),
    })
}

/// Prepare LSP range formatting for the view's single selection.
pub fn prepare_selections(editor: &Editor, view: ViewId) -> anyhow::Result<FormattingTask> {
    let (mut target, doc) = prepare_target(editor, view, FormatKind::Selections)?;
    let selection = doc.selection(view);
    if selection.len() != 1 {
        bail!("format_selections only supports a single selection for now");
    }
    // Range-only servers need not advertise whole-document formatting. Honor
    // the configured Format feature, then check this operation's capability.
    let server = doc
        .language_config()
        .into_iter()
        .flat_map(|config| &config.language_servers)
        .filter(|config| config.has_feature(LanguageServerFeature::Format))
        .filter_map(|config| doc.language_servers.get(&config.name))
        .find(|server| {
            server.is_initialized()
                && matches!(
                    server.capabilities().document_range_formatting_provider,
                    Some(lsp::OneOf::Left(true) | lsp::OneOf::Right(_))
                )
        })
        .context("No configured language server supports range formatting")?;
    target.server = Some(server.id());
    let encoding = server.offset_encoding();
    let range = lsp_client::util::range_to_lsp_range(doc.text(), selection.primary(), encoding);
    let request = server
        .text_document_range_formatting(
            doc.identifier(),
            range,
            lsp::FormattingOptions {
                tab_size: doc.tab_width() as u32,
                insert_spaces: matches!(doc.indent_style, IndentStyle::Spaces(_)),
                ..Default::default()
            },
            None,
        )
        .context("Language server rejected range formatting")?;
    let text = doc.text().clone();
    Ok(FormattingTask {
        target,
        future: async move {
            Ok(request.await?.map(|edits| {
                lsp_client::util::generate_transaction_from_edits(&text, edits, encoding)
            }))
        }
        .boxed(),
    })
}
