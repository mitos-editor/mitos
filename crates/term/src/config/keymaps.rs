//! Editor and component keymaps, including picker-specific inheritance.
use crate::keymap::component::{
    quicklist_picker_keymap, BlameAction, ComponentKeymaps, DiffAction, MenuAction, PageAction,
    PickerAction, PickerItemAction, PopupAction, PromptAction,
};
use crate::keymap::{self, KeyTrie};
use serde::{Deserialize, Deserializer};
use std::collections::HashMap;
use ui_core::keymap::Keymap;
use view::document::Mode;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct BufferPickerAction(PickerAction);

impl<'de> Deserialize<'de> for BufferPickerAction {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        use serde::de::IntoDeserializer;
        let action = String::deserialize(deserializer)?;
        let action = match action.as_str() {
            "close_buffer" => PickerAction::Item(PickerItemAction::CloseBuffer),
            action => PickerAction::deserialize(action.into_deserializer())?,
        };
        Ok(Self(action))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct QuicklistPickerAction(PickerAction);

impl<'de> Deserialize<'de> for QuicklistPickerAction {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        use serde::de::IntoDeserializer;
        let action = String::deserialize(deserializer)?;
        let action = match action.as_str() {
            "remove_entry" => PickerAction::Item(PickerItemAction::RemoveQuicklistEntry),
            action => PickerAction::deserialize(action.into_deserializer())?,
        };
        Ok(Self(action))
    }
}

/// User-facing `[keys]` table keeps editor and component schemas separate.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default, rename_all = "kebab-case", deny_unknown_fields)]
pub struct Keys {
    normal: Option<keymap::KeyTrieNode>,
    insert: Option<keymap::KeyTrieNode>,
    select: Option<keymap::KeyTrieNode>,
    prompt: Keymap<PromptAction>,
    picker: Keymap<PickerAction>,
    buffer_picker: Keymap<BufferPickerAction>,
    quicklist_picker: Keymap<QuicklistPickerAction>,
    menu: Keymap<MenuAction>,
    popup: Keymap<PopupAction>,
    hover: Keymap<PageAction>,
    signature_help: Keymap<PageAction>,
    blame: Keymap<BlameAction>,
    diff: Keymap<DiffAction>,
}

impl Keys {
    fn apply(
        self,
        editor: &mut HashMap<Mode, KeyTrie>,
        components: &mut ComponentKeymaps,
    ) -> (Keymap<PickerAction>, Keymap<PickerAction>) {
        for (mode, keys) in [
            (Mode::Normal, self.normal),
            (Mode::Insert, self.insert),
            (Mode::Select, self.select),
        ] {
            if let Some(keys) = keys {
                editor
                    .get_mut(&mode)
                    .unwrap()
                    .node_mut()
                    .unwrap()
                    .merge(keys);
            }
        }
        components.prompt.merge(self.prompt);
        components.picker.merge(self.picker);
        components.menu.merge(self.menu);
        components.popup.merge(self.popup);
        components.hover.merge(self.hover);
        components.signature_help.merge(self.signature_help);
        components.blame.merge(self.blame);
        components.diff.merge(self.diff);
        (
            self.buffer_picker.map_actions(|action| action.0),
            self.quicklist_picker.map_actions(|action| action.0),
        )
    }
}

pub(super) fn resolve_keymaps(
    configured: impl IntoIterator<Item = Keys>,
) -> (HashMap<Mode, KeyTrie>, ComponentKeymaps) {
    let mut editor = keymap::default();
    let mut components = ComponentKeymaps::default();
    let mut buffer_overrides = Keymap::default();
    let mut quicklist_overrides = Keymap::default();
    for keys in configured {
        let (buffer, quicklist) = keys.apply(&mut editor, &mut components);
        buffer_overrides.merge(buffer);
        quicklist_overrides.merge(quicklist);
    }
    // Resolve inheritance after both configuration layers: picker-specific
    // bindings take priority even over workspace changes to shared bindings.
    components.buffer_picker = components.picker.clone();
    components.buffer_picker.merge(buffer_overrides);
    components.quicklist_picker = quicklist_picker_keymap(&components.picker);
    components.quicklist_picker.merge(quicklist_overrides);
    (editor, components)
}
