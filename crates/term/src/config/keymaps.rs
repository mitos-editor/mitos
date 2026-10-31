//! Editor and component keymaps, including picker-specific inheritance.
use crate::keymap::component::{
    quicklist_picker_keymap, BlameAction, ComponentKeymaps, DiffAction, MenuAction, PageAction,
    PickerAction, PickerItemAction, PopupAction, ProjectReviewAction, PromptAction,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct GitPickerAction(PickerAction);
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct StashPickerAction(PickerAction);
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct HistoryPickerAction(PickerAction);

macro_rules! picker_actions {
    ($schema:ident, {$($name:literal => $action:ident),* $(,)?}) => {
        impl<'de> Deserialize<'de> for $schema {
            fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                use serde::de::IntoDeserializer;
                let action = String::deserialize(deserializer)?;
                Ok(Self(match action.as_str() {
                    $($name => PickerAction::Item(PickerItemAction::$action),)*
                    action => PickerAction::deserialize(action.into_deserializer())?,
                }))
            }
        }
    };
}
picker_actions!(GitPickerAction, { "stage" => Stage, "unstage" => Unstage, "stash" => Stash });
picker_actions!(HistoryPickerAction, { "open_version" => HistoryOpenVersion, "compare_buffer" => HistoryCompareBuffer, "compare_parent" => HistoryCompareParent, "back" => HistoryBack });
picker_actions!(StashPickerAction, { "apply_stash" => ApplyStash, "apply_stash_with_index" => ApplyStashWithIndex });

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
    git_picker: Keymap<GitPickerAction>,
    stash_picker: Keymap<StashPickerAction>,
    history_picker: Keymap<HistoryPickerAction>,
    menu: Keymap<MenuAction>,
    popup: Keymap<PopupAction>,
    hover: Keymap<PageAction>,
    signature_help: Keymap<PageAction>,
    blame: Keymap<BlameAction>,
    diff: Keymap<DiffAction>,
    project_review: Keymap<ProjectReviewAction>,
}

impl Keys {
    fn apply(
        self,
        editor: &mut HashMap<Mode, KeyTrie>,
        components: &mut ComponentKeymaps,
    ) -> [Keymap<PickerAction>; 5] {
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
        components.project_review.merge(self.project_review);
        [
            self.buffer_picker.map_actions(|action| action.0),
            self.quicklist_picker.map_actions(|action| action.0),
            self.git_picker.map_actions(|action| action.0),
            self.stash_picker.map_actions(|action| action.0),
            self.history_picker.map_actions(|action| action.0),
        ]
    }
}

pub(super) fn resolve_keymaps(
    configured: impl IntoIterator<Item = Keys>,
) -> (HashMap<Mode, KeyTrie>, ComponentKeymaps) {
    let mut editor = keymap::default();
    let mut components = ComponentKeymaps::default();
    let [mut buffer_overrides, mut quicklist_overrides, mut git_overrides, mut stash_overrides, mut history_overrides] =
        std::array::from_fn(|_| Keymap::default());
    for keys in configured {
        let [buffer, quicklist, git, stash, history] = keys.apply(&mut editor, &mut components);
        buffer_overrides.merge(buffer);
        quicklist_overrides.merge(quicklist);
        git_overrides.merge(git);
        stash_overrides.merge(stash);
        history_overrides.merge(history);
    }
    // Resolve inheritance after both configuration layers: picker-specific
    // bindings take priority even over workspace changes to shared bindings.
    components.buffer_picker = components.picker.clone();
    components.buffer_picker.merge(buffer_overrides);
    components.quicklist_picker = quicklist_picker_keymap(&components.picker);
    components.quicklist_picker.merge(quicklist_overrides);
    components.git_picker = ComponentKeymaps::git_picker(components.picker.clone());
    components.git_picker.merge(git_overrides);
    components.stash_picker = ComponentKeymaps::stash_picker(components.picker.clone());
    components.stash_picker.merge(stash_overrides);
    components.history_picker = ComponentKeymaps::history_picker(components.picker.clone());
    components.history_picker.merge(history_overrides);
    (editor, components)
}
