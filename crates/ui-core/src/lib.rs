//! Lightweight UI types shared across rendering and terminal backends.
//!
//! Keep this crate independent from editor state and protocol clients. That
//! boundary lets terminal-only changes compile without rebuilding the editor,
//! LSP, DAP, and VCS dependency graph.

pub mod buffer;
pub mod graphics;
pub mod input;
pub mod keyboard;
pub mod keymap;

pub mod theme {
    pub use crate::graphics::{Color, Modifier, Style};

    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
    /// Coarse terminal appearance reported by supported backends.
    pub enum Mode {
        /// A dark terminal background.
        Dark,
        /// A light terminal background.
        Light,
    }

    #[cfg(feature = "term")]
    impl From<termina::escape::csi::ThemeMode> for Mode {
        fn from(mode: termina::escape::csi::ThemeMode) -> Self {
            match mode {
                termina::escape::csi::ThemeMode::Dark => Self::Dark,
                termina::escape::csi::ThemeMode::Light => Self::Light,
            }
        }
    }
}

pub mod terminal {
    use serde::{Deserialize, Serialize};

    #[derive(
        Debug, Default, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize, Clone, Copy,
    )]
    #[serde(rename_all = "kebab-case")]
    /// Policy for enabling the Kitty keyboard protocol.
    pub enum KittyKeyboardProtocolConfig {
        /// Enable the protocol only when terminal capability detection supports it.
        #[default]
        Auto,
        /// Never enable the protocol.
        Disabled,
        /// Enable the protocol even when it was not detected automatically.
        Enabled,
    }
}
