//! Serializable clipboard settings. Runtime services live alongside this module.

use serde::{Deserialize, Serialize};
use std::borrow::Cow;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Command {
    pub(super) command: Cow<'static, str>,
    #[serde(default)]
    pub(super) args: Cow<'static, [Cow<'static, str>]>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub struct CommandProvider {
    pub(super) yank: Command,
    pub(super) paste: Command,
    pub(super) yank_primary: Option<Command>,
    pub(super) paste_primary: Option<Command>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
#[allow(clippy::large_enum_variant)]
/// Configured provider choice; execution belongs to a `ClipboardBackend`.
pub enum ClipboardProvider {
    Pasteboard,
    Wayland,
    XClip,
    XSel,
    Win32Yank,
    Tmux,
    #[cfg(windows)]
    Windows,
    Termux,
    Termcode,
    Custom(CommandProvider),
    None,
}

impl Default for ClipboardProvider {
    fn default() -> Self {
        #[cfg(not(target_arch = "wasm32"))]
        {
            super::native::default_provider()
        }
        #[cfg(target_arch = "wasm32")]
        {
            Self::None
        }
    }
}
