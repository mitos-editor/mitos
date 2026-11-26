//! Full diagnostic snapshots with editor-owned navigation targets.

use crate::features::document_links::{NavigationTarget, ReferenceLink};
use crate::{editor::Action, Document, DocumentId, Editor};
use diagnostics::DiagnosticDetails;

/// Open a named, immutable diagnostic snapshot. Links work without a running server.
/// Related ranges remain the server's last reported positions; they do not follow edits.
pub fn open_buffer(
    editor: &mut Editor,
    name: String,
    mut text: String,
    details: Option<&DiagnosticDetails>,
    origin: Option<ReferenceLink>,
    action: Action,
) -> DocumentId {
    let mut links: Vec<_> = origin.into_iter().collect();
    if let Some(details) = details {
        if !details.related_information.is_empty() {
            text.push_str("\nRelated locations (last reported positions; gf to open)\n");
            for related in &details.related_information {
                text.push_str(&related.message);
                text.push('\n');
                let label = match related.uri.to_file_path() {
                    Ok(path) => format!(
                        "{}:{}:{}",
                        stdx::path::get_relative_path(&path).display(),
                        related.range.start.row + 1,
                        related.range.start.col + 1
                    ),
                    Err(_) => related.uri.to_string(),
                };
                let target = NavigationTarget::Location {
                    uri: related.uri.clone(),
                    range: super::related_range(related),
                    offset_encoding: related.offset_encoding,
                };
                append_link(&mut text, &mut links, &label, target);
            }
        }
        if let Some(url) = &details.documentation {
            text.push_str("\nDocumentation (gf to open)\n");
            append_link(
                &mut text,
                &mut links,
                url.as_str(),
                NavigationTarget::Url(url.clone()),
            );
        }
    }
    let (view, source) = crate::current_ref!(editor);
    let origin_view = view.id;
    let source_jump = (source.id(), source.selection(view.id).clone());
    let doc = Document::from(
        text.into(),
        None,
        editor.config.clone(),
        editor.syn_loader.load_full(),
    )
    .with_readonly_name(format!("[diagnostic: {name}]"))
    .with_reference_links(links);
    let id = editor.new_file_from_document(action, doc);
    let focused = editor.tree.focus();
    if focused != origin_view {
        let source = editor.documents.get(&source_jump.0).unwrap();
        editor.tree.get_mut(focused).push_jump(source, source_jump);
    }
    id
}

fn append_link(
    text: &mut String,
    links: &mut Vec<ReferenceLink>,
    label: &str,
    target: NavigationTarget,
) {
    let start = text.chars().count();
    text.push_str(label);
    let end = start + label.chars().count();
    text.push('\n');
    links.push(ReferenceLink { start, end, target });
}
