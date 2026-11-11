//! Picker-specific shortcuts, clipped to one line and derived from the live keymap.

use super::*;
use crate::compositor::DrawContext;

impl<T: Send + Sync + 'static, D: Send + Sync + 'static> Picker<T, D> {
    pub(in crate::ui) fn query_contexts(
        &self,
        cx: &Context,
    ) -> Vec<crate::keymap::inspect::Context> {
        self.active_prompt().keymap_contexts(cx)
    }

    pub(in crate::ui) fn action_available(
        &self,
        maps: &ComponentKeymaps,
        action: PickerAction,
    ) -> bool {
        use PickerAction as A;
        use PickerItemAction as I;
        let preview = self.file_fn.is_some() || self.custom_preview.is_some();
        match action {
            A::ToggleReplacement => self.replacement.is_some(),
            A::ReplaceAll => self.replacement_visible(),
            A::ToggleCaseSensitive => self.case_sensitive.is_some(),
            A::ToggleLoading => self
                .editor_updates
                .as_ref()
                .is_some_and(|updates| updates.paged),
            A::TogglePreview | A::PreviewScrollUp | A::PreviewScrollDown => preview,
            A::OpenPreview | A::PreviewPrevious | A::PreviewNext => self.custom_preview.is_some(),
            A::AcceptHorizontalSplit | A::AcceptVerticalSplit => preview,
            A::PopulateQuicklist => self.quicklist_fn.is_some() || self.file_fn.is_some(),
            A::HistoryPrevious | A::HistoryNext => {
                self.active_prompt().history_register().is_some()
            }
            A::Item(item) if self.item_action_fn.is_some() => {
                match maps.picker_table((self.keymap)(maps)) {
                    "keys.buffer-picker" => matches!(item, I::CloseBuffer),
                    "keys.quicklist-picker" => matches!(item, I::RemoveQuicklistEntry),
                    "keys.git-picker" => matches!(
                        item,
                        I::Stage | I::Unstage | I::Stash | I::Stashes | I::Actions
                    ),
                    "keys.branch-picker" => matches!(item, I::CreateBranch | I::DeleteBranch),
                    "keys.worktree-picker" => matches!(
                        item,
                        I::CreateWorktree | I::RemoveWorktree | I::LockWorktree | I::UnlockWorktree
                    ),
                    "keys.stash-picker" => {
                        matches!(item, I::ApplyStash | I::ApplyStashWithIndex | I::Actions)
                    }
                    "keys.history-picker" => matches!(item, I::HistoryBack | I::Actions),
                    "keys.file-explorer" => matches!(
                        item,
                        I::CreatePath | I::MovePath | I::CopyPath | I::DeletePath | I::YankPath
                    ),
                    _ => true,
                }
            }
            A::Item(_) => false,
            _ => true,
        }
    }

    pub(super) fn render_action_menu(&self, area: Rect, surface: &mut Surface, cx: &DrawContext) {
        if self.on_close.is_some() || !self.keys.is_pending() {
            return;
        }
        let maps = &cx.config.component_keys;
        let keys = crate::keymap::inspect::filter((self.keymap)(maps), |action| {
            self.action_available(maps, *action)
        });
        if let Some(pending) = self.keys.pending_map(&keys) {
            let info = crate::keymap::inspect::prefix_info(pending, |action| {
                action.description().to_owned()
            });
            super::super::info::render(&info, area, surface, cx);
        }
    }

    pub(super) fn render_help(&self, area: Rect, surface: &mut Surface, cx: &DrawContext) {
        if self.on_close.is_some()
            || !cx.editor.config().picker.show_keybindings
            || area.height == 0
            || area.width <= 4
        {
            return;
        }
        let keys = (self.keymap)(&cx.config.component_keys);
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
        if self
            .editor_updates
            .as_ref()
            .is_some_and(|updates| updates.paged)
        {
            actions.push((PickerAction::ToggleLoading, "pause / continue / retry"));
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
                (PickerAction::Item(PickerItemAction::CreatePath), "new"),
                (PickerAction::Item(PickerItemAction::MovePath), "move"),
                (PickerAction::Item(PickerItemAction::CopyPath), "copy"),
                (PickerAction::Item(PickerItemAction::DeletePath), "delete"),
                (PickerAction::Item(PickerItemAction::YankPath), "yank path"),
            ]);
        }
        if self.show_preview && self.file_fn.is_some() && self.custom_preview.is_none() {
            actions.extend([
                (PickerAction::PreviewScrollDown, "scroll preview down"),
                (PickerAction::PreviewScrollUp, "scroll preview up"),
            ]);
        }
        let mut prefixes: Vec<_> = keys
            .0
            .iter()
            .filter(|(_, binding)| matches!(binding, ui_core::keymap::Binding::Keys(_)))
            .map(|(key, _)| format!("{} actions", key.key_sequence_format()))
            .collect();
        prefixes.sort();
        let mut hints = prefixes;
        hints.extend(
            self.help
                .map_or_else(Vec::new, |help| help(&cx.config.component_keys)),
        );
        hints.extend(actions.into_iter().filter_map(|(action, label)| {
            let key = crate::keymap::component::action_hint(keys, &action);
            (key != "unbound").then(|| format!("{key} {label}"))
        }));
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

#[cfg(test)]
#[path = "help_tests.rs"]
mod tests;
