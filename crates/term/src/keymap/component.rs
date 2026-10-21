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
    PopulateQuicklist,
    /// Item actions are supplied only by the schema for a specific picker.
    #[serde(skip)]
    Item(PickerItemAction),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PickerItemAction {
    CloseBuffer,
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

/// Terminal component maps, resolved from defaults and user configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComponentKeymaps {
    pub prompt: Keymap<PromptAction>,
    pub picker: Keymap<PickerAction>,
    pub buffer_picker: Keymap<PickerAction>,
    pub menu: Keymap<MenuAction>,
    pub popup: Keymap<PopupAction>,
    pub hover: Keymap<PageAction>,
    pub signature_help: Keymap<PageAction>,
    pub blame: Keymap<BlameAction>,
}

fn bindings<A: Copy>(entries: &[(&str, A)]) -> Keymap<A> {
    let mut keys = Keymap::default();
    for &(key, action) in entries {
        assert!(
            keys.insert(key.parse().unwrap(), Binding::Action(action))
                .is_none(),
            "duplicate default binding: {key}"
        );
    }
    keys
}

impl Default for ComponentKeymaps {
    fn default() -> Self {
        use MenuAction as M;
        use PickerAction as K;
        use PopupAction as O;
        use PromptAction as P;
        let pages = bindings(&[("A-p", PageAction::Previous), ("A-n", PageAction::Next)]);
        let picker = bindings(&[
            ("S-tab", K::MoveUp),
            ("up", K::MoveUp),
            ("C-p", K::MoveUp),
            ("tab", K::MoveDown),
            ("down", K::MoveDown),
            ("C-n", K::MoveDown),
            ("pagedown", K::PageDown),
            ("C-d", K::PageDown),
            ("pageup", K::PageUp),
            ("C-u", K::PageUp),
            ("home", K::MoveStart),
            ("end", K::MoveEnd),
            ("esc", K::Cancel),
            ("C-c", K::Cancel),
            ("C-q", K::PopulateQuicklist),
            ("A-ret", K::AcceptKeepOpen),
            ("ret", K::Accept),
            ("C-s", K::AcceptHorizontalSplit),
            ("C-v", K::AcceptVerticalSplit),
            ("C-t", K::TogglePreview),
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
                ("C-s", P::InsertWord),
                ("C-p", P::HistoryPrevious),
                ("up", P::HistoryPrevious),
                ("C-n", P::HistoryNext),
                ("down", P::HistoryNext),
                ("tab", P::CompleteNext),
                ("S-tab", P::CompletePrevious),
                ("C-q", P::ExitSelection),
                ("C-r", P::InsertRegister),
            ]),
            buffer_picker: picker.clone(),
            picker,
            menu: bindings(&[
                ("esc", M::Cancel),
                ("C-c", M::Cancel),
                ("ret", M::Accept),
                ("S-tab", M::MoveUp),
                ("up", M::MoveUp),
                ("C-p", M::MoveUp),
                ("tab", M::MoveDown),
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
            blame: bindings(&[
                ("y", BlameAction::CopyCommitId),
                ("Y", BlameAction::CopyMessage),
            ]),
        }
    }
}

impl ui_core::keymap::KeymapAction for PromptAction {}
impl ui_core::keymap::KeymapAction for MenuAction {}
impl ui_core::keymap::KeymapAction for PickerAction {}
impl ui_core::keymap::KeymapAction for PopupAction {}
impl ui_core::keymap::KeymapAction for PageAction {}
impl ui_core::keymap::KeymapAction for BlameAction {}
