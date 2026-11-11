//! Editor and component keymaps, including picker-specific inheritance.
use crate::keymap::component::{
    quicklist_picker_keymap, BlameAction, ComponentKeymaps, DiffAction, MenuAction, PageAction,
    PickerAction, PickerItemAction, PopupAction, PromptAction, SidebarAction,
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
struct BranchPickerAction(PickerAction);
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct WorktreePickerAction(PickerAction);
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct HistoryPickerAction(PickerAction);
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FileExplorerAction(PickerAction);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SidebarBinding(SidebarAction);

impl<'de> Deserialize<'de> for SidebarBinding {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        use serde::de::IntoDeserializer;
        let action = String::deserialize(deserializer)?;
        Ok(Self(match action.as_str() {
            "filter" => SidebarAction::Filter,
            "focus_editor" => SidebarAction::FocusEditor,
            "hide" => SidebarAction::Hide,
            "next_view" => SidebarAction::NextView,
            "previous_view" => SidebarAction::PreviousView,
            "resize" => SidebarAction::Resize,
            action => SidebarAction::Picker(
                FileExplorerAction::deserialize(action.into_deserializer())?.0,
            ),
        }))
    }
}

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
picker_actions!(GitPickerAction, { "stage" => Stage, "unstage" => Unstage, "stash" => Stash, "actions" => Actions, "stashes" => Stashes });
picker_actions!(BranchPickerAction, {"create_branch" => CreateBranch, "delete_branch" => DeleteBranch});
picker_actions!(WorktreePickerAction, {"create_worktree" => CreateWorktree, "remove_worktree" => RemoveWorktree, "lock_worktree" => LockWorktree, "unlock_worktree" => UnlockWorktree});
picker_actions!(HistoryPickerAction, {"actions" => Actions, "back" => HistoryBack});
picker_actions!(FileExplorerAction, { "create_path" => CreatePath, "move_path" => MovePath, "copy_path" => CopyPath, "delete_path" => DeletePath, "yank_path" => YankPath });
picker_actions!(StashPickerAction, { "apply_stash" => ApplyStash, "apply_stash_with_index" => ApplyStashWithIndex, "actions" => Actions });

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
    branch_picker: Keymap<BranchPickerAction>,
    worktree_picker: Keymap<WorktreePickerAction>,
    stash_picker: Keymap<StashPickerAction>,
    history_picker: Keymap<HistoryPickerAction>,
    file_explorer: Keymap<FileExplorerAction>,
    sidebar: Keymap<SidebarBinding>,
    sidebar_filter: Keymap<SidebarBinding>,
    menu: Keymap<MenuAction>,
    popup: Keymap<PopupAction>,
    hover: Keymap<PageAction>,
    signature_help: Keymap<PageAction>,
    blame: Keymap<BlameAction>,
    diff: Keymap<DiffAction>,
}

impl Keys {
    pub(super) fn validate_commands(
        &self,
        custom: &editor_config::CustomCommands,
    ) -> Result<(), toml::de::Error> {
        use crate::commands::{MappableCommand, TYPABLE_COMMAND_MAP};
        use serde::de::Error;

        fn validate(
            command: &MappableCommand,
            custom: &editor_config::CustomCommands,
        ) -> Result<(), toml::de::Error> {
            if let MappableCommand::Typable { name, .. } = command {
                let builtin = name.strip_prefix(editor_config::CustomCommand::ESCAPE);
                if !(builtin.is_none() && custom.get(name).is_some())
                    && !TYPABLE_COMMAND_MAP.contains_key(builtin.unwrap_or(name))
                {
                    return Err(toml::de::Error::custom(format!(
                        "No TypableCommand named ':{name}'"
                    )));
                }
            }
            Ok(())
        }

        fn visit(
            node: &keymap::KeyTrieNode,
            custom: &editor_config::CustomCommands,
        ) -> Result<(), toml::de::Error> {
            for binding in node.values() {
                match binding {
                    KeyTrie::MappableCommand(command) => validate(command, custom)?,
                    KeyTrie::Sequence(_, commands) => {
                        for command in commands {
                            validate(command, custom)?;
                        }
                    }
                    KeyTrie::Node(node) => visit(node, custom)?,
                }
            }
            Ok(())
        }

        for node in [&self.normal, &self.insert, &self.select]
            .into_iter()
            .flatten()
        {
            visit(node, custom)?;
        }
        Ok(())
    }

    fn apply(
        self,
        editor: &mut HashMap<Mode, KeyTrie>,
        components: &mut ComponentKeymaps,
    ) -> ([Keymap<PickerAction>; 8], [Keymap<SidebarAction>; 2]) {
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
            [
                self.buffer_picker.map_actions(|action| action.0),
                self.quicklist_picker.map_actions(|action| action.0),
                self.git_picker.map_actions(|action| action.0),
                self.stash_picker.map_actions(|action| action.0),
                self.history_picker.map_actions(|action| action.0),
                self.file_explorer.map_actions(|action| action.0),
                self.branch_picker.map_actions(|action| action.0),
                self.worktree_picker.map_actions(|action| action.0),
            ],
            [
                self.sidebar.map_actions(|action| action.0),
                self.sidebar_filter.map_actions(|action| action.0),
            ],
        )
    }
}

pub(super) fn resolve_keymaps(
    configured: impl IntoIterator<Item = Keys>,
) -> (HashMap<Mode, KeyTrie>, ComponentKeymaps) {
    let mut editor = keymap::default();
    let mut components = ComponentKeymaps::default();
    let [mut buffer_overrides, mut quicklist_overrides, mut git_overrides, mut stash_overrides, mut history_overrides, mut explorer_overrides, mut branch_overrides, mut worktree_overrides] =
        std::array::from_fn(|_| Keymap::default());
    let mut sidebar_overrides = Keymap::default();
    let mut sidebar_filter_overrides = Keymap::default();
    for keys in configured {
        let (
            [buffer, quicklist, git, stash, history, explorer, branch, worktree],
            [sidebar, filter],
        ) = keys.apply(&mut editor, &mut components);
        sidebar_overrides.merge(sidebar);
        sidebar_filter_overrides.merge(filter);
        buffer_overrides.merge(buffer);
        quicklist_overrides.merge(quicklist);
        git_overrides.merge(git);
        stash_overrides.merge(stash);
        history_overrides.merge(history);
        explorer_overrides.merge(explorer);
        branch_overrides.merge(branch);
        worktree_overrides.merge(worktree);
    }
    // Resolve inheritance after both configuration layers: picker-specific
    // bindings take priority even over workspace changes to shared bindings.
    components.buffer_picker = ComponentKeymaps::buffer_picker(components.picker.clone());
    components.buffer_picker.merge(buffer_overrides);
    components.quicklist_picker = quicklist_picker_keymap(&components.picker);
    components.quicklist_picker.merge(quicklist_overrides);
    components.git_picker = ComponentKeymaps::git_picker(components.picker.clone());
    components.git_picker.merge(git_overrides);
    components.branch_picker = ComponentKeymaps::branch_picker(components.picker.clone());
    components.branch_picker.merge(branch_overrides);
    components.worktree_picker = ComponentKeymaps::worktree_picker(components.picker.clone());
    components.worktree_picker.merge(worktree_overrides);
    components.stash_picker = ComponentKeymaps::stash_picker(components.picker.clone());
    components.stash_picker.merge(stash_overrides);
    components.history_picker = ComponentKeymaps::history_picker(components.picker.clone());
    components.history_picker.merge(history_overrides);
    components.file_explorer = ComponentKeymaps::file_explorer(components.picker.clone());
    components.file_explorer.merge(explorer_overrides);
    components.sidebar = ComponentKeymaps::sidebar(components.file_explorer.clone());
    components.sidebar.merge(sidebar_overrides);
    components.sidebar_filter = ComponentKeymaps::sidebar_filter(components.file_explorer.clone());
    components.sidebar_filter.merge(sidebar_filter_overrides);
    (editor, components)
}
