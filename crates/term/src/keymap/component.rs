//! Actions and defaults owned by the terminal components that execute them.

use serde::{Deserialize, Serialize};
use ui_core::keymap::{Binding, Keymap};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptAction {
    Cancel,
    Accept,
    MoveWordLeft,
    MoveWordRight,
    MoveLeft,
    MoveRight,
    MoveStart,
    MoveEnd,
    DeleteWordBackward,
    DeleteWordForward,
    DeleteToStart,
    DeleteToEnd,
    DeleteBackward,
    DeleteForward,
    InsertWord,
    HistoryPrevious,
    HistoryNext,
    CompleteNext,
    CompletePrevious,
    ExitSelection,
    InsertRegister,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PickerAction {
    HistoryPrevious,
    HistoryNext,
    MoveUp,
    MoveDown,
    PageUp,
    PageDown,
    MoveStart,
    MoveEnd,
    Cancel,
    Accept,
    AcceptKeepOpen,
    AcceptHorizontalSplit,
    AcceptVerticalSplit,
    TogglePreview,
    ToggleLoading,
    ToggleReplacement,
    ToggleCaseSensitive,
    ReplaceAll,
    PreviewScrollUp,
    PreviewScrollDown,
    PreviewPrevious,
    PreviewNext,
    OpenPreview,
    PopulateQuicklist,
    /// Item actions are supplied only by the schema for a specific picker.
    #[serde(untagged, skip_deserializing)]
    Item(PickerItemAction),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PickerItemAction {
    CloseBuffer,
    #[serde(rename = "remove_entry")]
    RemoveQuicklistEntry,
    Stage,
    Unstage,
    Stash,
    Actions,
    Stashes,
    ApplyStash,
    ApplyStashWithIndex,
    #[serde(rename = "back")]
    HistoryBack,
    CreateBranch,
    DeleteBranch,
    CreateWorktree,
    RemoveWorktree,
    LockWorktree,
    UnlockWorktree,
    CreatePath,
    MovePath,
    CopyPath,
    DeletePath,
    YankPath,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SidebarAction {
    Filter,
    FocusEditor,
    Hide,
    NextView,
    PreviousView,
    Resize,
    #[serde(untagged)]
    Picker(PickerAction),
}

impl SidebarAction {
    pub fn description(self) -> &'static str {
        match self {
            Self::Filter => "Filter files",
            Self::FocusEditor => "Focus document window",
            Self::Hide => "Hide file explorer",
            Self::NextView => "Focus next window",
            Self::PreviousView => "Focus previous window",
            Self::Resize => "Resize file explorer",
            Self::Picker(action) => action.description(),
        }
    }
}

impl PickerAction {
    pub fn description(self) -> &'static str {
        match self {
            Self::HistoryPrevious => "Previous query",
            Self::HistoryNext => "Next query",
            Self::MoveUp => "Move up",
            Self::MoveDown => "Move down",
            Self::PageUp => "Page up",
            Self::PageDown => "Page down",
            Self::MoveStart => "Move to start",
            Self::MoveEnd => "Move to end",
            Self::Cancel => "Cancel",
            Self::Accept => "Open selection",
            Self::AcceptKeepOpen => "Open selection and keep explorer focused",
            Self::AcceptHorizontalSplit => "Open in horizontal split",
            Self::AcceptVerticalSplit => "Open in vertical split",
            Self::TogglePreview => "Toggle preview",
            Self::ToggleLoading => "Pause loading, continue, or retry",
            Self::ToggleReplacement => "Toggle replacement input",
            Self::ToggleCaseSensitive => "Toggle case sensitivity",
            Self::ReplaceAll => "Replace all",
            Self::PreviewScrollUp => "Scroll preview up",
            Self::PreviewScrollDown => "Scroll preview down",
            Self::PreviewPrevious => "Previous preview",
            Self::PreviewNext => "Next preview",
            Self::OpenPreview => "Open preview",
            Self::PopulateQuicklist => "Populate quicklist",
            Self::Item(action) => match action {
                PickerItemAction::CloseBuffer => "Close buffer",
                PickerItemAction::RemoveQuicklistEntry => "Remove entry",
                PickerItemAction::Stage => "Stage",
                PickerItemAction::Unstage => "Unstage",
                PickerItemAction::Stash => "Stash",
                PickerItemAction::Actions => "Show actions",
                PickerItemAction::Stashes => "Show stashes",
                PickerItemAction::ApplyStash => "Apply stash",
                PickerItemAction::ApplyStashWithIndex => "Apply stash with index",
                PickerItemAction::HistoryBack => "Go back",
                PickerItemAction::CreatePath => "Create file or directory",
                PickerItemAction::MovePath => "Move file or directory",
                PickerItemAction::CopyPath => "Copy file or directory",
                PickerItemAction::DeletePath => "Delete file or directory",
                PickerItemAction::YankPath => "Yank path",
                PickerItemAction::CreateBranch => "Create branch",
                PickerItemAction::DeleteBranch => "Delete branch",
                PickerItemAction::CreateWorktree => "Create worktree",
                PickerItemAction::RemoveWorktree => "Remove worktree",
                PickerItemAction::LockWorktree => "Lock worktree",
                PickerItemAction::UnlockWorktree => "Unlock worktree",
            },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MenuAction {
    Cancel,
    Accept,
    MoveUp,
    MoveDown,
    PageUp,
    PageDown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PopupAction {
    Cancel,
    ScrollUp,
    ScrollDown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PageAction {
    Previous,
    Next,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BlameAction {
    CopyCommitId,
    CopyMessage,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DiffAction {
    ScrollLeft,
    ScrollRight,
    Restore,
}

/// Terminal component maps, resolved from defaults and user configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComponentKeymaps {
    pub prompt: Keymap<PromptAction>,
    pub picker: Keymap<PickerAction>,
    pub buffer_picker: Keymap<PickerAction>,
    pub quicklist_picker: Keymap<PickerAction>,
    pub git_picker: Keymap<PickerAction>,
    pub branch_picker: Keymap<PickerAction>,
    pub worktree_picker: Keymap<PickerAction>,
    pub stash_picker: Keymap<PickerAction>,
    pub history_picker: Keymap<PickerAction>,
    pub file_explorer: Keymap<PickerAction>,
    pub sidebar: Keymap<SidebarAction>,
    pub sidebar_filter: Keymap<SidebarAction>,
    pub menu: Keymap<MenuAction>,
    pub popup: Keymap<PopupAction>,
    pub hover: Keymap<PageAction>,
    pub signature_help: Keymap<PageAction>,
    pub blame: Keymap<BlameAction>,
    pub diff: Keymap<DiffAction>,
}

fn bindings<A: Copy>(entries: &[(&str, A)]) -> Keymap<A> {
    fn insert<A: Copy>(keys: &mut Keymap<A>, path: &[&str], action: A) {
        let key = path[0].parse().unwrap();
        if path.len() == 1 {
            assert!(
                keys.0.insert(key, Binding::Actions(vec![action])).is_none(),
                "duplicate default binding: {}",
                path[0]
            );
        } else {
            let entry = keys
                .0
                .entry(key)
                .or_insert_with(|| Binding::Keys(Keymap::default()));
            let Binding::Keys(nested) = entry else {
                panic!("conflicting default prefix")
            };
            insert(nested, &path[1..], action);
        }
    }
    let mut keys = Keymap::default();
    for &(path, action) in entries {
        insert(
            &mut keys,
            &path.split_ascii_whitespace().collect::<Vec<_>>(),
            action,
        );
    }
    keys
}

pub(crate) fn quicklist_picker_keymap(picker: &Keymap<PickerAction>) -> Keymap<PickerAction> {
    let mut keys = picker.clone();
    keys.merge(bindings(&[(
        "C-x d",
        PickerAction::Item(PickerItemAction::RemoveQuicklistEntry),
    )]));
    keys
}

impl ComponentKeymaps {
    /// Identify the configured table selected by a picker's keymap accessor.
    pub fn picker_table(&self, keys: &Keymap<PickerAction>) -> &'static str {
        for (name, candidate) in [
            ("keys.buffer-picker", &self.buffer_picker),
            ("keys.quicklist-picker", &self.quicklist_picker),
            ("keys.git-picker", &self.git_picker),
            ("keys.branch-picker", &self.branch_picker),
            ("keys.worktree-picker", &self.worktree_picker),
            ("keys.stash-picker", &self.stash_picker),
            ("keys.history-picker", &self.history_picker),
            ("keys.file-explorer", &self.file_explorer),
        ] {
            if std::ptr::eq(keys, candidate) {
                return name;
            }
        }
        "keys.picker"
    }

    pub fn buffer_picker(mut shared: Keymap<PickerAction>) -> Keymap<PickerAction> {
        shared.merge(bindings(&[(
            "C-x d",
            PickerAction::Item(PickerItemAction::CloseBuffer),
        )]));
        shared
    }
    pub fn sidebar_filter(explorer: Keymap<PickerAction>) -> Keymap<SidebarAction> {
        let mut keys = explorer.map_actions(SidebarAction::Picker);
        keys.merge(bindings(&[("C-e", SidebarAction::FocusEditor)]));
        keys
    }

    pub fn sidebar(explorer: Keymap<PickerAction>) -> Keymap<SidebarAction> {
        use SidebarAction as S;
        let mut keys = Self::sidebar_filter(explorer);
        keys.merge(bindings(&[
            ("/", S::Filter),
            ("j", S::Picker(PickerAction::MoveDown)),
            ("k", S::Picker(PickerAction::MoveUp)),
            (
                "n",
                S::Picker(PickerAction::Item(PickerItemAction::CreatePath)),
            ),
            (
                "r",
                S::Picker(PickerAction::Item(PickerItemAction::MovePath)),
            ),
            (
                "c",
                S::Picker(PickerAction::Item(PickerItemAction::CopyPath)),
            ),
            (
                "d",
                S::Picker(PickerAction::Item(PickerItemAction::DeletePath)),
            ),
            (
                "y",
                S::Picker(PickerAction::Item(PickerItemAction::YankPath)),
            ),
            ("q", S::Hide),
        ]));
        let windows = bindings(&[
            ("l", S::FocusEditor),
            ("C-l", S::FocusEditor),
            ("right", S::FocusEditor),
            ("w", S::NextView),
            ("C-w", S::NextView),
            ("W", S::PreviousView),
            ("q", S::Hide),
            ("C-q", S::Hide),
            ("b", S::Hide),
            ("r", S::Resize),
        ]);
        keys.0
            .insert("C-w".parse().unwrap(), Binding::Keys(windows));
        keys
    }
    pub fn git_picker(mut shared: Keymap<PickerAction>) -> Keymap<PickerAction> {
        use PickerItemAction as I;
        shared.merge(bindings(&[
            ("A-s", PickerAction::Item(I::Stage)),
            ("A-u", PickerAction::Item(I::Unstage)),
            ("C-x a s", PickerAction::Item(I::Stage)),
            ("C-x a u", PickerAction::Item(I::Unstage)),
            ("C-x a z", PickerAction::Item(I::Stash)),
            ("C-x a l", PickerAction::Item(I::Stashes)),
            ("C-x a ?", PickerAction::Item(I::Actions)),
        ]));
        shared
    }
    pub fn history_picker(mut shared: Keymap<PickerAction>) -> Keymap<PickerAction> {
        shared.merge(bindings(&[
            ("C-x a", PickerAction::Item(PickerItemAction::Actions)),
            ("C-x b", PickerAction::Item(PickerItemAction::HistoryBack)),
        ]));
        shared
    }

    pub fn file_explorer(mut shared: Keymap<PickerAction>) -> Keymap<PickerAction> {
        use PickerItemAction as I;
        shared.merge(bindings(&[
            ("A-n", PickerAction::Item(I::CreatePath)),
            ("A-m", PickerAction::Item(I::MovePath)),
            ("C-x n", PickerAction::Item(I::CreatePath)),
            ("C-x r", PickerAction::Item(I::MovePath)),
            ("C-x c", PickerAction::Item(I::CopyPath)),
            ("C-x d", PickerAction::Item(I::DeletePath)),
            ("C-x y", PickerAction::Item(I::YankPath)),
        ]));
        shared
    }

    pub fn branch_picker(mut shared: Keymap<PickerAction>) -> Keymap<PickerAction> {
        shared = super::inspect::filter(&shared, |action| *action != PickerAction::ToggleLoading);
        shared.merge(bindings(&[
            ("C-x n", PickerAction::Item(PickerItemAction::CreateBranch)),
            ("C-x d", PickerAction::Item(PickerItemAction::DeleteBranch)),
        ]));
        shared
    }

    pub fn worktree_picker(mut shared: Keymap<PickerAction>) -> Keymap<PickerAction> {
        shared = super::inspect::filter(&shared, |action| *action != PickerAction::ToggleLoading);
        shared.merge(bindings(&[
            (
                "C-x n",
                PickerAction::Item(PickerItemAction::CreateWorktree),
            ),
            (
                "C-x d",
                PickerAction::Item(PickerItemAction::RemoveWorktree),
            ),
            (
                "C-x a l",
                PickerAction::Item(PickerItemAction::LockWorktree),
            ),
            (
                "C-x a u",
                PickerAction::Item(PickerItemAction::UnlockWorktree),
            ),
        ]));
        shared
    }

    pub fn stash_picker(mut shared: Keymap<PickerAction>) -> Keymap<PickerAction> {
        use PickerItemAction as I;
        shared.merge(bindings(&[
            ("C-x a s", PickerAction::Item(I::ApplyStash)),
            ("C-x a i", PickerAction::Item(I::ApplyStashWithIndex)),
            ("C-x a ?", PickerAction::Item(I::Actions)),
        ]));
        shared
    }
}

impl Default for ComponentKeymaps {
    fn default() -> Self {
        use MenuAction as M;
        use PickerAction as K;
        use PopupAction as O;
        use PromptAction as P;
        let pages = bindings(&[("A-p", PageAction::Previous), ("A-n", PageAction::Next)]);
        let picker = bindings(&[
            ("C-s", K::AcceptHorizontalSplit),
            ("C-v", K::AcceptVerticalSplit),
            ("C-t", K::TogglePreview),
            ("C-q", K::PopulateQuicklist),
            ("A-r", K::ToggleReplacement),
            ("A-c", K::ToggleCaseSensitive),
            ("C-x j", K::PreviewScrollDown),
            ("C-x k", K::PreviewScrollUp),
            ("C-x [", K::PreviewPrevious),
            ("C-x ]", K::PreviewNext),
            ("C-x s", K::AcceptHorizontalSplit),
            ("C-x v", K::AcceptVerticalSplit),
            ("C-x ret", K::AcceptKeepOpen),
            ("C-x p", K::TogglePreview),
            ("C-x l", K::ToggleLoading),
            ("C-x q", K::PopulateQuicklist),
            ("C-x g", K::MoveStart),
            ("C-x G", K::MoveEnd),
            ("C-x h", K::HistoryPrevious),
            ("C-x H", K::HistoryNext),
            ("C-x r", K::ToggleReplacement),
            ("C-x R", K::ReplaceAll),
            ("C-x f", K::ToggleCaseSensitive),
            ("C-x o", K::OpenPreview),
            ("A-n", K::PreviewNext),
            ("A-p", K::PreviewPrevious),
            ("S-tab", K::MoveUp),
            ("up", K::MoveUp),
            ("C-p", K::MoveUp),
            ("tab", K::MoveDown),
            ("down", K::MoveDown),
            ("C-n", K::MoveDown),
            ("pagedown", K::PageDown),
            ("pageup", K::PageUp),
            ("esc", K::Cancel),
            ("C-c", K::Cancel),
            ("A-ret", K::AcceptKeepOpen),
            ("ret", K::Accept),
            ("C-ret", K::ReplaceAll),
            ("Meta-ret", K::ReplaceAll),
            ("A-k", K::PreviewScrollUp),
            ("A-up", K::PreviewScrollUp),
            ("A-j", K::PreviewScrollDown),
            ("A-down", K::PreviewScrollDown),
            ("A-left", K::PreviewPrevious),
            ("A-right", K::PreviewNext),
            ("A-o", K::OpenPreview),
        ]);
        Self {
            prompt: bindings(&[
                ("esc", P::Cancel),
                ("C-c", P::Cancel),
                ("ret", P::Accept),
                ("A-b", P::MoveWordLeft),
                ("C-left", P::MoveWordLeft),
                ("A-f", P::MoveWordRight),
                ("C-right", P::MoveWordRight),
                ("C-b", P::MoveLeft),
                ("left", P::MoveLeft),
                ("C-f", P::MoveRight),
                ("right", P::MoveRight),
                ("C-e", P::MoveEnd),
                ("end", P::MoveEnd),
                ("C-a", P::MoveStart),
                ("home", P::MoveStart),
                ("C-w", P::DeleteWordBackward),
                ("A-backspace", P::DeleteWordBackward),
                ("C-backspace", P::DeleteWordBackward),
                ("A-d", P::DeleteWordForward),
                ("A-del", P::DeleteWordForward),
                ("C-del", P::DeleteWordForward),
                ("C-k", P::DeleteToEnd),
                ("C-u", P::DeleteToStart),
                ("C-h", P::DeleteBackward),
                ("backspace", P::DeleteBackward),
                ("S-backspace", P::DeleteBackward),
                ("C-d", P::DeleteForward),
                ("del", P::DeleteForward),
                ("A-w", P::InsertWord),
                ("C-p", P::HistoryPrevious),
                ("up", P::HistoryPrevious),
                ("C-n", P::HistoryNext),
                ("down", P::HistoryNext),
                ("tab", P::CompleteNext),
                ("S-tab", P::CompletePrevious),
                ("C-q", P::ExitSelection),
                ("C-r", P::InsertRegister),
            ]),
            buffer_picker: Self::buffer_picker(picker.clone()),
            quicklist_picker: quicklist_picker_keymap(&picker),
            git_picker: Self::git_picker(picker.clone()),
            branch_picker: Self::branch_picker(picker.clone()),
            worktree_picker: Self::worktree_picker(picker.clone()),
            stash_picker: Self::stash_picker(picker.clone()),
            history_picker: Self::history_picker(picker.clone()),
            file_explorer: Self::file_explorer(picker.clone()),
            sidebar: Self::sidebar(Self::file_explorer(picker.clone())),
            sidebar_filter: Self::sidebar_filter(Self::file_explorer(picker.clone())),
            picker,
            menu: bindings(&[
                ("esc", M::Cancel),
                ("C-c", M::Cancel),
                ("ret", M::Accept),
                ("S-tab", M::MoveUp),
                ("up", M::MoveUp),
                ("C-p", M::MoveUp),
                ("tab", M::Accept),
                ("down", M::MoveDown),
                ("C-n", M::MoveDown),
                ("pageup", M::PageUp),
                ("C-u", M::PageUp),
                ("pagedown", M::PageDown),
                ("C-d", M::PageDown),
            ]),
            popup: bindings(&[
                ("esc", O::Cancel),
                ("C-c", O::Cancel),
                ("pagedown", O::ScrollDown),
                ("C-d", O::ScrollDown),
                ("pageup", O::ScrollUp),
                ("C-u", O::ScrollUp),
            ]),
            hover: pages.clone(),
            signature_help: pages,
            diff: bindings(&[
                ("left", DiffAction::ScrollLeft),
                ("right", DiffAction::ScrollRight),
                ("r", DiffAction::Restore),
            ]),
            blame: bindings(&[
                ("y", BlameAction::CopyCommitId),
                ("Y", BlameAction::CopyMessage),
            ]),
        }
    }
}

pub(crate) fn action_hint<A: PartialEq>(keys: &Keymap<A>, action: &A) -> String {
    fn best<A: PartialEq>(keys: &Keymap<A>, action: &A) -> Option<(usize, String)> {
        keys.0
            .iter()
            .filter_map(|(key, binding)| {
                let (count, suffix) = match binding {
                    Binding::Actions(actions)
                        if actions.as_slice() == std::slice::from_ref(action) =>
                    {
                        (0, String::new())
                    }
                    Binding::Keys(keys) => best(keys, action)?,
                    _ => return None,
                };
                Some((count + 1, format!("{}{suffix}", key.key_sequence_format())))
            })
            .min_by(|(count_a, hint_a), (count_b, hint_b)| {
                count_a
                    .cmp(count_b)
                    .then(hint_a.len().cmp(&hint_b.len()))
                    .then(hint_a.cmp(hint_b))
            })
    }
    best(keys, action).map_or_else(|| "unbound".into(), |(_, hint)| hint)
}
