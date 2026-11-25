use std::path::PathBuf;

use crate::ui::overlay::Overlay;

use super::*;

const OPEN_ALL_CONFIRM_THRESHOLD: usize = 50;

impl<T: Send + Sync + 'static, D: Send + Sync + 'static> Picker<T, D> {
    pub(super) fn open_all(&mut self, cx: &mut Context) -> EventResult {
        let Some(path) = self.open_all_path else {
            return EventResult::Consumed(None);
        };
        // The user may press the shortcut before the initial scan or the
        // latest query has made it into the matcher's snapshot.
        if self.results.matcher.active_injectors() > 0 || self.results.tick(10).running {
            cx.editor.set_status("File results are still loading");
            return EventResult::Consumed(None);
        }
        let paths: Vec<_> = self
            .results
            .matcher
            .snapshot()
            .matched_items(..)
            .filter_map(|item| path(item.data).map(Path::to_path_buf))
            .collect();
        if paths.is_empty() {
            cx.editor.set_status("No matching files");
            return EventResult::Consumed(None);
        }
        let action = self.default_action;
        if paths.len() <= OPEN_ALL_CONFIRM_THRESHOLD {
            return if open_files(cx.editor, &paths, action) {
                EventResult::Consumed(Some(self.close_callback(None)))
            } else {
                EventResult::Consumed(None)
            };
        }

        let on_close = self.on_close;
        let prompt = Prompt::new_with_callback(
            format!("Open {} files? [y/N] ", paths.len()).into(),
            None,
            ui::completers::none,
            move |cx, input, event| {
                if event != PromptEvent::Validate || !input.trim().eq_ignore_ascii_case("y") {
                    return None;
                }
                if !open_files(cx.editor, &paths, action) {
                    return None;
                }
                Some(Box::new(move |compositor, cx| {
                    if let Some(close) = on_close {
                        close(compositor, cx);
                    } else if let Some(picker) = compositor.find::<Overlay<Self>>() {
                        picker.content.close_callback(None)(compositor, cx);
                    }
                }))
            },
        );
        EventResult::Consumed(Some(Box::new(move |compositor, _| {
            compositor.push(Box::new(prompt));
        })))
    }
}

fn open_files(editor: &mut Editor, paths: &[PathBuf], action: Action) -> bool {
    let mut first = None;
    let mut failed = 0;
    for path in paths {
        // Editor::open creates a new buffer for a missing path. A bulk open
        // should skip files that disappeared while the picker was open.
        let result = path
            .metadata()
            .map_err(anyhow::Error::from)
            .and_then(|_| editor.open(path, Action::Load).map_err(anyhow::Error::from));
        match result {
            Ok(id) => {
                first.get_or_insert(id);
            }
            Err(error) => {
                failed += 1;
                log::warn!("Could not open {}: {error}", path.display());
            }
        }
    }
    if let Some(id) = first {
        editor.switch(id, action);
    }
    if failed > 0 {
        editor.set_error(|| format!("Could not open {failed} of {} files", paths.len()));
    }
    first.is_some()
}
