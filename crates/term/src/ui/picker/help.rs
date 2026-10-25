//! Picker-specific shortcuts, clipped to one line and derived from the live keymap.

use super::*;

impl<T: Send + Sync + 'static, D: Send + Sync + 'static> Picker<T, D> {
    pub(super) fn render_help(&self, area: Rect, surface: &mut Surface, cx: &Context) {
        if !cx.editor.config().picker.show_keybindings || area.height == 0 || area.width <= 4 {
            return;
        }
        let keys = (self.keymap)(&cx.config.current.component_keys);
        let mut actions = Vec::new();
        if self.replacement_visible() {
            actions.extend([
                (PickerAction::Accept, "replace one"),
                (PickerAction::ReplaceAll, "replace all"),
            ]);
        }
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
        let mut hints = self.help.map_or_else(Vec::new, |help| {
            help(&cx.config.current.component_keys)
        });
        hints.extend(actions
            .into_iter()
            .filter_map(|(action, label)| {
                let key = crate::keymap::component::action_hint(keys, &action);
                (key != "unbound").then(|| format!("{key} {label}"))
            })
        );
        if hints.is_empty() {
            return;
        }
        let width = area.width.saturating_sub(4) as usize;
        // Use the existing bottom border so hints never consume result rows.
        let top = area.bottom() - 1;
        surface.set_stringn(
            area.x + 2,
            top,
            hints.join(" · "),
            width,
            cx.editor.theme.get("ui.text.inactive"),
        );
    }
}

#[cfg(all(test, feature = "integration"))]
#[path = "help_tests.rs"]
mod tests;
