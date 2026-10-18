//! Validate the complete batch before touching source text. Actual writes retain
//! the editor's ordering, encoding, external-change checks, and completion path.

use anyhow::{ensure, Context};
use buffer::projection::Projection;
use editor_core::{diff::compare_ropes, Assoc, ChangeSet, Rope, Transaction};

use crate::{document::DocumentSavedEvent, DocumentId, Editor};

/// Map offsets through outside edits. Equal-length edits are conflicts too.
/// Force permits changed excerpt content, but never an invalid mapping.
pub(super) fn map_source(
    projection: &mut Projection<DocumentId>,
    id: DocumentId,
    changes: &ChangeSet,
    text: &Rope,
    force: bool,
) -> anyhow::Result<()> {
    for s in &mut projection.segments {
        if s.source != id {
            continue;
        }
        ensure!(
            changes.changes_iter().all(|(start, end, _)| {
                !(start < s.source_range.start && end > s.source_range.start
                    || start < s.source_range.end && end > s.source_range.end)
            }),
            "Source changed across an excerpt boundary; reopen the multi-buffer"
        );
        let range = changes.map_pos(s.source_range.start, Assoc::Before)
            ..changes.map_pos(s.source_range.end, Assoc::After);
        ensure!(
            range.start <= range.end && range.end <= text.len_chars(),
            "Source no longer maps to valid text"
        );
        ensure!(
            force || text.slice(range.clone()) == s.original_text.slice(..),
            "Source excerpt changed since opening the multi-buffer; use :w! to overwrite"
        );
        s.source_range = range;
        s.source_line_start = text.char_to_line(s.source_range.start);
        s.original_text = text.slice(s.source_range.clone()).into();
    }
    Ok(())
}

struct PlannedSource {
    id: DocumentId,
    version: i32,
    changes: ChangeSet,
    rebase: ChangeSet,
    text: Rope,
}

impl Editor {
    pub(crate) fn save_multibuffer(&mut self, id: DocumentId, force: bool) -> anyhow::Result<()> {
        let doc = self.document(id).context("Multi-buffer is closed")?;
        let multi = doc.multibuffer().context("Not a multi-buffer")?;
        ensure!(multi.source_mapping_valid, "Source edits crossed an older excerpt boundary; redo the expansion or reopen the multi-buffer");
        ensure!(
            multi.writes.is_empty() || !self.has_pending_writes(),
            "Wait for pending source writes before saving the multi-buffer again"
        );
        ensure!(
            multi.projection.is_valid(doc.text()),
            "Invalid excerpt mapping"
        );
        let mut plans = Vec::new();
        // Stable ordering also makes validation errors predictable.
        let mut source_ids: Vec<_> = multi.sources.keys().copied().collect();
        source_ids.sort_unstable();
        let mut mapped = multi.projection.clone();
        for source_id in source_ids {
            let source = self
                .document(source_id)
                .context("Multi-buffer source is closed")?;
            let metadata = multi.source(source_id);
            ensure!(
                (metadata.path.is_none() || source.path() == metadata.path.as_deref()),
                "Source filename changed since opening the multi-buffer"
            );
            let external = compare_ropes(&metadata.text, source.text());
            map_source(
                &mut mapped,
                source_id,
                external.changes(),
                source.text(),
                force,
            )
            .map_err(|error| anyhow::anyhow!("{}: {error}", source.display_name()))?;
            let mut edits = Vec::new();
            let mut previous_end = 0;
            for (original, mapped) in multi.segments().iter().zip(&mapped.segments) {
                if original.source != source_id {
                    continue;
                }
                ensure!(
                    mapped.source_range.start >= previous_end,
                    "Overlapping source excerpts"
                );
                previous_end = mapped.source_range.end;
                let replacement = doc.text().slice(original.projection_range.clone());
                if replacement == original.original_text.slice(..)
                    && (!force || replacement == mapped.original_text.slice(..))
                {
                    continue;
                }
                // Preserve positions inside unchanged context, including mappings
                // retained by undo revisions created before an excerpt expanded.
                let before: Rope = source.text().slice(mapped.source_range.clone()).into();
                let after: Rope = replacement.into();
                for (start, end, insert) in compare_ropes(&before, &after).changes_iter() {
                    edits.push((
                        mapped.source_range.start + start,
                        mapped.source_range.start + end,
                        insert,
                    ));
                }
            }
            if edits.is_empty() && !multi.writes.contains_key(&source_id) {
                continue;
            }
            ensure!(
                source.path().is_some(),
                "Cannot write source '{}' without a filename",
                source.display_name()
            );
            ensure!(
                !source.readonly && !source.is_binary(),
                "Source '{}' is read-only",
                source.display_name()
            );
            let transaction = Transaction::change(source.text(), edits.into_iter());
            source.validate_changes(transaction.changes())?;
            let mut text = source.text().clone();
            ensure!(
                transaction.changes().apply(&mut text),
                "Invalid source edit"
            );
            let rebase = external
                .changes()
                .clone()
                .compose(transaction.changes().clone());
            plans.push(PlannedSource {
                id: source_id,
                version: source.version(),
                changes: transaction.changes().clone(),
                rebase,
                text,
            });
        }
        self.document_mut(id).unwrap().commit_changes();
        // All text validation above is complete before the first source changes.
        for plan in &plans {
            self.apply_document_changes(plan.id, plan.version, &plan.changes)?;
        }
        for plan in &plans {
            let source = self.document(plan.id).unwrap();
            let revision = source.get_current_revision();
            let path = source.path().map(ToOwned::to_owned);
            let multi = self.document_mut(id).unwrap().multibuffer.as_mut().unwrap();
            multi.rebase_source(plan.id, &plan.rebase, &plan.text);
            multi.sources.get_mut(&plan.id).unwrap().path = path;
            multi.writes.insert(plan.id, (revision, plan.text.clone()));
        }
        {
            let doc = self.document_mut(id).unwrap();
            let multi = doc.multibuffer.as_mut().unwrap();
            multi.modified = multi.projection.is_modified(doc.buffer.text());
        }
        for plan in plans {
            self.save(plan.id, None::<std::path::PathBuf>, force)?;
        }
        Ok(())
    }

    pub(crate) fn complete_multibuffer_write(&mut self, event: &DocumentSavedEvent) {
        for doc in self.documents_mut() {
            let Some(multi) = doc.multibuffer.as_mut() else {
                continue;
            };
            if multi
                .writes
                .get(&event.doc_id)
                .is_some_and(|(revision, text)| *revision == event.revision && *text == event.text)
            {
                multi.writes.remove(&event.doc_id);
            }
        }
    }
}
