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

impl ClipboardProvider {
    pub(crate) fn plugin_custom_command(
        &self,
        kind: super::ClipboardType,
        write: bool,
    ) -> Option<(String, Vec<String>)> {
        let Self::Custom(provider) = self else {
            return None;
        };
        let command = match (write, kind) {
            (false, super::ClipboardType::Clipboard) => &provider.yank,
            // Match native Custom providers: reads historically use `yank`
            // for both registers; checking `yank_primary` could authorize a
            // different executable from the one actually invoked.
            (false, super::ClipboardType::Selection) => &provider.yank,
            (true, super::ClipboardType::Clipboard) => &provider.paste,
            (true, super::ClipboardType::Selection) => provider.paste_primary.as_ref()?,
        };
        Some((
            command.command.to_string(),
            command.args.iter().map(|arg| arg.to_string()).collect(),
        ))
    }
}
