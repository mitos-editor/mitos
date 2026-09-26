//! Diagnostic navigation and pickers across diagnostic providers.

use crate::{
    commands::{context::Context, lsp::jump_to_position, navigation::push_jump},
    ui::{self, overlay::overlaid, FileLocation, Picker},
};
use editor_core::{
    diagnostic::{DiagnosticProvider, NumberOrString, Severity},
    Selection, Uri,
};
use lsp_client::{lsp, OffsetEncoding};
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};
use stdx::path;
use tui::text::Span;
use view::{
    align_view,
    editor::Action,
    icons::ICONS,
    quicklist::{QuicklistEntry, QuicklistPosition, QuicklistTarget},
    theme::Style,
    Align, Document, DocumentId, Editor,
};

pub(super) fn goto_first_diag(cx: &mut Context) {
    let (view, doc) = current!(cx.editor);
    let selection = match doc.diagnostics().first() {
        Some(diag) => Selection::single(diag.range.start, diag.range.end),
        None => return,
    };
    push_jump(view, doc);
    doc.set_selection(view.id, selection);
    view.diagnostics_handler
        .immediately_show_diagnostic(doc, view.id);
}

pub(super) fn goto_last_diag(cx: &mut Context) {
    let (view, doc) = current!(cx.editor);
    let selection = match doc.diagnostics().last() {
        Some(diag) => Selection::single(diag.range.start, diag.range.end),
        None => return,
    };
    push_jump(view, doc);
    doc.set_selection(view.id, selection);
    view.diagnostics_handler
        .immediately_show_diagnostic(doc, view.id);
}

pub(super) fn goto_next_diag(cx: &mut Context) {
    let motion = move |editor: &mut Editor| {
        let (view, doc) = current!(editor);

        let cursor_pos = doc
            .selection(view.id)
            .primary()
            .cursor(doc.text().slice(..));

        let diag = doc
            .diagnostics()
            .iter()
            .find(|diag| diag.range.start > cursor_pos);

        let selection = match diag {
            Some(diag) => Selection::single(diag.range.start, diag.range.end),
            None => return,
        };
        push_jump(view, doc);
        doc.set_selection(view.id, selection);
        view.diagnostics_handler
            .immediately_show_diagnostic(doc, view.id);
    };

    cx.editor.apply_motion(motion);
}

pub(super) fn goto_prev_diag(cx: &mut Context) {
    let motion = move |editor: &mut Editor| {
        let (view, doc) = current!(editor);

        let cursor_pos = doc
            .selection(view.id)
            .primary()
            .cursor(doc.text().slice(..));

        let diag = doc
            .diagnostics()
            .iter()
            .rev()
            .find(|diag| diag.range.start < cursor_pos);

        let selection = match diag {
            // NOTE: the selection is reversed because we're jumping to the
            // previous diagnostic.
            Some(diag) => Selection::single(diag.range.end, diag.range.start),
            None => return,
        };
        push_jump(view, doc);
        doc.set_selection(view.id, selection);
        view.diagnostics_handler
            .immediately_show_diagnostic(doc, view.id);
    };
    cx.editor.apply_motion(motion)
}

struct DiagnosticStyles {
    icons: bool,
    hint: Style,
    info: Style,
    warning: Style,
    error: Style,
}

/// Where a picker diagnostic lives, so it can be previewed and jumped to regardless of source.
enum DiagnosticLocation {
    /// A diagnostic on an open document, in the document's char offsets. Every provider's
    /// diagnostics take this form once they are on the document (LSP included), and it covers
    /// scratch buffers, which have no path.
    Document {
        doc_id: DocumentId,
        /// The document's path, for the picker's path column. `None` for scratch buffers.
        path: Option<PathBuf>,
        range: editor_core::diagnostic::Range,
    },
    /// An LSP diagnostic for a file that is not currently open, positioned in the server's encoding.
    File {
        uri: Uri,
        range: lsp::Range,
        offset_encoding: OffsetEncoding,
    },
}

struct PickerDiagnostic {
    location: DiagnosticLocation,
    severity: Option<Severity>,
    code: Option<NumberOrString>,
    source: Option<Box<str>>,
    message: Box<str>,
}

#[derive(Copy, Clone, PartialEq)]
enum DiagnosticsFormat {
    ShowSourcePath,
    HideSourcePath,
}

type DiagnosticsPicker = Picker<PickerDiagnostic, DiagnosticStyles>;

/// Builds picker items from a single open document's diagnostics. These are in the document's own
/// char offsets, edit-mapped, and include every provider (LSP and internal alike).
fn open_document_diagnostics(doc: &Document) -> impl Iterator<Item = PickerDiagnostic> + '_ {
    let doc_id = doc.id();
    let path = doc.path().map(Path::to_path_buf);
    doc.diagnostics().iter().map(move |diag| PickerDiagnostic {
        location: DiagnosticLocation::Document {
            doc_id,
            path: path.clone(),
            range: diag.range,
        },
        severity: diag.severity,
        code: diag.code.clone(),
        source: diag.source.clone(),
        message: diag.message.clone(),
    })
}

/// Builds a picker item from an LSP diagnostic held in the editor's store. This is used for files
/// which are not currently open; open files are sourced from the document instead.
fn store_diagnostic(
    editor: &Editor,
    uri: &Uri,
    diagnostic: &lsp::Diagnostic,
    provider: &DiagnosticProvider,
) -> Option<PickerDiagnostic> {
    let offset_encoding = editor
        .language_server_by_id(provider.language_server_id()?)?
        .offset_encoding();
    let severity = diagnostic.severity.and_then(|severity| match severity {
        lsp::DiagnosticSeverity::ERROR => Some(Severity::Error),
        lsp::DiagnosticSeverity::WARNING => Some(Severity::Warning),
        lsp::DiagnosticSeverity::INFORMATION => Some(Severity::Info),
        lsp::DiagnosticSeverity::HINT => Some(Severity::Hint),
        _ => None,
    });
    let code = diagnostic.code.as_ref().map(|code| match code {
        lsp::NumberOrString::Number(n) => NumberOrString::Number(*n),
        lsp::NumberOrString::String(s) => NumberOrString::String(s.clone().into()),
    });
    Some(PickerDiagnostic {
        location: DiagnosticLocation::File {
            uri: uri.clone(),
            range: diagnostic.range,
            offset_encoding,
        },
        severity,
        code,
        source: diagnostic.source.clone().map(Into::into),
        message: diagnostic.message.clone().into(),
    })
}

fn jump_to_diagnostic(editor: &mut Editor, location: &DiagnosticLocation, action: Action) {
    match location {
        DiagnosticLocation::File {
            uri,
            range,
            offset_encoding,
        } => {
            let Some(path) = uri.as_path() else {
                editor.set_error(|| format!("unable to convert URI to filepath: {uri}"));
                return;
            };
            let (view, doc) = current!(editor);
            push_jump(view, doc);
            jump_to_position(editor, path, *range, *offset_encoding, action);
        }
        DiagnosticLocation::Document { doc_id, range, .. } => {
            if !editor.documents.contains_key(doc_id) {
                return;
            }
            let (view, doc) = current!(editor);
            push_jump(view, doc);
            editor.switch(*doc_id, action);
            let (view, doc) = current!(editor);
            let len = doc.text().len_chars();
            // Flip the selection so the cursor sits at the start of the diagnostic.
            let anchor = range.end.min(len);
            let head = range.start.min(len);
            doc.set_selection(view.id, Selection::single(anchor, head));
            if action.align_view(view, doc.id()) {
                align_view(doc, view, Align::Center);
            }
        }
    }
}
fn diagnostic_file_location<'a>(
    editor: &'a Editor,
    item: &'a PickerDiagnostic,
) -> Option<FileLocation<'a>> {
    match &item.location {
        DiagnosticLocation::File { uri, range, .. } => Some((
            uri.as_path()?.into(),
            Some((range.start.line as usize, range.end.line as usize)),
        )),
        DiagnosticLocation::Document { doc_id, range, .. } => {
            let text = editor.documents.get(doc_id)?.text();
            let len = text.len_chars();
            let start = text.char_to_line(range.start.min(len));
            let end = text.char_to_line(range.end.min(len));
            Some(((*doc_id).into(), Some((start, end))))
        }
    }
}

fn diagnostic_quicklist_entry(editor: &Editor, item: &PickerDiagnostic) -> Option<QuicklistEntry> {
    Some(match &item.location {
        DiagnosticLocation::File {
            uri,
            range,
            offset_encoding,
        } => QuicklistEntry {
            target: QuicklistTarget::Path(uri.as_path()?.to_path_buf()),
            position: QuicklistPosition::LspRange {
                range: *range,
                offset_encoding: *offset_encoding,
            },
        },
        DiagnosticLocation::Document { doc_id, range, .. } => {
            let len = editor.document(*doc_id)?.text().len_chars();
            QuicklistEntry {
                target: QuicklistTarget::Document(*doc_id),
                position: QuicklistPosition::Selection(Selection::single(
                    range.end.min(len),
                    range.start.min(len),
                )),
            }
        }
    })
}

fn diag_picker(
    cx: &Context,
    mut diagnostics: Vec<PickerDiagnostic>,
    format: DiagnosticsFormat,
) -> DiagnosticsPicker {
    // Sort by severity, most severe first; diagnostics with no severity sort last.
    diagnostics.sort_by_key(|diagnostic| std::cmp::Reverse(diagnostic.severity));

    let styles = DiagnosticStyles {
        icons: cx.editor.config().icons,
        hint: cx.editor.theme.get("hint"),
        info: cx.editor.theme.get("info"),
        warning: cx.editor.theme.get("warning"),
        error: cx.editor.theme.get("error"),
    };

    let mut columns = vec![
        ui::PickerColumn::new(
            "severity",
            |item: &PickerDiagnostic, styles: &DiagnosticStyles| {
                let icons = ICONS.load();
                match item.severity {
                    Some(Severity::Hint) => Span::styled(
                        if styles.icons {
                            format!("{}HINT", icons.diagnostic().hint())
                        } else {
                            "HINT".to_string()
                        },
                        styles.hint,
                    ),
                    Some(Severity::Info) => Span::styled(
                        if styles.icons {
                            format!("{}INFO", icons.diagnostic().info())
                        } else {
                            "INFO".to_string()
                        },
                        styles.info,
                    ),
                    Some(Severity::Warning) => Span::styled(
                        if styles.icons {
                            format!("{}WARN", icons.diagnostic().warning())
                        } else {
                            "WARN".to_string()
                        },
                        styles.warning,
                    ),
                    Some(Severity::Error) => Span::styled(
                        if styles.icons {
                            format!("{}ERROR", icons.diagnostic().error())
                        } else {
                            "ERROR".to_string()
                        },
                        styles.error,
                    ),
                    _ => Span::raw(""),
                }
                .into()
            },
        ),
        ui::PickerColumn::new("source", |item: &PickerDiagnostic, _| {
            item.source.as_deref().unwrap_or("").into()
        }),
        ui::PickerColumn::new("code", |item: &PickerDiagnostic, _| {
            match item.code.as_ref() {
                Some(NumberOrString::Number(n)) => n.to_string().into(),
                Some(NumberOrString::String(s)) => (&**s).into(),
                None => "".into(),
            }
        }),
        ui::PickerColumn::new("message", |item: &PickerDiagnostic, _| {
            (&*item.message).into()
        }),
    ];
    let mut primary_column = 3; // message

    if format == DiagnosticsFormat::ShowSourcePath {
        columns.insert(
            // between message code and message
            3,
            ui::PickerColumn::new("path", |item: &PickerDiagnostic, _| match &item.location {
                DiagnosticLocation::File { uri, .. } => match uri.as_path() {
                    Some(path) => path::get_truncated_path(path)
                        .to_string_lossy()
                        .to_string()
                        .into(),
                    None => Default::default(),
                },
                DiagnosticLocation::Document {
                    path: Some(path), ..
                } => path::get_truncated_path(path)
                    .to_string_lossy()
                    .to_string()
                    .into(),
                DiagnosticLocation::Document { path: None, .. } => "[scratch]".into(),
            }),
        );
        primary_column += 1;
    }

    Picker::new(
        columns,
        primary_column,
        diagnostics,
        styles,
        move |cx, diag, action| {
            jump_to_diagnostic(cx.editor, &diag.location, action);
            let (view, doc) = current!(cx.editor);
            view.diagnostics_handler
                .immediately_show_diagnostic(doc, view.id);
        },
    )
    .with_preview(diagnostic_file_location)
    .with_quicklist(diagnostic_quicklist_entry)
    .truncate_start(false)
}

pub fn diagnostics_picker(cx: &mut Context) {
    let doc = doc!(cx.editor);
    let diagnostics: Vec<_> = open_document_diagnostics(doc).collect();
    let picker = diag_picker(cx, diagnostics, DiagnosticsFormat::HideSourcePath);
    cx.push_layer(Box::new(overlaid(picker)));
}

pub fn workspace_diagnostics_picker(cx: &mut Context) {
    let mut diagnostics = Vec::new();
    // Open documents carry diagnostics from every provider, edit-mapped, scratch buffers included.
    for doc in cx.editor.documents() {
        diagnostics.extend(open_document_diagnostics(doc));
    }
    // The store additionally holds LSP diagnostics for files which are not currently open.
    let open_paths: HashSet<&Path> = cx.editor.documents().filter_map(|doc| doc.path()).collect();
    for (uri, diags) in &cx.editor.diagnostics {
        if uri.as_path().is_some_and(|path| open_paths.contains(path)) {
            continue;
        }
        diagnostics.extend(
            diags
                .iter()
                .filter_map(|(diag, provider)| store_diagnostic(cx.editor, uri, diag, provider)),
        );
    }
    let picker = diag_picker(cx, diagnostics, DiagnosticsFormat::ShowSourcePath);
    cx.push_layer(Box::new(overlaid(picker)));
}

pub(super) mod typed {
    //! Typable diagnostics commands.

    use crate::{compositor, ui::PromptEvent};
    use ::command_line::Args;
    use anyhow::{bail, ensure};

    #[cold]
    pub(in crate::commands) fn yank_diagnostic(
        cx: &mut compositor::Context,
        args: Args,
        event: PromptEvent,
    ) -> anyhow::Result<()> {
        if event != PromptEvent::Validate {
            return Ok(());
        }

        let reg = match args.first() {
            Some(s) => {
                ensure!(s.chars().count() == 1, format!("Invalid register {s}"));
                s.chars().next().unwrap()
            }
            None => '+',
        };

        let (view, doc) = current_ref!(cx.editor);
        let primary = doc.selection(view.id).primary();

        // Look only for diagnostics that intersect with the primary selection
        let diag: Vec<_> = doc
            .diagnostics()
            .iter()
            .filter(|d| primary.overlaps(&editor_core::Range::new(d.range.start, d.range.end)))
            .map(|d| d.message.clone().into_string())
            .collect();
        let n = diag.len();
        if n == 0 {
            bail!("No diagnostics under primary selection");
        }

        cx.editor.registers.write(reg, diag)?;
        cx.editor.set_status(format!(
            "Yanked {n} diagnostic{} to register {reg}",
            if n == 1 { "" } else { "s" }
        ));
        Ok(())
    }
}
