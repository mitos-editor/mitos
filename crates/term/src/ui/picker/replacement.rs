//! Optional replacement input and actions; result publication stays in `results`.

use super::*;

type ReplacementCallback<T, D> =
    Box<dyn Fn(&mut Context, &[&T], &str, &str, bool, &D) -> anyhow::Result<usize>>;
type CaseSensitiveCallback<D> = Box<dyn Fn(&D, bool)>;

pub(super) struct Replacement<T, D> {
    pub prompt: Prompt,
    callback: ReplacementCallback<T, D>,
    visible: bool,
    active: bool,
}

pub(super) struct CaseSensitiveToggle<D> {
    pub enabled: bool,
    callback: CaseSensitiveCallback<D>,
}

pub(super) struct PickerInputAreas {
    pub search: Rect,
    pub search_separator: Rect,
    pub replacement: Option<Rect>,
    pub replacement_separator: Option<Rect>,
    pub results: Rect,
}

pub(super) fn split_picker_input_area(area: Rect, show_replacement: bool) -> PickerInputAreas {
    let [search, search_separator, remaining] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(0),
    ])
    .areas(area);
    let (replacement, replacement_separator, results) = if show_replacement {
        let [input, separator, results] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(0),
        ])
        .areas(remaining);
        (Some(input), Some(separator), results)
    } else {
        (None, None, remaining)
    };
    PickerInputAreas {
        search,
        search_separator,
        replacement,
        replacement_separator,
        results,
    }
}

impl<T: Send + Sync + 'static, D: Send + Sync + 'static> Picker<T, D> {
    pub(crate) fn with_replacement(
        mut self,
        callback: impl Fn(&mut Context, &[&T], &str, &str, bool, &D) -> anyhow::Result<usize> + 'static,
    ) -> Self {
        self.replacement = Some(Replacement {
            prompt: Prompt::new("replace: ".into(), None, ui::completers::none, |_, _, _| {}),
            callback: Box::new(callback),
            visible: false,
            active: false,
        });
        self
    }

    pub(crate) fn with_case_sensitive_toggle(
        mut self,
        enabled: bool,
        callback: impl Fn(&D, bool) + 'static,
    ) -> Self {
        self.case_sensitive = Some(CaseSensitiveToggle {
            enabled,
            callback: Box::new(callback),
        });
        self
    }

    pub(super) fn replacement_visible(&self) -> bool {
        self.replacement
            .as_ref()
            .is_some_and(|replacement| replacement.visible)
    }

    pub(super) fn replacement_active(&self) -> bool {
        self.replacement
            .as_ref()
            .is_some_and(|replacement| replacement.active)
    }

    pub(super) fn active_prompt(&self) -> &Prompt {
        if self.replacement_active() {
            &self.replacement.as_ref().unwrap().prompt
        } else {
            &self.prompt
        }
    }

    pub(super) fn active_prompt_mut(&mut self) -> &mut Prompt {
        if self.replacement_active() {
            &mut self.replacement.as_mut().unwrap().prompt
        } else {
            &mut self.prompt
        }
    }

    pub(super) fn toggle_replacement_prompt(&mut self) {
        let Some(replacement) = &mut self.replacement else {
            return;
        };
        replacement.active = !replacement.active;
        replacement.visible = true;
        self.prompt.set_prompt("search: ");
    }

    pub(super) fn toggle_case_sensitive(&mut self, editor: &mut Editor) {
        let Some(toggle) = &mut self.case_sensitive else {
            return;
        };
        toggle.enabled = !toggle.enabled;
        (toggle.callback)(&self.results.editor_data, toggle.enabled);
        if let Some(updates) = &mut self.file_updates {
            updates.stop();
        }
        self.request_dynamic_refresh(true);
        editor.set_status(if self.case_sensitive.as_ref().unwrap().enabled {
            "Global search is case-sensitive"
        } else {
            "Global search is using smart case"
        });
    }

    pub(super) fn replace_matches(&mut self, cx: &mut Context, replace_all: bool) {
        // Detect changes even when an action arrives before the next redraw.
        self.refresh_files(cx.editor);
        self.refresh_query_if_changed(cx.editor);
        self.publish_options();
        if self.dynamic_query_pending
            || self.query_refresh
            || self.results.is_refreshing()
            || self
                .file_updates
                .as_ref()
                .is_some_and(|updates| updates.is_pending())
            || self.results.matcher.tick(0).running
            || self.results.matcher.active_injectors() > 0
        {
            cx.editor
                .set_error(|| "Wait for global search to finish before replacing matches.");
            return;
        }
        let Some(replacement) = &self.replacement else {
            return;
        };
        let query = self.primary_query();
        if query.is_empty() {
            cx.editor.set_error(|| "Enter a search pattern first.");
            return;
        }
        let snapshot = self.results.matcher.snapshot();
        let items: Vec<_> = if replace_all {
            snapshot
                .matched_items(0..snapshot.matched_item_count())
                .map(|item| item.data)
                .collect()
        } else {
            self.selection().into_iter().collect()
        };
        if items.is_empty() {
            cx.editor.set_status("No matches to replace");
            return;
        }
        let result = (replacement.callback)(
            cx,
            &items,
            &query,
            replacement.prompt.line(),
            replace_all,
            &self.results.editor_data,
        );
        match result {
            Ok(0) => cx.editor.set_status("No matches replaced"),
            Ok(count) => {
                // Refresh coordinates and preview from the changed buffers. Do not
                // leave stale rows actionable while the next generation loads.
                if let Some(updates) = &mut self.file_updates {
                    updates.stop();
                }
                self.request_dynamic_refresh(true);
                cx.editor.set_status(format!(
                    "Replaced {count} {}",
                    if count == 1 { "match" } else { "matches" }
                ));
            }
            Err(error) => cx.editor.set_error(|| format!("Replace failed: {error}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replacement_input_has_its_own_row_and_separator() {
        let areas = split_picker_input_area(Rect::new(3, 5, 40, 12), true);
        assert_eq!(areas.search, Rect::new(3, 5, 40, 1));
        assert_eq!(areas.search_separator, Rect::new(3, 6, 40, 1));
        assert_eq!(areas.replacement, Some(Rect::new(3, 7, 40, 1)));
        assert_eq!(areas.replacement_separator, Some(Rect::new(3, 8, 40, 1)));
        assert_eq!(areas.results, Rect::new(3, 9, 40, 8));
        for width in 0..4 {
            for height in 0..5 {
                let area = Rect::new(3, 5, width, height);
                let areas = split_picker_input_area(area, true);
                for rect in [
                    areas.search,
                    areas.search_separator,
                    areas.replacement.unwrap(),
                    areas.replacement_separator.unwrap(),
                    areas.results,
                ] {
                    assert!(rect.x >= area.x && rect.y >= area.y);
                    assert!(rect.right() <= area.right() && rect.bottom() <= area.bottom());
                }
            }
        }
    }
}
