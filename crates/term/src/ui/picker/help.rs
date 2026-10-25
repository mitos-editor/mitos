//! Contextual shortcuts for all pickers, derived from their live component keymap.

use super::*;

impl<T: Send + Sync + 'static, D: Send + Sync + 'static> Picker<T, D> {
    pub(super) fn render_help(
        &self,
        area: Rect,
        mut inner: Rect,
        surface: &mut Surface,
        cx: &Context,
    ) -> Rect {
        if !cx.editor.config().picker.show_keybindings || area.height == 0 || area.width <= 4 {
            return inner;
        }
        let keys = (self.keymap)(&cx.config.current.component_keys);
        let mut actions = Vec::new();
        if self.replacement.is_some() {
            actions.push((
                PickerAction::ToggleReplacement,
                if self.replacement_visible() {
                    "switch input"
                } else {
                    "replace"
                },
            ));
        }
        if self.case_sensitive.is_some() {
            actions.push((PickerAction::ToggleCaseSensitive, "toggle case"));
        }
        if self.item_action_fn.is_some() {
            actions.extend([
                (
                    PickerAction::Item(PickerItemAction::CloseBuffer),
                    "close buffer",
                ),
                (
                    PickerAction::Item(PickerItemAction::RemoveQuicklistEntry),
                    "remove entry",
                ),
            ]);
        }
        actions.push((
            PickerAction::Accept,
            if self.replacement_visible() {
                "replace one"
            } else if self.file_fn.is_some() {
                "open"
            } else {
                "select"
            },
        ));
        if self.replacement_visible() {
            actions.push((PickerAction::ReplaceAll, "replace all"));
            actions.push((PickerAction::AcceptKeepOpen, "open"));
        }
        if self.file_fn.is_some() {
            actions.extend([
                (PickerAction::AcceptHorizontalSplit, "split below"),
                (PickerAction::AcceptVerticalSplit, "split right"),
                (PickerAction::TogglePreview, "preview"),
            ]);
        }
        if self.quicklist_fn.is_some() || self.file_fn.is_some() {
            actions.push((PickerAction::PopulateQuicklist, "quicklist"));
        }
        actions.push((PickerAction::Cancel, "close"));
        let hints = actions
            .into_iter()
            .filter_map(|(action, label)| {
                let key = crate::keymap::component::action_hint(keys, &action);
                (key != "unbound").then(|| format!("{key} {label}"))
            })
            .collect();
        let width = area.width.saturating_sub(4) as usize;
        let lines = help_lines(hints, width);
        let bottom_border = u16::from(inner.bottom() < area.bottom());
        let reserved = 3 + self.header_height() + if self.replacement_visible() { 2 } else { 0 };
        let height = lines.len().min(
            inner
                .height
                .saturating_sub(reserved)
                .saturating_add(bottom_border) as usize,
        ) as u16;
        let top = area.bottom().saturating_sub(height);
        inner.height = inner.height.min(top.saturating_sub(inner.y));
        for (row, line) in lines.into_iter().take(height as usize).enumerate() {
            surface.set_stringn(
                area.x + 2,
                top + row as u16,
                line,
                width,
                cx.editor.theme.get("ui.text.inactive"),
            );
        }
        inner
    }
}

/// Wrap whole hints so a shortcut and its action stay together.
fn help_lines(hints: Vec<String>, width: usize) -> Vec<String> {
    if width == 0 {
        return Vec::new();
    }
    let mut lines = Vec::new();
    let mut line = String::new();
    for hint in hints {
        if !line.is_empty()
            && Line::from(line.as_str()).width() + 3 + Line::from(hint.as_str()).width() > width
        {
            lines.push(std::mem::take(&mut line));
        }
        if !line.is_empty() {
            line.push_str(" · ");
        }
        line.push_str(&hint);
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

#[cfg(all(test, feature = "integration"))]
#[path = "help_tests.rs"]
mod tests;
