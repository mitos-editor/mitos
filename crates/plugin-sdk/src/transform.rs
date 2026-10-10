//! Bounded transforms of selected Unicode scalar ranges.
//!
//! Every conversion finishes before effects are returned. A failure produces
//! no edits. A successful change returns one edit group and a revisioned view
//! selection, preserving range direction and the primary selection. The host
//! validates these revisions and applies the group as one undo step.

use crate::{Action, EditorContext, Response, SelectionRange, TextEdit};

pub const MAX_TRANSFORM_BYTES: usize = 1024 * 1024;
pub const MAX_TRANSFORM_SELECTIONS: usize = 128;
const READ_CHARS: usize = 16 * 1024;

/// Transform each selection through the public, revisioned region service.
/// Input and output each have a combined 1 MiB budget, with at most 128 ranges.
pub fn selections(
    context: &EditorContext,
    convert: impl FnMut(&str) -> Result<String, String>,
) -> Response {
    let target = context.document.as_ref().map(|doc| (doc.id, doc.version));
    selections_with_read(
        context,
        |start, end| {
            let (document, version) = target.ok_or("A document is required")?;
            crate::component::read_document(document, version, start as u64, end as u64)
                .map_err(|error| error.message)
        },
        convert,
    )
}

/// The same atomic transform with an explicit reader, useful for native tests
/// or an already-owned source. `read` must return exactly the requested scalar
/// range. Reads use at most 16K scalars (64 KiB UTF-8) before checking the total
/// byte budget, rather than allocating a large region before its size check.
pub fn selections_with_read(
    context: &EditorContext,
    mut read: impl FnMut(usize, usize) -> Result<String, String>,
    mut convert: impl FnMut(&str) -> Result<String, String>,
) -> Response {
    match prepare(context, &mut read, &mut convert) {
        Ok(response) => response,
        Err(error) => Response {
            error: Some(error),
            ..Response::default()
        },
    }
}

fn prepare(
    context: &EditorContext,
    read: &mut impl FnMut(usize, usize) -> Result<String, String>,
    convert: &mut impl FnMut(&str) -> Result<String, String>,
) -> Result<Response, String> {
    let doc = context.document.as_ref().ok_or("A document is required")?;
    let view = context.view.as_ref().ok_or("A document view is required")?;
    if view.document != doc.id
        || view.selections.is_empty()
        || view.selections.len() > MAX_TRANSFORM_SELECTIONS
        || view.primary >= view.selections.len()
    {
        return Err("Invalid document view or selection count (maximum 128)".into());
    }
    let mut ordered: Vec<_> = view.selections.iter().enumerate().collect();
    ordered.sort_unstable_by_key(|(_, range)| {
        (range.anchor.min(range.head), range.anchor.max(range.head))
    });
    let mut previous = None;
    let mut selected_chars = 0usize;
    for (_, range) in &ordered {
        let start = range.anchor.min(range.head);
        let end = range.anchor.max(range.head);
        if end as u64 > doc.char_count
            || previous.is_some_and(|(old_start, old_end)| {
                start < old_end || (start, end) == (old_start, old_end)
            })
        {
            return Err("Selections are invalid or overlap".into());
        }
        selected_chars = selected_chars.saturating_add(end - start);
        if selected_chars > MAX_TRANSFORM_BYTES {
            return Err("Selected input exceeds 1 MiB".into());
        }
        previous = Some((start, end));
    }
    let mut ranges = view.selections.clone();
    let mut edits = Vec::new();
    let mut shift = 0i128;
    let mut input_bytes = 0usize;
    let mut output_bytes = 0usize;
    for (index, range) in ordered {
        let start = range.anchor.min(range.head);
        let end = range.anchor.max(range.head);
        let mut selected = String::new();
        let mut offset = start;
        while offset < end {
            let stop = offset.saturating_add(READ_CHARS).min(end);
            let chunk = read(offset, stop)?;
            if chunk.chars().count() != stop - offset {
                return Err("Document reader returned an inconsistent scalar range".into());
            }
            input_bytes = input_bytes.saturating_add(chunk.len());
            if input_bytes > MAX_TRANSFORM_BYTES {
                return Err("Selected input exceeds 1 MiB".into());
            }
            selected.push_str(&chunk);
            offset = stop;
        }
        let text = convert(&selected)?;
        output_bytes = output_bytes.saturating_add(text.len());
        if output_bytes > MAX_TRANSFORM_BYTES {
            return Err("Transformed output exceeds 1 MiB".into());
        }
        let new_start = usize::try_from(start as i128 + shift)
            .map_err(|_| "Transformed selection exceeds the offset range")?;
        let new_len = text.chars().count();
        let new_end = new_start
            .checked_add(new_len)
            .ok_or("Transformed selection is too large")?;
        ranges[index] = if range.anchor <= range.head {
            SelectionRange {
                anchor: new_start,
                head: new_end,
            }
        } else {
            SelectionRange {
                anchor: new_end,
                head: new_start,
            }
        };
        shift += new_len as i128 - (end - start) as i128;
        if text != selected {
            edits.push(TextEdit { start, end, text });
        }
    }
    if edits.is_empty() {
        return Ok(Response::default());
    }
    Ok(Response {
        actions: vec![
            Action::Edit {
                document: doc.id,
                version: doc.version,
                edits,
            },
            Action::SetSelection {
                document: doc.id,
                version: doc.version,
                view: view.id,
                binding_revision: view.binding_revision,
                selection_revision: view.selection_revision,
                ranges,
                primary: view.primary,
            },
        ],
        error: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DocumentSnapshot, ViewSnapshot};

    fn context(text: &str, selections: Vec<SelectionRange>) -> EditorContext {
        EditorContext {
            document: Some(DocumentSnapshot {
                id: 7,
                version: 3,
                path: None,
                language: None,
                char_count: text.chars().count() as u64,
                byte_count: text.len() as u64,
            }),
            view: Some(ViewSnapshot {
                id: 9,
                document: 7,
                binding_revision: 11,
                selection_revision: 12,
                primary: 1,
                selections,
            }),
            ..EditorContext::default()
        }
    }

    #[test]
    fn preserves_unsorted_backward_unicode_ranges_and_snapshot_revisions() {
        let text = "é straße";
        let context = context(
            text,
            vec![
                SelectionRange { anchor: 8, head: 2 },
                SelectionRange { anchor: 0, head: 1 },
            ],
        );
        let response = selections_with_read(
            &context,
            |start, end| Ok(text.chars().skip(start).take(end - start).collect()),
            |text| Ok(text.to_uppercase()),
        );
        assert!(response.error.is_none());
        assert!(
            matches!(&response.actions[0], Action::Edit { version: 3, edits, .. } if edits.len() == 2)
        );
        assert!(matches!(&response.actions[1], Action::SetSelection {
            version: 3, binding_revision: 11, selection_revision: 12, primary: 1, ranges, ..
        } if ranges == &[SelectionRange { anchor: 9, head: 2 }, SelectionRange { anchor: 0, head: 1 }]));
    }

    #[test]
    fn later_errors_and_inconsistent_reads_emit_no_partial_edits() {
        let context = context(
            "ab",
            vec![
                SelectionRange { anchor: 0, head: 1 },
                SelectionRange { anchor: 1, head: 2 },
            ],
        );
        let response = selections_with_read(
            &context,
            |start, _| {
                if start == 0 {
                    Ok("a".into())
                } else {
                    Err("stale version".into())
                }
            },
            |text| Ok(text.to_uppercase()),
        );
        assert_eq!(response.error.as_deref(), Some("stale version"));
        assert!(response.actions.is_empty());
        let response = selections_with_read(
            &context,
            |_, _| Ok("too long".into()),
            |text| Ok(text.into()),
        );
        assert!(response.actions.is_empty());
        assert!(response.error.unwrap().contains("inconsistent"));
    }

    #[test]
    fn rejects_oversized_output_and_overlapping_ranges_atomically() {
        let mut context = context(
            "ab",
            vec![
                SelectionRange { anchor: 0, head: 1 },
                SelectionRange { anchor: 1, head: 2 },
            ],
        );
        let response = selections_with_read(
            &context,
            |_, _| Ok("a".into()),
            |_| Ok("x".repeat(MAX_TRANSFORM_BYTES)),
        );
        assert!(response.actions.is_empty());
        assert!(response.error.unwrap().contains("output"));
        context.view.as_mut().unwrap().selections[1] = SelectionRange { anchor: 0, head: 2 };
        let response = selections_with_read(
            &context,
            |_, _| panic!("invalid selection must fail before reading"),
            |_| Ok(String::new()),
        );
        assert!(response.actions.is_empty());
        assert!(response.error.unwrap().contains("overlap"));
    }
}
