use std::collections::HashSet;

use crate::ui::overlay::Overlay;
use indexmap::IndexSet;
use view::DocumentId;

use super::*;

const OPEN_ALL_CONFIRM_THRESHOLD: usize = 50;

impl<T: Send + Sync + 'static, D: Send + Sync + 'static> Picker<T, D> {
    pub(super) fn supports_open_all(&self) -> bool {
        self.open_all_target.is_some() || self.file_fn.is_some()
    }

    pub(super) fn open_all(&mut self, cx: &mut Context) -> EventResult {
        // The user may press the shortcut before the initial scan or the
        // latest query has made it into the matcher's snapshot.
        if self.dynamic_query_pending()
            || self.results.matcher.active_injectors() > 0
            || self.results.tick(10).running
        {
            cx.editor.set_status("File results are still loading");
            return EventResult::Consumed(None);
        }
        let mut seen = HashSet::new();
        let targets: IndexSet<_> = self
            .results
            .matcher
            .snapshot()
            .matched_items(..)
            .filter_map(|item| match self.open_all_target {
                Some(target) => target(cx.editor, item.data),
                None => self.file_fn.as_ref()?(cx.editor, item.data).map(|(target, _)| target),
            })
            .filter(|target| seen.insert(*target))
            .map(|target| match target {
                PathOrId::Id(id) => QuicklistTarget::Document(id),
                PathOrId::Path(path) => {
                    let path = stdx::path::canonicalize(path);
                    cx.editor
                        .document_id_by_path(&path)
                        .map_or(QuicklistTarget::Path(path), QuicklistTarget::Document)
                }
            })
            .collect();
        if targets.is_empty() {
            cx.editor.set_status("No matching files or buffers");
            return EventResult::Consumed(None);
        }
        let action = self.default_action;
        if targets.len() <= OPEN_ALL_CONFIRM_THRESHOLD {
            return if open_targets(cx.editor, &targets, action) {
                EventResult::Consumed(Some(self.close_callback(None)))
            } else {
                EventResult::Consumed(None)
            };
        }

        let on_close = self.on_close;
        let prompt = Prompt::new_with_callback(
            format!("Open {} buffers? [y/N] ", targets.len()).into(),
            None,
            ui::completers::none,
            move |cx, input, event| {
                if event != PromptEvent::Validate || !input.trim().eq_ignore_ascii_case("y") {
                    return None;
                }
                if !open_targets(cx.editor, &targets, action) {
                    return None;
                }
                Some(Box::new(move |compositor, cx| {
                    if let Some(close) = on_close {
                        close(compositor, cx);
                    } else if let Some(picker) = compositor.top_mut::<Overlay<Self>>() {
                        picker.content.close_callback(None)(compositor, cx);
                    } else if let Some(picker) = compositor.top_mut::<Self>() {
                        picker.close_callback(None)(compositor, cx);
                    } else {
                        compositor.pop();
                    }
                }))
            },
        );
        EventResult::Consumed(Some(Box::new(move |compositor, _| {
            compositor.push(Box::new(prompt));
        })))
    }
}

fn open_targets(editor: &mut Editor, targets: &IndexSet<QuicklistTarget>, action: Action) -> bool {
    let mut first = None;
    let mut failed = 0;
    for target in targets {
        match open_target(editor, target) {
            Ok(id) => {
                first.get_or_insert(id);
            }
            Err(error) => {
                failed += 1;
                log::warn!("Could not open {target:?}: {error}");
            }
        }
    }
    if let Some(id) = first {
        editor.switch(id, action);
    }
    if failed > 0 {
        editor.set_error(|| {
            format!(
                "Could not open {failed} of {} files or buffers",
                targets.len()
            )
        });
    }
    first.is_some()
}

fn open_target(editor: &mut Editor, target: &QuicklistTarget) -> anyhow::Result<DocumentId> {
    match target {
        QuicklistTarget::Document(id) if editor.document(*id).is_some() => Ok(*id),
        QuicklistTarget::Document(_) => anyhow::bail!("Buffer no longer exists"),
        QuicklistTarget::Path(path) => {
            if let Some(id) = editor.document_id_by_path(path) {
                return Ok(id);
            }
            // Editor::open creates a new buffer for a missing path.
            path.metadata()?;
            Ok(editor.open(path, Action::Load)?)
        }
    }
}
